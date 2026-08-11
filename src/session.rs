//! Sessions are git worktrees on their own branch.
//!
//! This is the part that makes the sandbox real. The container protects your
//! *host*; the worktree protects your *repo*. Your working tree is never
//! mounted, so an agent cannot touch uncommitted work or `main` — review is a
//! plain `git diff`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What `remove` did with the branch, so the caller can report it truthfully
/// rather than always claiming the branch was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    /// Commits nobody has reviewed; the branch outlives the session.
    BranchKept,
    /// Nothing was committed, so the branch preserved nothing.
    BranchDropped,
    /// A scratch session (`omh auth`, `omh doctor`) never had one.
    NoBranch,
}

pub struct Session {
    pub id: String,
    /// `None` for a scratch session. `omh auth` and `omh doctor` need a
    /// container and a writable `/work`, not somewhere to keep work — giving
    /// them a branch litters the user's namespace with names like `omh/auth`
    /// that outlive the command that made them.
    pub branch: Option<String>,
    pub worktree: PathBuf,
}

impl Session {
    pub fn new(worktrees_dir: &Path, id: String) -> Self {
        Self {
            branch: Some(format!("omh/{id}")),
            worktree: worktrees_dir.join(&id),
            id,
        }
    }

    /// A throwaway working directory with no branch and no git registration.
    pub fn scratch(dir: PathBuf, id: String) -> Self {
        Self {
            branch: None,
            worktree: dir,
            id,
        }
    }

