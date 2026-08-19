//! The repository the sandbox is allowed to have.
//!
//! A session's worktree carries a `.git` *file* pointing at an admin directory
//! inside the user's checkout, and omh never mounts that checkout — so inside
//! the sandbox the pointer leads nowhere and every git command fails. The agent
//! loses `status`, `diff`, `log`, `stash` and `reset --hard`, and the editor
//! attached over SSH loses its source control panel with them.
//!
//! What it gets instead is a repository of its own: a gitdir omh keeps outside
//! the worktree, seeded with a single commit of the tree the session started
//! from, mounted into the container and pointed at by a `.git` file that exists
//! only inside it. The host's own pointer is never written, so `omh s diff`,
//! `omh s commit` and `omh s push` are untouched.
//!
//! What makes it safe is what is *not* in it. One commit, one branch, no
//! remotes, and no *commit* from the checkout — so an agent reading its own
//! history learns nothing about yours, and there is no `main` here to move.
//!
//! Not "no object": a file whose content matches yours hashes to the same blob,
//! so shared blobs are unavoidable and mean nothing. History is the thing that
//! must not cross, and the guard is written against a commit for that reason.
//!
//! The governing rule for everything downstream: **never trust what the sandbox
//! asserts.** The agent can write in this gitdir, so it can set `user.email`,
//! delete a tag, or force-add a file the exclude list names. Nothing read from
//! here is taken on trust — identity and carried-file policy are enforced on
//! the host, at the moment work crosses back.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the gitdir is mounted inside the container.
///
/// Outside `/work` on purpose. A gitdir *inside* the worktree would be carried
/// into `omh s commit`'s `git add -A` and land the sandbox's entire scratch
/// history in the user's branch.
pub const GUEST_GITDIR: &str = "/omh/shadow";

/// The pointer file mounted at `/work/.git`, naming the gitdir above.
///
/// This is what makes git work inside the sandbox and nowhere else: it shadows
/// the worktree's real pointer for the container's view only, so the host's
/// file — which names an admin directory in the user's checkout — is never
/// written and every host-side git command is unaffected.
pub fn pointer_file() -> String {
    format!("gitdir: {GUEST_GITDIR}\n")
}

/// The sandbox's own repository for one session.
pub struct Shadow {
    /// The gitdir, mounted into the container. Agent-writable.
    pub gitdir: PathBuf,
    /// Where the seed commit is recorded. A sibling of the gitdir rather than
    /// anything inside it: the gitdir is mounted, so a tag or a config entry
    /// there is something the agent can delete, and losing the seed is losing
    /// the only fixed point a harvest can replay from.
    pub seed_record: PathBuf,
    /// Named for the session so the user can tell which sandbox an editor
    /// window is showing, and `-scratch` because that is what it is: the
    /// history the user curates before any of it becomes the branch's.
    pub branch: String,
}

impl Shadow {
    pub fn new(shadow_dir: &Path, session_id: &str) -> Self {
        Self {
            gitdir: shadow_dir.join(format!("{session_id}.git")),
            seed_record: shadow_dir.join(format!("{session_id}.seed")),
            branch: format!("{session_id}-scratch"),
        }
    }

