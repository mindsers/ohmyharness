//! Sessions are git worktrees on their own branch.
//!
//! This is the part that makes the sandbox real. The container protects your
//! *host*; the worktree protects your *repo*. Your working tree is never
//! mounted, so an agent cannot touch uncommitted work or `main` — review is a
//! plain `git diff`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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
        Self { branch: None, worktree: dir, id }
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
        git(repo, &args)
            .with_context(|| format!("creating worktree for session {}", self.id))?;
        Ok(())
    }

    fn branch_exists(&self, repo: &Path) -> bool {
        let Some(branch) = &self.branch else { return false };
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

    pub fn remove(&self, repo: &Path) -> Result<()> {
        if git(repo, &["worktree", "remove", "--force", &self.worktree.to_string_lossy()]).is_ok() {
            // The branch outlives the worktree on purpose: removing a session
            // must not be able to destroy work that was never reviewed.
            return Ok(());
        }

        // git can lose the registration while the directory survives, and then
        // refuses with "is not a working tree" — leaving a session that can
        // never be removed. Clean up whatever is actually on disk.
        if self.worktree.exists() {
            std::fs::remove_dir_all(&self.worktree)
                .with_context(|| format!("removing {}", self.worktree.display()))?;
        }
        let _ = git(repo, &["worktree", "prune"]);
        Ok(())
    }

    pub fn diff(&self, repo: &Path, base: &str) -> Result<String> {
        let branch = self.branch.as_deref().context("a scratch session has no branch")?;
        git(repo, &["diff", "--stat", &format!("{base}...{branch}")])
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
    // What the remote says is authoritative when there is one.
    if let Ok(head) = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(name) = head.trim().strip_prefix("origin/") {
            if !name.is_empty() {
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
        anyhow::bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
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

        s.remove(&root).unwrap();
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

        s.ensure(&root, "main").expect("must recover rather than fail forever");
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

        assert!(keep.worktree.join(".git").exists(), "untouched session survived");
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
        git(&root, &["commit", "-q", "--allow-empty", "-m", "trunk work"]).unwrap();
        let trunk_tip = git(&root, &["rev-parse", "master"]).unwrap().trim().to_string();

        // wander off somewhere unrelated before creating the session
        git(&root, &["checkout", "-q", "-b", "feature"]).unwrap();
        git(&root, &["commit", "-q", "--allow-empty", "-m", "unrelated"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "master").unwrap();

        let started_at = git(&s.worktree, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        assert_eq!(started_at, trunk_tip, "must start from master, not feature");
    }

    #[test]
    fn an_explicit_base_is_honoured() {
        let (d, root) = repo_on("master");
        git(&root, &["checkout", "-q", "-b", "feature"]).unwrap();
        git(&root, &["commit", "-q", "--allow-empty", "-m", "feature work"]).unwrap();
        let tip = git(&root, &["rev-parse", "feature"]).unwrap().trim().to_string();
        git(&root, &["checkout", "-q", "master"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "feature").unwrap();
        assert_eq!(git(&s.worktree, &["rev-parse", "HEAD"]).unwrap().trim(), tip);
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
        let agent_tip = git(&root, &["rev-parse", s.branch.as_deref().unwrap()]).unwrap().trim().to_string();

        git(&root, &["commit", "-q", "--allow-empty", "-m", "trunk moved on"]).unwrap();
        s.remove(&root).unwrap();
        s.ensure(&root, "master").unwrap();

        assert_eq!(
            git(&root, &["rev-parse", s.branch.as_deref().unwrap()]).unwrap().trim(),
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

        // git forgets, the directory stays
        std::fs::remove_dir_all(root.join(".git/worktrees")).unwrap();
        assert!(s.worktree.exists());

        s.remove(&root).expect("must clean up what is actually there");
        assert!(!s.worktree.exists(), "the directory must be gone");
        assert!(s.branch_exists(&root), "and the branch still kept");
    }

    #[test]
    fn removing_a_session_that_was_never_created_is_not_an_error() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s09".into());
        s.remove(&root).expect("nothing to do is not a failure");
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
            git(&root, &["branch", "--list", "omh/auth"]).unwrap().trim(),
            "",
            "no branch may be created"
        );
    }

    #[test]
    fn removing_a_scratch_session_leaves_nothing_behind() {
        let (d, root) = repo();
        let s = Session::scratch(d.path().join("scratch/doctor"), "doctor".into());
        s.ensure(&root, "main").unwrap();
        s.remove(&root).unwrap();

        assert!(!s.worktree.exists());
        assert_eq!(git(&root, &["branch", "--list", "omh/*"]).unwrap().trim(), "");
    }

    /// A scratch directory must not live among the worktrees, or `omh s ls`
    /// lists a login as if it were a session you could resume.
    #[test]
    fn scratch_sessions_are_not_listed_as_sessions() {
        let (d, root) = repo();
        let wt = d.path().join("wt");
        Session::new(&wt, "s01".into()).ensure(&root, "main").unwrap();
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