    /// What to call this session in output.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.id)
    }

    /// Create the worktree if it does not exist yet. Idempotent, so relaunching
    /// into an existing session resumes it.
    pub fn ensure(&self, repo: &Path, base: &str) -> Result<()> {
        if self.worktree.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(self.worktree.parent().unwrap())?;
        let path = self.worktree.to_string_lossy().into_owned();

        // No branch means no git involvement at all — just a writable directory.
        let Some(branch) = self.branch.clone() else {
            std::fs::create_dir_all(&self.worktree)?;
            return Ok(());
        };

        // A directory removed outside git leaves its registration behind, and
        // `worktree add` then refuses forever. Prune first so a session id can
        // never become permanently unusable. Prune only drops entries whose
        // directory is already gone, so live sessions are unaffected.
        let _ = git(repo, &["worktree", "prune"]);

        // `omh rm` keeps branches on purpose, so a session id can outlive its
        // worktree. Reattach to the existing branch rather than failing — that
        // is what resuming a session means.
        let args: Vec<&str> = if self.branch_exists(repo) {
            vec!["worktree", "add", &path, &branch]
        } else {
            // Explicit base: without it git branches from whatever HEAD is,
            // which produces a session whose diff has the wrong baseline.
            vec!["worktree", "add", "-b", &branch, &path, base]
        };
        git(repo, &args).with_context(|| format!("creating worktree for session {}", self.id))?;
        Ok(())
    }

    fn branch_exists(&self, repo: &Path) -> bool {
        let Some(branch) = &self.branch else {
            return false;
        };
        git(repo, &["rev-parse", "--verify", "--quiet", branch]).is_ok()
    }

    /// How many commits `base` has that this session does not. A session that
    /// silently drifts behind trunk makes the agent work against stale code.
    pub fn behind(&self, repo: &Path, base: &str) -> usize {
        let Some(branch) = &self.branch else { return 0 };
        git(repo, &["rev-list", "--count", &format!("{branch}..{base}")])
            .ok()
            .and_then(|out| out.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Commits on this session's branch that are not already in `base`.
    ///
    /// The question `remove` needs answered: whether keeping the branch would
    /// preserve anything.
    pub fn commits(&self, repo: &Path, base: &str) -> usize {
        let Some(branch) = &self.branch else { return 0 };
        git(repo, &["rev-list", "--count", &format!("{base}..{branch}")])
            .ok()
            .and_then(|out| out.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn remove(&self, repo: &Path, base: &str) -> Result<Removed> {
        // Decided before the worktree goes, because afterwards the branch is
        // the only thing left to ask.
        let outcome = match &self.branch {
            None => Removed::NoBranch,
            Some(_) if self.commits(repo, base) > 0 => Removed::BranchKept,
            Some(_) => Removed::BranchDropped,
        };

        if git(
            repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.worktree.to_string_lossy(),
            ],
        )
        .is_err()
        {
            // git can lose the registration while the directory survives, and
            // then refuses with "is not a working tree" — leaving a session
            // that can never be removed. Clean up whatever is on disk.
            if self.worktree.exists() {
                std::fs::remove_dir_all(&self.worktree)
                    .with_context(|| format!("removing {}", self.worktree.display()))?;
            }
            let _ = git(repo, &["worktree", "prune"]);
        }

        // A branch carrying commits outlives its worktree on purpose: removing
        // a session must never destroy work nobody has reviewed. A branch
        // carrying none holds nothing to review — `--force` above has already
        // discarded anything uncommitted — so keeping it only leaves a dead ref
        // behind after every abandoned session.
        if outcome == Removed::BranchDropped {
            if let Some(branch) = &self.branch {
                let _ = git(repo, &["branch", "-D", branch]);
            }
        }
        Ok(outcome)
    }

    pub fn diff(&self, repo: &Path, base: &str) -> Result<String> {
        let branch = self
            .branch
            .as_deref()
            .context("a scratch session has no branch")?;
        git(repo, &["diff", "--stat", &format!("{base}...{branch}")])
    }

    /// Stage everything in the worktree and commit it onto the session branch.
    ///
    /// Runs in the worktree rather than the checkout. On the host its `.git`
    /// pointer resolves, which is the whole reason this can be a plain git call
    /// — inside the sandbox that pointer leads nowhere and none of this works.
    ///
    /// `add -A` is only safe because `carry` has already hidden what omh itself
    /// put in the worktree: carried files and the `CLAUDE.md`/`AGENTS.md` staged
    /// at launch. That exclusion is the sole thing keeping omh's own staging out
    /// of the user's commit, and nothing else links the two modules.
    pub fn commit(&self, message: Option<&str>) -> Result<()> {
        self.branch
            .as_deref()
            .context("a scratch session has no branch to commit to")?;

        git(&self.worktree, &["add", "-A"])?;

        // Asked *after* staging, and against the index rather than the worktree:
        // `git diff` says nothing about untracked files, so a session whose only
        // work is new files reads as clean when the question comes first. That
        // is the shape of `e0a41b8`, where a release published an empty tap and
        // reported success.
        let staged = git(&self.worktree, &["diff", "--cached", "--name-only"])?;
        if staged.trim().is_empty() {
            anyhow::bail!("nothing to commit in {}", self.label());
        }

        // Verbatim, and no trailer. omh has no view on what the work was for,
        // which is the refusal `omh why` already makes about rationale it does
        // not hold.
        match message {
            Some(message) => {
                git(&self.worktree, &["commit", "-q", "-m", message])?;
            }
            // git's own editor flow rather than `$EDITOR` directly: it already
            // knows `core.editor`, the commented template, and that an empty
            // message aborts. Reaching past it would reimplement three things
            // slightly wrong. Inherited stdio, because `git()` captures output
            // and an editor with nowhere to draw hangs.
            None => {
                let status = Command::new("git")
                    .current_dir(&self.worktree)
                    .arg("commit")
                    .status()
                    .context("running git commit")?;
                anyhow::ensure!(status.success(), "commit aborted");
            }
        }
        Ok(())
    }

    /// Changed files sitting in the worktree, committed to nothing.
    ///
    /// What omh staged itself is excluded by `carry`, so it is not counted here
    /// for the same reason `commit` does not commit it — a session reporting
    /// "2 uncommitted" that means omh's own `CLAUDE.md` is a session nobody
    /// looks at twice.
    pub fn uncommitted(&self) -> usize {
        git(&self.worktree, &["status", "--porcelain"])
            .map(|out| out.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    /// Commits this session has that its upstream does not.
    ///
    /// `None` means there is no upstream yet, which is a different state from
    /// zero: one says name it and push, the other says you are done.
    pub fn unpushed(&self) -> Option<usize> {
        let branch = self.branch.as_deref()?;
        self.upstream()?;
        git(
            &self.worktree,
            &["rev-list", "--count", &format!("@{{u}}..{branch}")],
        )
        .ok()
        .and_then(|out| out.trim().parse().ok())
    }

    /// The remote branch this session tracks, once it has one.
    pub fn upstream(&self) -> Option<String> {
        git(
            &self.worktree,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        )
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|out| !out.is_empty())
    }

    /// Push the session branch to origin under a name a reviewer can read, and
    /// return the name it landed under.
    ///
    /// Naming is required the first time and never again. `omh/s01` records
    /// *when* the work happened rather than what it was, and on origin it
    /// outlives the session that would explain it — so omh refuses rather than
    /// choosing, the same refusal it makes about commit messages.
    pub fn push(&self, name: Option<&str>) -> Result<String> {
        let branch = self
            .branch
            .as_deref()
            .context("a scratch session has no branch to push")?;

        let target = match (name, self.upstream()) {
            (Some(name), _) => name.to_string(),
            // `origin/fix/tap-guard` → `fix/tap-guard`. Split once, because a
            // branch name may hold slashes of its own and usually does.
            (None, Some(up)) => up
                .split_once('/')
                .map(|(_, branch)| branch.to_string())
                .unwrap_or(up),
            (None, None) => anyhow::bail!(
                "{branch} is a session id, not a branch name\n  name it:  omh s push <name>"
            ),
        };

        git(
            &self.worktree,
            &[
                "push",
                "-u",
                "origin",
                &format!("{branch}:refs/heads/{target}"),
            ],
        )?;

        // Read it back from origin before calling this a success. Every step
        // above can pass against a local repo while the remote stays untouched
        // — the failure `e0a41b8` shipped, where a release published an empty
        // tap with every job green.
        let local = git(&self.worktree, &["rev-parse", branch])?;
        let published = git(
            &self.worktree,
            &["ls-remote", "origin", &format!("refs/heads/{target}")],
        )?;
        let published = published.split_whitespace().next().unwrap_or_default();
        anyhow::ensure!(
            published == local.trim(),
            "push reported success, but origin/{target} does not hold {branch}"
        );
        Ok(target)
    }
}

/// Reject a session id that is not a single path component.
///
/// `-s` reaches `Session::new` straight from the command line and the worktree
/// path is joined from it — and `remove` deletes that directory.
pub fn validate_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        anyhow::bail!("a session needs a name");
    }
    if id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        anyhow::bail!("a session id is a single name, not a path: `{id}`");
    }
    Ok(())
}