    /// Create the repository and seed it with the worktree as it stands.
    ///
    /// Idempotent for a *finished* shadow: relaunching into a running session
    /// must not reset the agent's checkpoints, so one that has a seed recorded
    /// is left exactly as it is. One without is the wreckage of a launch that
    /// died partway through, and is rebuilt rather than adopted — see the two
    /// notes in the body for why that cannot lose work.
    pub fn ensure(&self, worktree: &Path, excluded: &[String]) -> Result<()> {
        // Both, not just the directory. Seven subprocess calls stand between
        // `git init` and a usable repository, and a launch killed anywhere in
        // the middle leaves a directory that looks finished. The record is
        // written last and only after the rename below, so its presence is what
        // actually means "this one got there".
        if self.gitdir.exists() && self.seed_record.exists() {
            return Ok(());
        }
        let parent = self
            .gitdir
            .parent()
            .context("a shadow gitdir needs somewhere to live")?;
        std::fs::create_dir_all(parent)?;

        // Built beside the real path and moved onto it, so the directory the
        // rest of omh looks for never exists in a half-made state. Same
        // filesystem by construction — both are children of `parent` — which is
        // what makes the move atomic rather than a copy.
        //
        // Anything left over from an attempt that did not finish is removed
        // first, and that is safe precisely because it did not finish: without
        // a seed record it was never mounted, so there is no agent work in it
        // to lose.
        let building = parent.join(format!(
            "{}.building",
            self.gitdir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&building);
        let _ = std::fs::remove_dir_all(&self.gitdir);

        // `init --bare` because the worktree is supplied per-command and is not
        // this directory: the positional non-bare form puts a `.git` *inside*
        // the directory it is given, one level deeper than anything here
        // expects.
        //
        // Positional rather than through the `git()` wrapper because `--bare`
        // and `--work-tree` cannot be combined — git refuses the pair, and
        // confusingly blames the missing `--git-dir` it was in fact given. Not,
        // as this once said, because the repository does not exist yet: `init`
        // through the wrapper is fine, and accepts a `--work-tree` naming a
        // path that is not there at all.
        let made = Command::new("git")
            // `--template=` empty, because `git init` otherwise copies the
            // user's `init.templateDir` into this gitdir — and this gitdir is
            // mounted into the container. That is a host-to-sandbox copy of
            // arbitrary host files, in the one module whose thesis is that
            // safety comes from what is *not* in here; template hooks would
            // then also run inside the sandbox.
            .args(["init", "-q", "--bare", "--template=", "-b", &self.branch])
            .arg(&building)
            .output()
            .context("creating the sandbox's repository")?;
        anyhow::ensure!(
            made.status.success(),
            "git init: {}",
            String::from_utf8_lossy(&made.stderr).trim()
        );
        // Not `--bare` as the repository *behaves*: it has a worktree, it is
        // just one git is told about rather than one it sits in.
        git(&building, worktree, &["config", "core.bare", "false"])?;

        // And deliberately no `core.worktree`. It looks like the missing half
        // of the line above and it is the opposite: this gitdir is written on
        // the host and read inside a container, so a worktree path recorded
        // here is a host path that does not exist there — and `core.worktree`
        // outranks the directory the `.git` pointer sits in. Setting it made
        // every command in the sandbox fail with `fatal: Invalid path` — the
        // whole list this feature exists to restore.
        //
        // Nothing needs it. Host-side callers pass `--work-tree` themselves,
        // and in the container the pointer file's own directory is the answer,
        // which is `/work` and correct. Guarded by
        // `the_pointer_file_alone_resolves_to_the_worktree_it_sits_in`, which
        // resolves the way the container does rather than the way the tests
        // used to — that blind spot is why this shipped.

        // Inside the gitdir, so nothing omh does appears in the user's tree —
        // the exclude file `carry` writes lives in the *worktree's* git dir and
        // is a different mechanism for a different repository.
        Self::write_exclude(&building, excluded)?;
        Self::write_pre_push(&building)?;

        // And deliberately no `core.hooksPath`, for the same reason as
        // `core.worktree` above and learned the same way — by writing it and
        // watching the container fail.
        //
        // A global `core.hooksPath` does send git looking elsewhere and would
        // leave the hook installed but never consulted. That is a real hazard
        // on the *host*, and it does not exist in the sandbox: the container
        // carries no global git config at all, so `$GIT_DIR/hooks` is where it
        // looks and the hook is right there. Pinning the value wrote a host
        // path into a config only the container reads, which pointed at nothing
        // and let a push through — verified by pushing to a reachable remote
        // and finding two commits on it.
        //
        // If a host-side reader is ever added, it passes `-c core.hooksPath=`
        // itself rather than recording anything here.

        // Everything the worktree holds except what was just excluded. `add -A`
        // rather than a path list: the seed has to be the tree the session
        // actually starts from, or every later diff is against a fiction.
        git(&building, worktree, &["add", "-A", "."])?;
        git(
            &building,
            worktree,
            &[
                "-c",
                "user.name=omh",
                "-c",
                "user.email=omh@localhost",
                // The user's global config governs this commit otherwise, and
                // two ordinary settings turn it into a launch that will not
                // start: `commit.gpgsign = true` fails with `gpg failed to
                // sign the data`, and a `core.hooksPath` pointing at a husky or
                // team hooks directory runs their `pre-commit` against omh's
                // seed. Neither is the user asking for anything — this is a
                // commit they never made, in a repository they cannot see.
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--no-verify",
                "--allow-empty",
                "-m",
                SEED_MESSAGE,
            ],
        )?;
        let seed = git(&building, worktree, &["rev-parse", "HEAD"])?;

        // The record before the rename, so the two cannot disagree in the
        // direction that matters. A crash between them leaves a seed naming a
        // gitdir that is not there yet, and the next launch rebuilds both; the
        // reverse — a gitdir with no seed — is the state the fast path above
        // would wave through.
        std::fs::write(&self.seed_record, seed.trim())?;
        std::fs::rename(&building, &self.gitdir)?;
        Ok(())
    }

    /// Remove the repository and the seed recorded for it.
    ///
    /// Best-effort on purpose: this runs while a session is being torn down,
    /// and a shadow that will not delete is not a reason to fail a removal the
    /// user asked for. What it must not do is leave the *seed* behind without
    /// the gitdir — a seed naming a repository that is gone is exactly the
    /// state `ensure`'s fast path reads as "already built" — so the record goes
    /// last, after the directory it describes.
    pub fn reap(&self) {
        let _ = std::fs::remove_dir_all(&self.gitdir);
        let _ = std::fs::remove_file(&self.seed_record);
    }

    /// Keep the agent's own `git status` clean at launch.
    ///
    /// Advisory, and known to be: `git add -f` walks straight through it. It is
    /// here so an honest agent is not shown a secret to commit, not to stop a
    /// determined one — what stops the secret reaching the branch is the check
    /// on the host when work crosses back.
    ///
    /// Takes the directory rather than reading `self.gitdir`, because it runs
    /// while the repository is still being built under another name.
    fn write_exclude(gitdir: &Path, excluded: &[String]) -> Result<()> {
        let info = gitdir.join("info");
        std::fs::create_dir_all(&info)?;
        // Just what the caller names. `container::plan` derives that from the
        // mounts it is about to make, which already covers omh's staged rules —
        // chaining `carry::STAGED_RULES` here as well only made the list
        // disagree with its own source when a capability changed.
        let body: String = excluded.iter().map(|n| format!("{n}\n")).collect();
        std::fs::write(info.join("exclude"), body)?;
        Ok(())
    }

    /// The one wall left standing, and git's own hook rather than a pattern
    /// over the command line — git knows what a push is, and omh's pattern for
    /// matching `git` at all shipped broken once by missing the multi-line
    /// scripts agents most often emit.
    ///
    /// Worth knowing when it actually fires, because it is not when you would
    /// guess, and the sequence matters. Measured against git 2.55.0:
    ///
    /// - **No remote — the shipped state.** `git push` dies on `fatal: No
    ///   configured push destination`, before any hook runs, and git's advice
    ///   is `git remote add <name> <url>`. So the thing that ends the first
    ///   attempt also hands the agent the recipe for the second.
    /// - **A remote the agent added.** With one configured but no upstream, git
    ///   asks for `--set-upstream`; supply it, or push by name, and the hook
    ///   fires ahead of the transfer. Verified against a reachable remote,
    ///   which stayed at zero commits.
    ///
    /// So this is not what makes a push impossible — having no remote is. It is
    /// what catches the agent after git has talked it into fixing that, which
    /// is the only route by which work could leave the machine.
    fn write_pre_push(gitdir: &Path) -> Result<()> {
        let hooks = gitdir.join("hooks");
        std::fs::create_dir_all(&hooks)?;
        let hook = hooks.join("pre-push");
        // A quoted heredoc, not `echo '…'`. The message is prose and prose has
        // apostrophes: `the sandbox's own` closed the single quote, and the
        // hook became a script that does not parse. It still exited non-zero,
        // so it still refused and a test asserting failure still passed — but
        // what the agent read was `unexpected EOF while looking for matching
        // \`'\`` with no mention of omh, which is the whole reason the hook
        // exists rather than letting git fail on its own.
        //
        // `<<'OMH'` quotes the delimiter, so nothing inside is interpolated and
        // no punctuation in the message can reach the shell.
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ncat >&2 <<'OMH'\n{NO_PUSH}\nOMH\nexit 1\n"),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }
}

/// The seed commit's message, which is a delivery surface and not a label.
///
/// Every `git log`, `git show` and editor timeline renders it, at the moment
/// the agent is working out what this repository is — which a rules section,
/// paid for once and then competing with everything after it, cannot reach.
pub const SEED_MESSAGE: &str = "The session starts here.\n\n\
     This repository is the sandbox's own, and it is not the branch anyone \
     reviews. Commit as often as you like — that is what it is for, and \
     `git reset --hard` back to a checkpoint is yours to use. What reaches the \
     person you are working with is the state of the files, which they read \
     with `omh s diff` and commit themselves on the host. Your commit messages \
     here stay here.\n\n\
     There is nothing to push and no remote to push to.";

/// Why a push cannot work, said where the agent is trying to push.
const NO_PUSH: &str =
    "omh: nothing to push from here. This repository is the sandbox's own and has no remote — \
     your work reaches the outside through the host, where `omh s commit` puts it on the branch \
     and `omh s push` sends it. Say that rather than trying to push yourself.";

fn git(gitdir: &Path, worktree: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        // Explicitly, because a child inherits omh's cwd and `add -A .` is
        // resolved against it: invoked from inside the session's worktree, git
        // computes a prefix and seeds only that subtree, silently leaving the
        // rest of the tree out of the commit every later diff is measured from.
        .current_dir(worktree)
        .arg("--git-dir")
        .arg(gitdir)
        .arg("--work-tree")
        .arg(worktree)
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

    /// A checkout with history, a worktree holding a session's starting tree,
    /// and a carried file the user never tracked.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("checkout");
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("f.txt"), "base\n").unwrap();
        run(&["add", "-A"]);
        run(&[
            "commit",
            "-q",
            "-m",
            "a commit only the host should know about",
        ]);

        // A real worktree, made the way omh makes one, because the `.git`
        // pointer it writes is the *only* route from here back to the
        // checkout's object store. Built as a plain directory instead, the
        // isolation test could not reach the thing it exists to forbid: a leak
        // that resolves the checkout through that pointer and writes
        // `objects/info/alternates` passed, because there was no pointer to
        // resolve. The guard was correct and the fixture made it decorative.
        let wt = dir.path().join("wt");
        run(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "omh/s01",
            wt.to_str().unwrap(),
        ]);
        std::fs::write(wt.join("f.txt"), "base\n").unwrap();
        std::fs::write(wt.join(".env"), "SECRET=1\n").unwrap();

