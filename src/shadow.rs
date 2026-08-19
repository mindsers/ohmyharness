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
//! remotes, and no object from the checkout — so an agent reading its own
//! history learns nothing about yours, and there is no `main` here to move.
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
    /// Idempotent: relaunching into a running session must not reset the
    /// agent's checkpoints, so an existing gitdir is left exactly as it is.
    pub fn ensure(&self, worktree: &Path, excluded: &[String]) -> Result<()> {
        if self.gitdir.exists() {
            return Ok(());
        }
        let parent = self
            .gitdir
            .parent()
            .context("a shadow gitdir needs somewhere to live")?;
        std::fs::create_dir_all(parent)?;

        // `init --bare` because the worktree is supplied per-command and is not
        // this directory. A non-bare init would put a `.git` *inside* the
        // gitdir and leave `core.bare` disagreeing with how it is used.
        // `init` takes the directory as an argument rather than through
        // `--git-dir`: it is the one command run before the repository exists,
        // and git rejects `--work-tree` from a wrapper with nothing to wrap.
        let made = Command::new("git")
            .args(["init", "-q", "--bare", "-b", &self.branch])
            .arg(&self.gitdir)
            .output()
            .context("creating the sandbox's repository")?;
        anyhow::ensure!(
            made.status.success(),
            "git init: {}",
            String::from_utf8_lossy(&made.stderr).trim()
        );
        // Not `--bare` as the repository *behaves*: it has a worktree, it is
        // just one git is told about rather than one it sits in.
        git(&self.gitdir, worktree, &["config", "core.bare", "false"])?;
        git(
            &self.gitdir,
            worktree,
            &["config", "core.worktree", &worktree.to_string_lossy()],
        )?;

        // Inside the gitdir, so nothing omh does appears in the user's tree —
        // the exclude file `carry` writes lives in the *worktree's* git dir and
        // is a different mechanism for a different repository.
        self.write_exclude(excluded)?;
        self.write_pre_push()?;

        // Everything the worktree holds except what was just excluded. `add -A`
        // rather than a path list: the seed has to be the tree the session
        // actually starts from, or every later diff is against a fiction.
        git(&self.gitdir, worktree, &["add", "-A", "."])?;
        git(
            &self.gitdir,
            worktree,
            &[
                "-c",
                "user.name=omh",
                "-c",
                "user.email=omh@localhost",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                SEED_MESSAGE,
            ],
        )?;

        let seed = git(&self.gitdir, worktree, &["rev-parse", "HEAD"])?;
        std::fs::write(&self.seed_record, seed.trim())?;
        Ok(())
    }

    /// Keep the agent's own `git status` clean at launch.
    ///
    /// Advisory, and known to be: `git add -f` walks straight through it. It is
    /// here so an honest agent is not shown a secret to commit, not to stop a
    /// determined one — what stops the secret reaching the branch is the check
    /// on the host when work crosses back.
    fn write_exclude(&self, excluded: &[String]) -> Result<()> {
        let info = self.gitdir.join("info");
        std::fs::create_dir_all(&info)?;
        // The carried files, and omh's own staged rules — the placeholder a
        // bind mount needs its destination to be is not the agent's work
        // either, and `commit` unstages the same list for the same reason.
        let names = excluded
            .iter()
            .map(String::as_str)
            .chain(crate::carry::STAGED_RULES);
        let body: String = names.map(|n| format!("{n}\n")).collect();
        std::fs::write(info.join("exclude"), body)?;
        Ok(())
    }

    /// The one wall left standing, and git's own hook rather than a pattern
    /// over the command line — git knows what a push is, and omh's pattern for
    /// matching `git` at all shipped broken once by missing the multi-line
    /// scripts agents most often emit.
    ///
    /// Worth knowing when it actually fires, because it is not when you would
    /// guess. Measured in a container on 2026-08-19:
    ///
    /// - No remote configured — the shipped state — and git fails first, on
    ///   `no upstream branch`, before any hook runs. Safe, but the message is
    ///   git's and it suggests `--set-upstream`, which fails again.
    /// - The agent adds a remote it can genuinely reach, which is the case that
    ///   matters and the only one where work could leave the machine. The hook
    ///   fires ahead of the transfer and the remote stays empty.
    ///
    /// So this is not what makes a push impossible — having no remote is. It is
    /// what makes the refusal say something true when the agent has built
    /// itself one.
    fn write_pre_push(&self) -> Result<()> {
        let hooks = self.gitdir.join("hooks");
        std::fs::create_dir_all(&hooks)?;
        let hook = hooks.join("pre-push");
        std::fs::write(&hook, format!("#!/bin/sh\necho '{NO_PUSH}' >&2\nexit 1\n"))?;
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
     This repository is the sandbox's own. It is not the branch anyone reviews: \
     the person you are working with sees your work with `omh s diff` on the \
     host, and decides at commit time whether to keep these commits as they are \
     or squash them under a message of their own. Either way they read what you \
     wrote here, so write messages worth keeping.\n\n\
     There is nothing to push and no remote to push to.";

/// Why a push cannot work, said where the agent is trying to push.
const NO_PUSH: &str =
    "omh: nothing to push from here. This repository is the sandbox's own and has no remote — \
     your work reaches the outside through the host, where `omh s commit` puts it on the branch \
     and `omh s push` sends it. Say that rather than trying to push yourself.";

fn git(gitdir: &Path, worktree: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
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

        let wt = dir.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
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