/// The branch a session should be reviewed against. Hardcoding `main` breaks
/// every repo that still uses `master`, or any other convention — and it fails
/// at review time, after the agent has already done the work.
pub fn default_branch(repo: &Path) -> String {
    // What the remote says is authoritative when there is one — but only while
    // it still points at something. `origin/HEAD` is cached at clone time and
    // nothing refreshes it when a repo renames its trunk, so a repo cloned back
    // when it was `master` keeps claiming `master` forever. Taking that claim on
    // faith fails at `worktree add` with `invalid reference`, which is every
    // session, not just review time.
    if let Ok(head) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = head.trim().strip_prefix("origin/") {
            let remote_ref = format!("refs/remotes/origin/{name}");
            if !name.is_empty()
                && git(repo, &["rev-parse", "--verify", "--quiet", &remote_ref]).is_ok()
            {
                return name.to_string();
            }
        }
    }
    for candidate in ["main", "master"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return candidate.to_string();
        }
    }
    // Whatever this repo actually calls its trunk.
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".into())
}

/// The session a bare `omh <harness>` or `omh attach` should land in: the most
/// recently created one.
pub fn current(worktrees_dir: &Path) -> Option<String> {
    list(worktrees_dir).pop()
}

/// Resolve which session to use. Creating a fresh one on every launch would
/// defeat persistence entirely — you would never reattach to the agent you left
/// running — so a new session is something you ask for.
pub fn pick(worktrees_dir: &Path, explicit: Option<&str>, new: bool) -> String {
    if let Some(id) = explicit {
        return id.to_string();
    }
    if new {
        return next_id(worktrees_dir);
    }
    current(worktrees_dir).unwrap_or_else(|| next_id(worktrees_dir))
}