        let shadow_dir = dir.path().join("shadow");
        std::fs::create_dir_all(&shadow_dir).unwrap();
        (dir, wt, shadow_dir)
    }

    fn head_of(repo: &Path) -> String {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The isolation the sandbox is *for*, asserted as an invariant rather than
    /// a mount list: whatever else the shadow gains, the checkout's commits are
    /// never reachable from it. A shadow seeded by cloning, or by pointing at
    /// the real object store to save disk, would pass every other test here and
    /// hand the agent every branch you have.
    #[test]
    fn the_shadow_holds_no_commit_from_the_checkout() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        let host_commit = head_of(&checkout);
        let reachable = Command::new("git")
            .arg("--git-dir")
            .arg(&s.gitdir)
            .args(["cat-file", "-e", &host_commit])
            .output()
            .unwrap();
        assert!(
            !reachable.status.success(),
            "the host's history must not be reachable from the sandbox's repo"
        );

        let count = git(&s.gitdir, &wt, &["rev-list", "--all", "--count"]).unwrap();
        assert_eq!(count.trim(), "1", "the seed, and nothing else");
        let remotes = git(&s.gitdir, &wt, &["remote"]).unwrap();
        assert!(remotes.trim().is_empty(), "nowhere to push: {remotes:?}");
    }

    /// The seed is the only fixed point a harvest can replay from, and the
    /// agent can write everywhere the container can reach. Recorded as a tag it
    /// went away with one `git tag -d`; recorded in the gitdir's config the
    /// agent rewrites it. So it lives on the host, outside the mount.
    #[test]
    fn the_seed_is_recorded_where_the_sandbox_cannot_reach_it() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        let seed = std::fs::read_to_string(&s.seed_record).unwrap();
        assert!(!seed.is_empty(), "a seed has to be recorded");
        assert!(
            !s.seed_record.starts_with(&s.gitdir),
            "the record must not sit inside the directory the agent can write"
        );

        // the agent does its worst inside its own repo
        let _ = git(&s.gitdir, &wt, &["tag", "-d", "session-start"]);
        assert_eq!(
            std::fs::read_to_string(&s.seed_record).unwrap(),
            seed,
            "the seed survives the sandbox"
        );
    }

    /// `carry_in` puts files the repo does not track into the worktree — the
    /// user's `.env` among them. Left visible, the agent's first `git status`
    /// offers to add a secret, and `git add -A` takes it.
    #[test]
    fn a_carried_file_is_not_something_the_shadow_tracks() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();

        let tracked = git(&s.gitdir, &wt, &["ls-files"]).unwrap();
        assert!(
            !tracked.contains(".env"),
            "carried, not committed: {tracked}"
        );
        let status = git(&s.gitdir, &wt, &["status", "--porcelain"]).unwrap();
        assert!(
            status.trim().is_empty(),
            "a session opens on a clean tree or the agent starts by tidying: {status:?}"
        );
    }

    /// The one thing that stays walled. Everything else about this repo is
    /// meant to work, so the agent has no standing reason to think a push
    /// would — and `git push` reaching a real remote is the one mistake that
    /// leaves the machine.
    ///
    /// git's own hook rather than a shell pattern over the command line: git
    /// knows what a push is, and the pattern omh used to match `git` at all
    /// shipped broken once already by missing multi-line scripts.
    #[test]
    fn the_shadow_refuses_to_push() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        let hook = s.gitdir.join("hooks/pre-push");
        assert!(hook.exists(), "a pre-push hook has to be installed");

        let out = Command::new("sh").arg(&hook).output().unwrap();
        assert!(!out.status.success(), "the hook must refuse");

        // Asserting the *message*, not the exit code. A non-zero exit is what a
        // shell syntax error gives you too — and that is exactly what this
        // shipped: `NO_PUSH` contains an apostrophe, the script wrapped it in
        // single quotes, and the hook refused only by failing to parse. The
        // agent got `unexpected EOF while looking for matching \`'\`` and no
        // mention of omh at all, while a test asserting failure stayed green.
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            said.contains("omh s commit"),
            "a refusal has to say where work actually leaves: {said}"
        );
        assert!(
            !said.contains("syntax error") && !said.contains("unexpected"),
            "the hook must run, not merely fail to parse: {said}"
        );

        // git skips a hook it cannot execute, with a *hint* and exit 0 — the
        // push then succeeds. A mode this test does not check is a wall that is
        // not there.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&hook).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "git ignores a non-executable hook");
        }
    }

    /// Every other test here reaches the repository the way the *host* does,
    /// through the private `git()` helper, which passes `--work-tree`. The
    /// container never does that: it finds the repository through the `.git`
    /// pointer omh mounts onto `/work`, and takes the worktree from wherever
    /// that pointer sits.
    ///
    /// The difference is not academic. A `core.worktree` written on the host
    /// records a host path, outranks the pointer's own directory, and made
    /// every git command in the sandbox fail with `fatal: Invalid path` — while
    /// the whole suite stayed green, because `--work-tree` overrode the bad
    /// value on every call a test made. This resolves with no `--work-tree` at
    /// all, which is the only way that class of mistake is visible from here.
    #[test]
    fn the_pointer_file_alone_resolves_to_the_worktree_it_sits_in() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // the container's view: discovery through the pointer, nothing else
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", s.gitdir.display())).unwrap();
        let out = Command::new("git")
            .current_dir(&wt)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .unwrap();

        assert!(
            out.status.success(),
            "git must work through the pointer alone: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            std::fs::canonicalize(&wt).unwrap().to_string_lossy(),
            "the worktree is where the pointer sits, not somewhere recorded on the host"
        );
    }

    /// `ensure` makes a repository out of seven subprocess calls, and a machine
    /// that dies between the first and the last leaves a directory that looks
    /// exactly like a finished one. Read as "seeded" because it exists, that
    /// shadow opens the session on a repository with no seed and no exclude
    /// list — the agent's first `git status` offers it the carried `.env`.
    ///
    /// So the directory only appears once it is complete, and a leftover from
    /// an attempt that did not get there is not mistaken for the real thing.
    #[test]
    fn a_half_built_shadow_is_never_mistaken_for_a_finished_one() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");

        // what a launch killed partway through leaves behind
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&s.gitdir)
            .output()
            .unwrap();
        assert!(!s.seed_record.exists(), "it never got as far as the seed");

        s.ensure(&wt, &[".env".to_string()]).unwrap();

        assert!(s.seed_record.exists(), "the seed has to be recorded");
        assert_eq!(
            git(&s.gitdir, &wt, &["rev-list", "--all", "--count"])
                .unwrap()
                .trim(),
            "1",
            "a finished shadow has its seed commit"
        );
        let status = git(&s.gitdir, &wt, &["status", "--porcelain"]).unwrap();
        assert!(!status.contains(".env"), "and its exclude list: {status:?}");
    }

    /// Relaunching into a running session is ordinary — `omh claude` twice, an
    /// editor attaching alongside a terminal. Re-seeding there would throw away
    /// every checkpoint the agent had made, which is the one thing this whole
    /// repository exists to keep.
    #[test]
    fn seeding_twice_keeps_the_work_the_agent_already_did() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A"]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-q", "-m", "a checkpoint"]).unwrap();
        let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

        s.ensure(&wt, &[]).unwrap();

        assert_eq!(
            git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
            checkpoint,
            "a relaunch must not discard the agent's checkpoints"
        );
    }
}