/// Human-readable, monotonic session ids: `s01`, `s02`, ...
pub fn next_id(worktrees_dir: &Path) -> String {
    let used = std::fs::read_dir(worktrees_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_prefix('s')?
                        .parse::<u32>()
                        .ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    format!("s{:02}", used + 1)
}

pub fn list(worktrees_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(worktrees_dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            git(&root, &args).unwrap();
        }
        (dir, root)
    }

    /// `rm` keeps a branch so unreviewed work is unloseable. A branch with no
    /// commits holds no work to lose — `worktree remove --force` has already
    /// discarded anything uncommitted — so keeping it preserves nothing and
    /// leaves a dead ref behind after every abandoned session.
    #[test]
    fn removing_a_session_that_produced_nothing_drops_its_branch() {
        let (dir, root) = repo();
        let s = Session::new(&dir.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        let outcome = s.remove(&root, "main").unwrap();
        assert_eq!(outcome, Removed::BranchDropped);
        assert!(
            git(&root, &["rev-parse", "--verify", "omh/s01"]).is_err(),
            "an empty branch should not survive its session"
        );
    }

    /// The load-bearing half: a branch carrying commits must survive, because
    /// `rm` must never be able to destroy work nobody has reviewed.
    #[test]
    fn removing_a_session_that_committed_keeps_its_branch() {
        let (dir, root) = repo();
        let s = Session::new(&dir.path().join("wt"), "s02".into());
        s.ensure(&root, "main").unwrap();

        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "."]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();

        let outcome = s.remove(&root, "main").unwrap();
        assert_eq!(outcome, Removed::BranchKept);
        assert!(
            git(&root, &["rev-parse", "--verify", "omh/s02"]).is_ok(),
            "unreviewed work must be unloseable"
        );
    }

    /// A scratch session (`omh auth`, `omh doctor`) has no branch at all, and
    /// asking git about one would error rather than report nothing to keep.
    #[test]
    fn removing_a_scratch_session_reports_no_branch() {
        let (dir, root) = repo();
        let mut s = Session::new(&dir.path().join("wt"), "s03".into());
        s.branch = None;
        s.ensure(&root, "main").unwrap();
        assert_eq!(s.remove(&root, "main").unwrap(), Removed::NoBranch);
    }

    #[test]
    fn ensure_creates_worktree_on_its_own_branch() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        assert!(s.worktree.join(".git").exists());
        assert_eq!(s.branch.as_deref(), Some("omh/s01"));
    }

    #[test]
    fn ensure_is_idempotent() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        s.ensure(&root, "main").unwrap();
    }

    /// Regression: `rm` keeps branches on purpose so unreviewed work can never be
    /// destroyed — which made reusing a session id fail, because `ensure` always
    /// passed `-b`. Resuming a session must reattach to its existing branch.
    #[test]
    fn session_resumes_onto_a_surviving_branch() {
        let (d, root) = repo();
        let wt = d.path().join("wt");
        let s = Session::new(&wt, "s01".into());

        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();

        s.remove(&root, "main").unwrap();
        assert!(!s.worktree.exists(), "worktree gone");
        assert!(s.branch_exists(&root), "branch must survive rm");

        s.ensure(&root, "main").unwrap();
        assert_eq!(
            std::fs::read_to_string(s.worktree.join("work.txt")).unwrap(),
            "agent output",
            "resuming must recover the branch's work, not start empty"
        );
    }

    #[test]
    fn diff_reports_against_base() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("new.rs"), "fn main() {}").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "add"]).unwrap();
        assert!(s.diff(&root, "main").unwrap().contains("new.rs"));
    }

    // ── committing a session's work ─────────────────────────────────────────

    /// What the commit actually contains, which is the only question these
    /// tests are asking. `git status` would answer about the worktree instead.
    fn committed_files(wt: &Path) -> String {
        git(wt, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap()
    }

    /// `commit` stages everything, so the guarantee that omh's own staging stays
    /// out of the user's work rests entirely on `carry`'s exclusion holding.
    /// Nothing else connects these two modules, and the failure — omh's
    /// `CLAUDE.md` riding into a PR on the commit omh itself made — is invisible
    /// until a reviewer finds it.
    #[test]
    fn a_file_omh_staged_never_reaches_the_commit() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        std::fs::write(s.worktree.join("CLAUDE.md"), "staged by omh").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        s.commit(Some("Add the work")).unwrap();

        let files = committed_files(&s.worktree);
        assert!(
            files.contains("work.rs"),
            "the agent's work must land: {files}"
        );
        assert!(
            !files.contains("CLAUDE.md"),
            "omh's own staging must not: {files}"
        );
    }

    /// `git diff` does not report untracked files, so asking whether anything
    /// changed *before* staging answers "nothing" for a session whose only work
    /// is new files. That is `e0a41b8` — the tap formula a release published as
    /// a no-op — arriving in a second place.
    #[test]
    fn a_brand_new_file_is_committed_rather_than_read_as_nothing_to_do() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("brand-new.rs"), "fn main() {}").unwrap();

        s.commit(Some("Add a new file")).unwrap();

        assert!(committed_files(&s.worktree).contains("brand-new.rs"));
    }

    /// A no-op reporting success teaches people to trust a commit that never
    /// happened, and the next command they run is `push`.
    #[test]
    fn committing_a_clean_worktree_is_an_error() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        let err = s.commit(Some("nothing to say")).unwrap_err();
        assert!(err.to_string().contains("nothing to commit"), "got: {err}");
    }

    /// omh has no view on what the work was for and will not invent one — the
    /// same refusal `omh why` makes about rationale it does not hold.
    #[test]
    fn the_commit_message_is_exactly_what_was_given() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        s.commit(Some("Fix the guard")).unwrap();

        let message = git(&s.worktree, &["log", "-1", "--format=%B"]).unwrap();
        assert_eq!(
            message.trim(),
            "Fix the guard",
            "omh added something to the message"
        );
    }

    // ── pushing it somewhere a PR can be opened from ────────────────────────

    /// A real bare remote rather than a mock: half of what `push` promises is
    /// that the branch actually arrived, and nothing you can stub answers that.
    fn repo_with_origin() -> (tempfile::TempDir, PathBuf) {
        let (d, root) = repo();
        let origin = d.path().join("origin.git");
        git(
            d.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        )
        .unwrap();
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        )
        .unwrap();
        (d, root)
    }

    fn session_with_a_commit(root: &Path, wt: &Path, file: &str) -> Session {
        let s = Session::new(wt, "s01".into());
        s.ensure(root, "main").unwrap();
        std::fs::write(s.worktree.join(file), "fn main() {}").unwrap();
        s.commit(Some("Add the work")).unwrap();
        s
    }

    /// `omh/s01` names when the work happened, not what it was. On origin it
    /// outlives the session that explains it, and it is what the PR inherits.
    #[test]
    fn pushing_without_a_name_refuses_rather_than_using_the_session_id() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");

        let err = s.push(None).unwrap_err();

        assert!(err.to_string().contains("not a branch name"), "got: {err}");
    }

    #[test]
    fn a_named_push_reaches_the_remote_and_sets_upstream() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");

        s.push(Some("fix/tap-guard")).unwrap();

        assert_eq!(s.upstream().as_deref(), Some("origin/fix/tap-guard"));
        let on_remote = git(&root, &["ls-remote", "origin", "fix/tap-guard"]).unwrap();
        assert!(
            !on_remote.trim().is_empty(),
            "the branch must actually be on origin"
        );
    }

    /// Naming it once is the whole bargain: refusing every time would be a
    /// command you cannot put in a loop.
    #[test]
    fn a_later_push_needs_no_name() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");
        s.push(Some("fix/tap-guard")).unwrap();

        std::fs::write(s.worktree.join("more.rs"), "fn more() {}").unwrap();
        s.commit(Some("Add more")).unwrap();

        assert_eq!(s.push(None).unwrap(), "fix/tap-guard");
    }

    /// Every local step can succeed while the remote a reviewer would open the
    /// PR from stays untouched — which is `e0a41b8` exactly: a release job that
    /// copied, staged, pushed and passed, against a local clone, while the tap
    /// it was publishing to stayed empty.
    ///
    /// Reproduced with a `pushurl`, because that is the configuration where the
    /// push genuinely succeeds and origin genuinely does not have it. Deleting
    /// the remote instead would only prove that `git push` fails when there is
    /// nothing to push to, which needs no guard of ours — the first version of
    /// this test did exactly that and stayed green with the check removed.
    #[test]
    fn a_push_that_did_not_reach_origin_is_not_a_success() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");
        let elsewhere = d.path().join("elsewhere.git");
        git(
            d.path(),
            &["init", "-q", "--bare", elsewhere.to_str().unwrap()],
        )
        .unwrap();
        git(
            &root,
            &[
                "config",
                "remote.origin.pushurl",
                elsewhere.to_str().unwrap(),
            ],
        )
        .unwrap();

        let err = s.push(Some("fix/tap-guard")).unwrap_err();

        assert!(
            err.to_string().contains("does not hold"),
            "the push succeeded; only the read-back can catch this: {err}"
        );
    }

    // ── what `s ls` reports about work in flight ────────────────────────────

    /// The state that strands work is the one `s ls` could not see: a session
    /// holding a day of uncommitted changes looked exactly like an untouched
    /// one. It must not count what omh itself put there, for the same reason
    /// `commit` must not commit it.
    #[test]
    fn uncommitted_counts_the_agents_work_and_not_omhs_own() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        std::fs::write(s.worktree.join("CLAUDE.md"), "staged by omh").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        assert_eq!(s.uncommitted(), 1);
    }

    #[test]
    fn a_clean_session_reports_nothing_uncommitted() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        assert_eq!(s.uncommitted(), 0);
    }

    /// Before a push there is no upstream to measure against, which is a
    /// different answer from "nothing to push" — one means name it, the other
    /// means you are done.
    #[test]
    fn unpushed_distinguishes_never_pushed_from_up_to_date() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");
        assert_eq!(s.unpushed(), None, "no upstream yet");

        s.push(Some("fix/tap-guard")).unwrap();
        assert_eq!(s.unpushed(), Some(0), "everything is on origin");

        std::fs::write(s.worktree.join("more.rs"), "fn more() {}").unwrap();
        s.commit(Some("Add more")).unwrap();
        assert_eq!(s.unpushed(), Some(1));
    }

    /// `omh auth` and `omh doctor` get a writable directory, not somewhere to
    /// keep work. `diff` already refuses them; so must this.
    #[test]
    fn a_scratch_session_cannot_be_committed() {
        let d = tempfile::tempdir().unwrap();
        let s = Session::scratch(d.path().join("scratch"), "doctor".into());
        std::fs::create_dir_all(&s.worktree).unwrap();
        assert!(s.commit(Some("anything")).is_err());
    }

    #[test]
    fn ids_increment_and_list_in_order() {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        assert_eq!(next_id(&wt), "s01");
        std::fs::create_dir_all(wt.join("s01")).unwrap();
        std::fs::create_dir_all(wt.join("s02")).unwrap();
        assert_eq!(next_id(&wt), "s03");
        assert_eq!(list(&wt), ["s01", "s02"]);
    }

    #[test]
    fn git_failure_surfaces_stderr() {
        let (_d, root) = repo();
        let err = git(&root, &["rev-parse", "--verify", "nope"]).unwrap_err();
        assert!(err.to_string().contains("rev-parse"), "got: {err}");
    }

    /// Regression: a worktree directory removed outside git (manual `rm -rf`,
    /// disk cleanup, a stale checkout) left the registration behind, and every
    /// later `ensure` failed with "missing but already registered". A session id
    /// must never become permanently unusable.
    #[test]
    fn a_worktree_deleted_behind_gits_back_is_recoverable() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        std::fs::remove_dir_all(&s.worktree).unwrap();

        s.ensure(&root, "main")
            .expect("must recover rather than fail forever");
        assert!(s.worktree.join(".git").exists());
    }

    /// Pruning must not disturb a session whose directory is still there.
    #[test]
    fn recovering_one_session_leaves_others_alone() {
        let (d, root) = repo();
        let wt = d.path().join("wt");
        let keep = Session::new(&wt, "s01".into());
        let lose = Session::new(&wt, "s02".into());
        keep.ensure(&root, "main").unwrap();
        lose.ensure(&root, "main").unwrap();

        std::fs::remove_dir_all(&lose.worktree).unwrap();
        lose.ensure(&root, "main").unwrap();

        assert!(
            keep.worktree.join(".git").exists(),
            "untouched session survived"
        );
        assert!(lose.worktree.join(".git").exists());
    }

    fn repo_on(branch: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", branch],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            git(&root, &args).unwrap();
        }
        (dir, root)
    }

    /// Regression: `omh diff` assumed `main`, so on a `master` repo it failed at
    /// review time — after the agent had already done the work.
    #[test]
    fn the_default_branch_is_detected_not_assumed() {
        for branch in ["main", "master", "trunk"] {
            let (_d, root) = repo_on(branch);
            assert_eq!(default_branch(&root), branch, "on a {branch} repo");
        }
    }

    /// The remote knows its own trunk better than any local convention does, so
    /// `origin/HEAD` outranks the `main`/`master` guess.
    #[test]
    fn the_remotes_own_answer_outranks_local_convention() {
        let (_d, root) = repo_on("main");
        git(&root, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]).unwrap();
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        )
        .unwrap();

        assert_eq!(default_branch(&root), "trunk");
    }

    /// Regression: `origin/HEAD` is a cached guess from clone time that nothing
    /// updates when a repo renames its trunk, so it can name a branch that no
    /// longer exists. Trusting it unchecked made every session fail to start
    /// with `invalid reference: master`.
    #[test]
    fn a_stale_origin_head_loses_to_a_branch_that_exists() {
        let (_d, root) = repo_on("main");
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/master",
            ],
        )
        .unwrap();

        assert_eq!(default_branch(&root), "main");
    }

    #[test]
    fn diff_against_the_detected_default_just_works() {
        let (d, root) = repo_on("master");
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "master").unwrap();
        std::fs::write(s.worktree.join("new.rs"), "fn main() {}").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "work"]).unwrap();

        let out = s.diff(&root, &default_branch(&root)).unwrap();
        assert!(out.contains("new.rs"), "got: {out}");
    }

    /// Regression: sessions branched from whatever HEAD happened to be, so one
    /// created while you were on a feature branch — or a moment before a commit
    /// landed — started from the wrong place and its diff was meaningless.
    #[test]
    fn a_new_session_branches_from_the_named_base_not_from_head() {
        let (d, root) = repo_on("master");
        git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "trunk work"],
        )
        .unwrap();
        let trunk_tip = git(&root, &["rev-parse", "master"])
            .unwrap()
            .trim()
            .to_string();

        // wander off somewhere unrelated before creating the session
        git(&root, &["checkout", "-q", "-b", "feature"]).unwrap();
        git(&root, &["commit", "-q", "--allow-empty", "-m", "unrelated"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "master").unwrap();

        let started_at = git(&s.worktree, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(started_at, trunk_tip, "must start from master, not feature");
    }

    #[test]
    fn an_explicit_base_is_honoured() {
        let (d, root) = repo_on("master");
        git(&root, &["checkout", "-q", "-b", "feature"]).unwrap();
        git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "feature work"],
        )
        .unwrap();
        let tip = git(&root, &["rev-parse", "feature"])
            .unwrap()
            .trim()
            .to_string();
        git(&root, &["checkout", "-q", "master"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "feature").unwrap();
        assert_eq!(
            git(&s.worktree, &["rev-parse", "HEAD"]).unwrap().trim(),
            tip
        );
    }

    /// Resuming must never move a branch that already holds work — rebasing an
    /// agent's unreviewed commits out from under it would be unrecoverable.
    #[test]
    fn resuming_never_moves_an_existing_branch() {
        let (d, root) = repo_on("master");
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "master").unwrap();
        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();
        let agent_tip = git(&root, &["rev-parse", s.branch.as_deref().unwrap()])
            .unwrap()
            .trim()
            .to_string();

        git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "trunk moved on"],
        )
        .unwrap();
        s.remove(&root, "master").unwrap();
        s.ensure(&root, "master").unwrap();

        assert_eq!(
            git(&root, &["rev-parse", s.branch.as_deref().unwrap()])
                .unwrap()
                .trim(),
            agent_tip,
            "the agent's commit must survive"
        );
    }

    #[test]
    fn a_session_reports_how_far_behind_it_has_drifted() {
        let (d, root) = repo_on("master");
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "master").unwrap();
        assert_eq!(s.behind(&root, "master"), 0);

        for m in ["one", "two"] {
            git(&root, &["commit", "-q", "--allow-empty", "-m", m]).unwrap();
        }
        assert_eq!(s.behind(&root, "master"), 2);
    }

    // ── choosing a session ──────────────────────────────────────────────────

    fn worktrees(ids: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        for id in ids {
            std::fs::create_dir_all(wt.join(id)).unwrap();
        }
        (d, wt)
    }

    #[test]
    fn there_is_no_current_session_before_any_exist() {
        let (_d, wt) = worktrees(&[]);
        assert_eq!(current(&wt), None);
    }

    #[test]
    fn the_current_session_is_the_most_recent() {
        let (_d, wt) = worktrees(&["s01", "s02", "s03"]);
        assert_eq!(current(&wt).as_deref(), Some("s03"));
    }

    /// Regression: every bare launch called `next_id`, so `omh claude` twice
    /// produced two sessions and you could never reattach to the agent you left
    /// running — which makes dtach persistence pointless.
    #[test]
    fn a_bare_launch_resumes_rather_than_multiplying_sessions() {
        let (_d, wt) = worktrees(&["s01"]);
        assert_eq!(pick(&wt, None, false), "s01");
    }

    #[test]
    fn the_first_launch_creates_the_first_session() {
        let (_d, wt) = worktrees(&[]);
        assert_eq!(pick(&wt, None, false), "s01");
    }

    #[test]
    fn a_new_session_is_something_you_ask_for() {
        let (_d, wt) = worktrees(&["s01", "s02"]);
        assert_eq!(pick(&wt, None, true), "s03");
    }

    #[test]
    fn an_explicit_id_always_wins() {
        let (_d, wt) = worktrees(&["s01", "s02"]);
        assert_eq!(pick(&wt, Some("s01"), false), "s01");
        assert_eq!(pick(&wt, Some("s09"), true), "s09", "explicit beats --new");
    }

    /// Regression: git can lose a worktree's registration while the directory
    /// survives (a prune that raced, an admin dir removed by hand). `worktree
    /// remove` then refuses with "is not a working tree" and the session can
    /// never be removed — the mirror of the missing-directory case.
    #[test]
    fn a_worktree_git_has_forgotten_can_still_be_removed() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        // Commit first, so this still exercises the branch-survives path in
        // the degraded case rather than the drop-an-empty-branch path.
        std::fs::write(s.worktree.join("w.txt"), "x").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();

        // git forgets, the directory stays
        std::fs::remove_dir_all(root.join(".git/worktrees")).unwrap();
        assert!(s.worktree.exists());

        s.remove(&root, "main")
            .expect("must clean up what is actually there");
        assert!(!s.worktree.exists(), "the directory must be gone");
        assert!(s.branch_exists(&root), "and the branch still kept");
    }

    #[test]
    fn removing_a_session_that_was_never_created_is_not_an_error() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s09".into());
        s.remove(&root, "main")
            .expect("nothing to do is not a failure");
    }

    // ── session ids are path components ─────────────────────────────────────

    #[test]
    fn ordinary_session_ids_are_accepted() {
        for id in ["s01", "doctor", "auth", "my-session"] {
            validate_id(id).unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    /// `-s` is user input and `remove` deletes the directory it names.
    #[test]
    fn a_session_id_cannot_escape_the_worktree_directory() {
        for id in ["..", "../..", "a/b", "/etc", ""] {
            assert!(validate_id(id).is_err(), "`{id}` must be rejected");
        }
    }

    // ── scratch sessions ────────────────────────────────────────────────────

    /// Regression: `omh auth` and `omh doctor` each left an `omh/auth` and
    /// `omh/doctor` branch behind, because `rm` keeps branches by design —
    /// a rule that is right for your work and wrong for a login.
    #[test]
    fn a_scratch_session_creates_no_branch() {
        let (d, root) = repo();
        let s = Session::scratch(d.path().join("scratch/auth"), "auth".into());
        s.ensure(&root, "main").unwrap();

        assert!(s.worktree.is_dir(), "it still needs a writable /work");
        assert_eq!(
            git(&root, &["branch", "--list", "omh/auth"])
                .unwrap()
                .trim(),
            "",
            "no branch may be created"
        );
    }

    #[test]
    fn removing_a_scratch_session_leaves_nothing_behind() {
        let (d, root) = repo();
        let s = Session::scratch(d.path().join("scratch/doctor"), "doctor".into());
        s.ensure(&root, "main").unwrap();
        s.remove(&root, "main").unwrap();

        assert!(!s.worktree.exists());
        assert_eq!(
            git(&root, &["branch", "--list", "omh/*"]).unwrap().trim(),
            ""
        );
    }

    /// A scratch directory must not live among the worktrees, or `omh s ls`
    /// lists a login as if it were a session you could resume.
    #[test]
    fn scratch_sessions_are_not_listed_as_sessions() {
        let (d, root) = repo();
        let wt = d.path().join("wt");
        Session::new(&wt, "s01".into())
            .ensure(&root, "main")
            .unwrap();
        Session::scratch(d.path().join("scratch/auth"), "auth".into())
            .ensure(&root, "main")
            .unwrap();

        assert_eq!(list(&wt), ["s01"]);
    }

    #[test]
    fn a_real_session_still_gets_its_branch() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        assert_eq!(s.branch.as_deref(), Some("omh/s01"));
        assert!(s.branch_exists(&root));
    }
}
