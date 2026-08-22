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

/// Where the `pre-push` hook lands inside the container.
///
/// Mounted read-only rather than written into the gitdir, so the agent cannot
/// take it away. Measured: against a read-only mount `rm` gives `Resource
/// busy`, and overwriting or `chmod -x` give `Read-only file system` — the
/// three ways a file-owning agent silently disarms a hook, all closed.
///
/// It is still not a wall. `git push --no-verify` and
/// `git -c core.hooksPath=… push` never consult the file at all, and both were
/// measured pushing to a reachable remote with this mount in place. What the
/// mount buys is that the hook cannot *quietly* stop being there — a bypass now
/// takes a deliberate flag, which is a different thing from a missing file.
pub const GUEST_PRE_PUSH: &str = "/omh/shadow/hooks/pre-push";

/// Where the sandbox's own config lands inside the container.
///
/// Mounted read-only over the copy `ensure` wrote, for the reason the push hook
/// is: the gitdir has to be writable because the agent commits into it, so
/// every file in it is the agent's to change — and this one decides what `git`
/// does. `NEUTRALISED` answers that for calls omh makes on the host; nothing
/// answers it for the agent's own git inside the container.
///
/// Measured inside a real container, because a host-side stand-in got the
/// answer right and the reason wrong. `commit`, `checkout -b` and
/// `reset --hard` never write this file and do not notice it. `git config` and
/// `git remote add` do, and meet:
///
/// ```text
/// error: could not write config file /omh/shadow/config: Resource busy
/// fatal: could not set 'remote.origin.url' to 'https://example.com/x.git'
/// ```
///
/// `Resource busy`, not `Read-only file system` — git replaces this file by
/// renaming a lock over it, and what refuses is the rename onto a mount point
/// rather than the read-only flag. The host stand-in used before this was
/// measured says `Operation not permitted`, and for a different reason again:
/// an immutable flag refusing the rename, not a mount. Same outcome, three
/// different mechanisms, which is exactly how a stand-in gets quoted as if it
/// were the thing.
///
/// That second one is the interesting one. `git push` fails for want of a
/// remote and git's own error suggests `git remote add` — which now fails too,
/// so the route git talks the agent into is closed rather than signposted. The
/// `pre-push` hook keeps its job on the other route: `git push <url> <ref>`
/// needs nothing in config and still meets it.
pub const GUEST_CONFIG: &str = "/omh/shadow/config";

/// The hook's body, so the launcher can stage the same bytes it mounts.
pub fn pre_push_hook() -> String {
    // A quoted heredoc, not `echo '…'`. The message is prose and prose has
    // apostrophes: `the sandbox's own` closed the single quote, and the hook
    // became a script that does not parse. It still exited non-zero, so it
    // still refused and a test asserting failure still passed — but what the
    // agent read was `unexpected EOF while looking for matching \`'\`` with no
    // mention of omh, which is the whole reason it exists rather than letting
    // git fail on its own.
    format!("#!/bin/sh\ncat >&2 <<'OMH'\n{NO_PUSH}\nOMH\nexit 1\n")
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
    /// What the last harvest took, in the sandbox's own commit ids.
    ///
    /// Beside the seed and for the same reason: the gitdir is mounted, so
    /// anything recorded inside it is the agent's to delete, and a replay point
    /// that can be forged is a branch that can be handed work twice.
    ///
    /// Absent until the first harvest, which is why it is an `Option` rather
    /// than a second seed — a session that has never landed anything replays
    /// from the seed, and that is not a missing record, it is the first round.
    pub landed_record: PathBuf,
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
            landed_record: shadow_dir.join(format!("{session_id}.landed")),
            branch: format!("{session_id}-scratch"),
        }
    }

    /// Create the repository and seed it with the worktree as it stands.
    ///
    /// Idempotent for a *finished* shadow: relaunching into a running session
    /// must not reset the agent's checkpoints, so one that has a seed recorded
    /// keeps every commit, ref and index it had. The one thing it does not keep
    /// is the exclude list, which is derived from mounts that move between
    /// launches — see the comment on the fast path. One without is the wreckage of a launch that
    /// died partway through, and is rebuilt rather than adopted — see the two
    /// notes in the body for why that cannot lose work.
    pub fn ensure(&self, worktree: &Path, excluded: &[String]) -> Result<()> {
        // Both, not just the directory. Seven subprocess calls stand between
        // `git init` and a usable repository, and a launch killed anywhere in
        // the middle leaves a directory that looks finished. The record is
        // written last and only after the rename below, so its presence is what
        // actually means "this one got there".
        if self.gitdir.exists() && self.seed_record.exists() {
            // The repository is left exactly as it is — that is what makes
            // relaunching safe — but the exclude list is not part of "as it
            // is". `container::plan` builds it from the `carry_in` policy and
            // then from the mounts it is about to make, and the second half
            // moves: switch harness, or switch a capability on, and a document
            // lands inside `/work` that this repository has never heard of, so
            // `git add -A` sweeps omh's rendered file — credentials and all —
            // into a history `--keep` replays onto the branch.
            //
            // Written here rather than through a `refresh` the caller has to
            // remember. The tests would catch `plan` forgetting one today; what
            // they cannot catch is the *next* caller of `ensure`, which would
            // be a path they do not cover. `ensure` already takes the list, so
            // the only question was whether it believed it on the second
            // launch.
            //
            // Wholesale, so anything the agent added to this file for its own
            // housekeeping goes with it. That is the trade: merging would keep
            // those, and would also keep an entry omh has since dropped —
            // leaving a path silently untracked in the one repository whose
            // job is to show the agent its own work.
            Self::write_exclude(&self.gitdir, excluded)?;
            Self::write_config(&self.gitdir)?;
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

        // An identity of its own, because git will not commit without one and
        // the container has no global config to supply it. Without this the
        // agent's first checkpoint dies on `Author identity unknown` — the one
        // thing this repository exists to let it do — on any machine nobody has
        // configured, which is every container and every CI runner.
        //
        // It costs nothing in trust: the agent can rewrite this, and a harvest
        // stamps authorship on the host anyway, per the module's own rule about
        // not believing what the sandbox says about itself. This is here so an
        // unconfigured machine works, not so the name can be relied on.
        Self::write_config(&building)?;

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
                &seed_message(),
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

    /// The commit the session started from, read from the host-side record.
    ///
    /// Not from the shadow itself, at any price. A tag is one `tag -d` away and
    /// the root commit stops being the seed the moment an agent runs
    /// `checkout --orphan` — both leave a harvest replaying from the wrong
    /// point, which is worse than refusing to replay at all.
    pub fn seed(&self) -> Result<String> {
        let seed = std::fs::read_to_string(&self.seed_record).with_context(|| {
            format!(
                "no seed recorded for this session at {}",
                self.seed_record.display()
            )
        })?;
        Ok(seed.trim().to_string())
    }

    /// What the last harvest took, if there has been one.
    ///
    /// A *sandbox-side* commit, deliberately: the range a harvest replays is
    /// computed in the fetched history's ids, and the ids the branch ended up
    /// with are different commits — `replant` rewrites them onto a new parent
    /// and `stamp` rewrites them again. Recording what landed on the branch
    /// would be recording something this range can never mention.
    ///
    /// **Absent and unreadable are different answers.** Only "not there" means
    /// *never harvested*. Every other failure is a record that exists and could
    /// not be read, and reading that as `None` replays from the seed — offering
    /// the branch everything it already has, which is the defect this record
    /// exists to close, reached by a permissions error rather than by a second
    /// run.
    ///
    /// An earlier version collapsed the two and excused it by saying the
    /// ancestry check downstream would catch it. It does not: that check lives
    /// inside the arm where a record *was* read, so the one case being excused
    /// is the one case it never sees. `needles` in this file already states the
    /// rule this now follows — cannot tell must not spell the same as clean.
    pub fn landed(&self) -> Result<Option<String>> {
        let landed = match std::fs::read_to_string(&self.landed_record) {
            Ok(landed) => landed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "reading what {} last handed over, at {}",
                        self.branch,
                        self.landed_record.display()
                    )
                })
            }
        };
        let landed = landed.trim();
        // Empty is not absent either, and it is the likelier of the two: the
        // write that produces this record truncates before it writes, so a
        // process killed in that window leaves zero bytes where a commit id
        // was. Read as *never harvested* it would replay from the seed and skip
        // the ancestry check on the way past — the widest failure available,
        // reached by the narrowest accident.
        anyhow::ensure!(
            !landed.is_empty(),
            "{} is empty, which is what an interrupted write leaves behind. omh \
             cannot tell what it last handed over, and will not guess. Take the \
             files as they stand with `omh s commit -m`",
            self.landed_record.display()
        );
        Ok(Some(landed.to_string()))
    }

    /// Refuse to harvest from a repository whose history is not all reachable.
    ///
    /// Three states make a harvest succeed while quietly dropping commits, and
    /// all three are ordinary things for an agent to leave behind. Every one is
    /// readable from the gitdir on the host, without entering the sandbox.
    ///
    /// A silent partial harvest is the worst outcome available here: the user
    /// reviews what arrived, sees plausible work, and never learns what did not
    /// come. Refusing costs a command; the alternative costs the thing the
    /// feature exists to protect.
    pub fn preflight(&self, worktree: &Path) -> Result<()> {
        // Absent before anything else, because every check below asks git a
        // question and git's failure to answer is not the same fact as the
        // answer being bad. Without this, a session whose sandbox never ran
        // reported a *detached HEAD* — `symbolic-ref` failing on a directory
        // that is not there, read as "not on a branch".
        anyhow::ensure!(
            self.gitdir.exists(),
            "{} has no sandbox repository — nothing has run in it yet, so there \
             are no commits of its own to keep. `omh s commit -m` takes the \
             files as they are",
            worktree.display()
        );

        // Detached: `git checkout <sha>` to look at an old checkpoint, and the
        // commits after it are no longer reachable from HEAD. Measured: the
        // harvest reported success and left them behind.
        let attached = Command::new("git")
            .arg("--git-dir")
            .arg(&self.gitdir)
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        anyhow::ensure!(
            attached,
            "the sandbox's repository is on a detached HEAD, so a harvest would \
             take only what that commit can reach. Put it back on a branch \
             first:\n  git --git-dir={} checkout {}",
            self.gitdir.display(),
            self.branch
        );

        // Interrupted: a rebase or merge left halfway leaves HEAD pointing at
        // something that is not the work and not a mistake anyone made.
        for marker in ["rebase-merge", "rebase-apply", "MERGE_HEAD"] {
            anyhow::ensure!(
                !self.gitdir.join(marker).exists(),
                "the sandbox's repository has a {marker} in progress — finish or \
                 abort it before harvesting, or the harvest takes the half of it \
                 that happens to be reachable"
            );
        }

        // Stranded: anything reachable from a ref but not from HEAD. Catches
        // the branch an agent made and wandered off, which neither test above
        // sees.
        let stranded = git(
            &self.gitdir,
            worktree,
            &["rev-list", "--all", "--not", "HEAD"],
        )?;
        let stranded: Vec<&str> = stranded.lines().take(3).collect();
        anyhow::ensure!(
            stranded.is_empty(),
            "the sandbox's repository has commits no branch it is on can reach \
             ({}…) — they would be dropped in silence. Merge or delete them \
             first: git --git-dir={} log --all --not HEAD",
            stranded.join(", "),
            self.gitdir.display()
        );
        Ok(())
    }

    /// Everything the agent committed, replanted onto the session's branch.
    ///
    /// Six steps, and the order of the first two is the whole safety of it.
    ///
    /// **Fetch before replant.** The objects land in the user's own repository
    /// first, so a replant that conflicts leaves the branch untouched and the
    /// work reachable at the fetched ref. Measured: a conflicting replant fails
    /// loudly and loses nothing.
    ///
    /// **Refuse, never strip.** A carried file that reached a commit — by
    /// `git add -f`, by being copied under another name, or by having its
    /// contents pasted into source — is not something omh can quietly remove.
    /// Stripping the path leaves an empty commit with a misleading message and
    /// does nothing about the other two shapes; rewriting the agent's work to
    /// hide a secret is the user's call. So this stops and says which commit.
    ///
    /// Authorship is stamped **after** curation, in a pass of its own. Folding
    /// it into the interactive rebase as `--exec` would put the security step
    /// in the todo list the user is editing, where deleting a line deletes the
    /// guard.
    pub fn harvest(
        &self,
        repo: &Path,
        worktree: &Path,
        branch: &str,
        carried: &[String],
        curate: bool,
    ) -> Result<usize> {
        self.preflight(worktree)?;

        // Whatever the agent has not checkpointed yet is still its work, and a
        // harvest that drops it is the tail of the session gone. Measured: the
        // uncommitted remainder simply did not arrive.
        if !git(&self.gitdir, worktree, &["status", "--porcelain"])?
            .trim()
            .is_empty()
        {
            git(&self.gitdir, worktree, &["add", "-A", "."])?;
            git(
                &self.gitdir,
                worktree,
                &["commit", "-q", "--no-verify", "-m", "Work in progress"],
            )?;
        }

        let seed = self.seed()?;
        let scratch = format!("refs/omh/{}/harvest", self.branch);
        // `protocol.file.allow=always`, against `NEUTRALISED`'s blanket `never`.
        //
        // That default exists because the sandbox owns its gitdir and could
        // point a submodule or an alternate at anything on the host. Here the
        // path is omh's own, computed from `paths.shadows()` and never read
        // from anything the agent wrote — so the reason for the ban does not
        // apply, and without the override the fetch dies on `transport 'file'
        // not allowed`. Narrowed to this one call rather than dropped from the
        // list, because every other host-side read still wants it.
        git_in(
            repo,
            &[
                "-c",
                "protocol.file.allow=always",
                "fetch",
                "-q",
                &self.gitdir.to_string_lossy(),
                // Forced. The ref is omh's own scratch namespace, and every
                // failure path leaves it behind — including the carried-secret
                // refusal, whose own advice is "drop that commit in the sandbox
                // and harvest again". Dropping a commit is a rewind, so the next
                // fetch was non-fast-forward and `--keep` died permanently for
                // that session, with a message naming a ref the user has never
                // heard of. Following omh's instructions must not brick omh.
                &format!("+HEAD:{scratch}"),
            ],
        )?;

        // Where to replay from: what the last harvest took, or the seed if
        // this is the first. Replaying from the seed every time is what made
        // `--keep` a one-shot — the second run offered commits the branch had
        // already been given, and whether that duplicated them or died applying
        // them came down to whether the patches still fitted.
        //
        // The record is checked against the history it claims to be part of. An
        // agent that `reset --hard`s below it — one of the four commands this
        // repository exists to give back — leaves a replay point the sandbox no
        // longer reaches. Replaying from the seed instead would hand the branch
        // work it already has, and picking a different point for the user is
        // not omh's to do, so it stops.
        let from = match self.landed()? {
            Some(landed) => {
                // `is_ok` covers two answers and the message names both: *not
                // an ancestor*, which is the agent having rewound, and *git
                // could not tell*, which is a record that no longer reads as a
                // commit at all. Refusing is right for either; blaming the agent
                // for the second would not be.
                let reaches =
                    git_in(repo, &["merge-base", "--is-ancestor", &landed, &scratch]).is_ok();
                anyhow::ensure!(
                    reaches,
                    "the sandbox's history no longer reaches {}, which is what omh last \
                     kept from it — a `reset --hard` or a rebase below that point, or a \
                     record git can no longer read as a commit. omh will not guess which \
                     commits are new. Take the files as they stand with `omh s commit -m`",
                    &landed[..landed.len().min(8)]
                );
                landed
            }
            None => seed.clone(),
        };

        let range = format!("{from}..{scratch}");
        // Not `unwrap_or(0)`. A count that did not parse is a question git did
        // not answer, and zero here means *nothing to keep* — it returns
        // success without ever running the carried-secret scan below.
        let count: usize = git_in(repo, &["rev-list", "--count", &range])?
            .trim()
            .parse()
            .with_context(|| format!("counting what {} has to hand over", self.branch))?;
        if count == 0 {
            git_in(repo, &["update-ref", "-d", &scratch])?;
            return Ok(0);
        }
        self.refuse_carried(repo, &range, carried)?;

        // The guard `Session::commit` makes and this did not: "omh will not
        // commit to a branch it did not open". Without it a session worktree
        // that wandered — `worktree add -b` losing to git's DWIM, or one left
        // detached — still had its branch rewritten by `update-ref`, while the
        // `reset --mixed` below fixed the *worktree's own* unrelated HEAD. The
        // commits landed on a branch nobody was standing on and omh reported
        // success.
        let head = git_in(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        anyhow::ensure!(
            head.trim() == branch,
            "{} is on {} rather than {branch}; omh will not move a branch the \
             session is not on",
            worktree.display(),
            head.trim()
        );

        let before = git_in(repo, &["rev-parse", branch])?.trim().to_string();

        // Named for the session. One path shared by every harvest meant the
        // first thing a second one did was force-remove the first one's
        // worktree — measured, `worktree remove --force` tears down a live
        // interactive rebase and exits 0, so curation in progress simply
        // vanished. Under the common dir rather than `.git`, which is a *file*
        // in a linked worktree or a submodule and made `worktree add` fail on
        // "could not create leading directories".
        let common = git_in(repo, &["rev-parse", "--git-common-dir"])?;
        let replant = repo
            .join(common.trim())
            .join(format!("omh-harvest-{}", self.branch));
        let _ = git_in(
            repo,
            &["worktree", "remove", "--force", &replant.to_string_lossy()],
        );
        git_in(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &replant.to_string_lossy(),
                &scratch,
            ],
        )?;

        let curated = Self::replant(&replant, branch, &from, curate, &scratch)
            .and_then(|()| Self::stamp(&replant, &before));
        // Counted *after* curation, because the number is reported as what was
        // kept and the user drops commits in the todo list — `--empty=drop`
        // removes more. Taken from the range that actually landed, this said
        // "kept 3" over a branch that got 1.
        let landed = curated
            .and_then(|()| git_in(&replant, &["rev-parse", "HEAD"]))
            .and_then(|tip| {
                let tip = tip.trim().to_string();
                let n = git_in(
                    &replant,
                    &["rev-list", "--count", &format!("{before}..{tip}")],
                )?;
                // Not `unwrap_or(0)` either — the same rule as the count above.
                // This is the number the user is told was kept, over a branch
                // that has already moved: "kept 0" after landing three is a lie
                // about work they now have.
                let n: usize = n
                    .trim()
                    .parse()
                    .with_context(|| format!("counting what landed on {branch}"))?;
                Ok((tip, n))
            });

        // Removed before any error surfaces, so no message may point at it:
        // `worktree remove --force` deletes a conflicted rebase and all, exit 0.
        // What survives a failure is the fetched ref, and that is what
        // `replant`'s message names.
        let _ = git_in(
            repo,
            &["worktree", "remove", "--force", &replant.to_string_lossy()],
        );

        // Only now. Until the branch has the work, the fetched ref is the copy
        // that survives a failure — and after it, the same ref is the only thing
        // keeping the pre-curation objects reachable in the user's repository.
        let (tip, landed) = landed?;
        // With `before` as the expected old value. `update-ref` will force-move
        // a branch that is checked out in another worktree — `git branch -f`
        // refuses, this does not — so without the third argument a commit made
        // on the session branch while the todo list sat open was discarded, and
        // omh reported a cheerful "kept N".
        git_in(
            repo,
            &["update-ref", &format!("refs/heads/{branch}"), &tip, &before],
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "{e}\n\n{branch} moved while the harvest was running, so nothing was \
                 written. The work is still at {scratch} — run `omh s commit \
                 --keep` again"
            )
        })?;
        // Recorded before the ref goes, because the ref is what names it. From
        // here the next harvest replays from this point rather than from the
        // seed, which is what makes landing work in rounds possible at all.
        //
        // After the branch has it, never before: a record written earlier would
        // claim a handover that a later failure undid, and the next harvest
        // would skip commits nobody ever received. Skipping is the worse of the
        // two, because what it loses goes quietly.
        //
        // **The window on the other side is open and this does not close it.** A
        // ref move and a file write cannot be one operation, so a process killed
        // between them leaves the branch holding work the record does not
        // mention, and the next harvest offers it again — this defect, reached
        // by a crash rather than by a second run. It surfaces as a refusal or a
        // no-op rather than as damage, because the replant drops what is already
        // upstream and fails loudly when it cannot. Closing it means keeping the
        // record as a ref in the user's repository and moving both in one
        // `update-ref --stdin` transaction: a change to what the record *is*,
        // not to when it is written.
        // Everything from here on runs *after* the branch already has the
        // work, and this function's other failures promise the opposite — "the
        // branch is untouched", "nothing was written". A bare git error here
        // would be read the same way and it would be the wrong way round, so
        // each of these says what is already true of the branch.
        let landed_on = |what: &str| {
            format!(
                "{what}, but {branch} already has the work — the harvest itself \
                 succeeded. A later `--keep` may offer those commits again"
            )
        };
        let handed_over = git_in(repo, &["rev-parse", &scratch])
            .with_context(|| landed_on("omh could not read back what it handed over"))?;
        std::fs::write(&self.landed_record, handed_over.trim())
            .with_context(|| landed_on("omh could not record what it handed over"))?;
        git_in(repo, &["update-ref", "-d", &scratch])
            .with_context(|| landed_on("omh could not clean up its own scratch ref"))?;

        // The branch moved under a live worktree, so its index describes the
        // commit that used to be HEAD. Without this `git status` reports files
        // deleted that are sitting on disk.
        git_in(worktree, &["reset", "-q", "--mixed"])
            .with_context(|| landed_on("omh could not refresh the session's index"))?;
        Ok(landed)
    }

    /// The curation pass: the agent's commits onto the branch, the user's shape.
    fn replant(at: &Path, branch: &str, seed: &str, curate: bool, scratch: &str) -> Result<()> {
        let mut args = vec!["rebase", "--onto", branch, seed, "--empty=drop"];
        if curate {
            args.push("-i");
        } else {
            args.push("-q");
        }

        // Inherited stdio when curating, and this is the whole flag working or
        // not. `git_in` captures output, so `rebase -i` launched an editor with
        // no terminal and `vi` sat there forever — measured, by a check that
        // hung for two minutes. With an editor that exits without a tty it is
        // worse: the rebase proceeds on the *unedited* todo, no error, and omh
        // reports a curation that never happened.
        //
        // `session.rs` already carries this rule for `git commit` in the same
        // words — "an editor with nowhere to draw hangs". This was written
        // without reading it.
        if curate {
            let ok = Command::new("git")
                .current_dir(at)
                .args(NEUTRALISED.iter().flat_map(|kv| ["-c", kv]))
                .args(&args)
                .status()
                .context("running git rebase -i")?;
            return if ok.success() {
                Ok(())
            } else {
                Err(Self::replant_failed(scratch))
            };
        }

        git_in(at, &args)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{e}\n\n{}", Self::replant_failed(scratch)))
    }

    /// What is true after a replant that did not finish, whichever way it went.
    fn replant_failed(scratch: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "the branch is untouched and nothing is lost — omh fetched the work \
             before replanting, and it is still at {scratch}. Look at it with \
             `git log {scratch}`, or take the files without the history using \
             `omh s commit -m`"
        )
    }

    /// The enforcement pass. Non-interactive on purpose.
    ///
    /// `--allow-empty` on the amend because `--empty=drop` governs commits that
    /// *become* empty and keeps ones that started that way. An agent's
    /// `git commit --allow-empty` marker then reached here and the amend died
    /// on "doing so would make it empty", taking the whole harvest — and the
    /// curation the user had just done — with it.
    fn stamp(at: &Path, from: &str) -> Result<()> {
        let exec = format!(
            "git -c user.name='{AUTHOR_NAME}' -c user.email='{AUTHOR_EMAIL}' \
             commit -q --amend --no-edit --reset-author --allow-empty"
        );
        git_in(
            at,
            &[
                "rebase", "-q", "--onto", from, from, "HEAD", "--exec", &exec,
            ],
        )
        .map(|_| ())
    }

    /// Stop if anything the user carried in reached a commit.
    ///
    /// Four doors, and a path check closes one of them: `git add -f .env` it
    /// catches, a copy under another name it does not, nor a value pasted into
    /// source, nor one written into a commit message.
    ///
    /// So three searches, not two. `-S` is a pickaxe over diff *content* and
    /// does not read messages at all — measured, it finds nothing for a secret
    /// that appears only in a subject line, while `--grep` finds it. What `-S`
    /// does give, and the reason it is the right tool for the other two, is
    /// that it reports a secret added in one commit and removed in a later one
    /// inside the same range: both change the occurrence count, so both are
    /// named.
    ///
    /// **`-F` on the `--grep`, or the needle is a pattern rather than the bytes
    /// it is.** `--grep` takes a *pattern*, and which language it is written in
    /// is the reader's setting: `grep.patternType` is `basic` unless someone
    /// says otherwise, and people do say otherwise. Measured against git 2.55.0
    /// across all three, on a commit subject quoting the secret verbatim:
    ///
    /// | in the secret | `basic` | `extended` | `perl` |
    /// |---|---|---|---|
    /// | `*` | **missed** | missed | missed |
    /// | `+` | found | **missed** | **missed** |
    /// | `[` | fatal | fatal | fatal |
    ///
    /// So a `*` in a secret goes through on a stock install, a `+` goes through
    /// for anyone who set `extended` or `perl`, and an unbalanced `[` takes the
    /// whole feature down — `git log` exits 128, the harvest fails, and
    /// `--keep` stays dead for that session. A guard whose reach depends on the
    /// user's dotfiles is not a guard. `-F` pins it: fixed strings, whatever
    /// `grep.patternType` says. `-S` needs no such flag — a pickaxe is already
    /// literal unless `--pickaxe-regex` asks otherwise.
    ///
    /// omh is in a position no scanner is — it staged these files, so it knows
    /// the bytes and never has to guess. With one caveat worth stating: it
    /// reads them from the checkout *now*, so a secret rotated mid-session is
    /// matched at its new value and an agent commit holding the old one goes
    /// through.
    fn refuse_carried(&self, repo: &Path, range: &str, carried: &[String]) -> Result<()> {
        for rel in carried {
            let rel = rel.trim().trim_end_matches('/');
            let found = git_in(repo, &["log", "--oneline", range, "--", rel])?;
            if let Some(line) = found.lines().next() {
                anyhow::bail!(
                    "{rel} is a carried file and {line} has it. omh will not \
                     rewrite your history to hide a secret — drop that commit in \
                     the sandbox and harvest again, or take the files without the \
                     history with `omh s commit -m`"
                );
            }
            for needle in Self::needles(&repo.join(rel))? {
                for (how, args) in [
                    ("contains", vec!["log", "--oneline", "-S", &needle, range]),
                    (
                        "mentions",
                        vec!["log", "--oneline", "-F", "--grep", &needle, range],
                    ),
                ] {
                    let hit = git_in(repo, &args)?;
                    if let Some(line) = hit.lines().next() {
                        anyhow::bail!(
                            "{line} {how} a line from {rel}, which you carried in. \
                             omh will not rewrite your history to hide a secret — \
                             drop that commit in the sandbox and harvest again"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Lines worth searching for: long enough to mean something, and not a
    /// comment or a blank.
    ///
    /// Walks a directory, because `carry_in` takes one and `read_to_string` on
    /// a directory is an `Err` — which, defaulted, meant **zero needles and a
    /// content scan that silently did nothing**. A `certs/` entry got the path
    /// check only, so `cp certs/deploy.key infra/key.pem` and commit put a
    /// private key on the branch through the door this function exists to shut.
    ///
    /// Unreadable is an error, not an empty answer. A file that has been
    /// rotated, renamed or made unreadable since launch is a case where omh
    /// cannot tell whether the harvest is clean, and "cannot tell" must not
    /// spell the same as "clean" in the one module whose subject is the user's
    /// secrets. Binary files are the exception the loop takes deliberately:
    /// they decode-fail, and a byte sequence is not a line to search for.
    fn needles(at: &Path) -> Result<Vec<String>> {
        if at.is_dir() {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(at)
                .with_context(|| format!("reading carried directory {}", at.display()))?
                .flatten()
            {
                out.extend(Self::needles(&entry.path())?);
            }
            return Ok(out);
        }
        if !at.exists() {
            return Ok(Vec::new());
        }
        let body = match std::fs::read_to_string(at) {
            Ok(body) => body,
            // Not text. Nothing to search for line-wise, and the path check
            // still covers it.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| format!("reading carried file {}", at.display()))
            }
        };
        Ok(body
            .lines()
            .map(str::trim)
            .filter(|l| l.len() >= 12 && !l.starts_with('#') && !l.starts_with("//"))
            .map(str::to_string)
            .collect())
    }

    /// omh's own view of what this repository's config may say.
    ///
    /// Everything `git init` needs to know about the repository, plus an
    /// identity, and nothing else. Anything the agent added is dropped — not
    /// because a relaunch is a boundary, it can set the key again a second
    /// later, but because a key that persists is one omh will still be reading
    /// long after whatever wrote it, and several of them turn `git` into *run
    /// whatever this repository says*.
    ///
    /// Written by asking git rather than by composing the file: `git init`
    /// records `repositoryformatversion` and `filemode` here, and a hand-built
    /// config that forgot either would be a repository git reads differently
    /// from the one it made. So the keys outside the allowlist are unset one by
    /// one and the rest is left exactly as git wrote it.
    ///
    /// The identity is not a claim about who wrote anything — a harvest stamps
    /// authorship on the host — it is there because git will not commit without
    /// one and the container has no global config to supply it.
    fn write_config(gitdir: &Path) -> Result<()> {
        // What git records about the repository itself, as opposed to what it
        // should *do* — and the difference is the whole allowlist. Several of
        // these are detected from the filesystem when the repository is made
        // and differ by platform: `ignorecase` and `precomposeunicode` appear
        // on macOS and not on a case-sensitive Linux box, `symlinks` on
        // Windows. Dropping one leaves a repository git reads differently from
        // the one it created — paths differing only in case become two files,
        // accented filenames flip between NFC and NFD and read as modified.
        //
        // The two `extensions.*` keys git itself sets, by name rather than by
        // prefix. The prefix was wider than it needed to be: git refuses to
        // work in a repository whose extensions it does not recognise, so a
        // preserved `extensions.whatever` from an older shadow is a harvest
        // that can never run again. Losing a real one would be worse, which is
        // why these two are named.
        //
        // Guarded by `refreshing_keeps_what_git_records_about_the_repository`,
        // which measures against a fresh `git init` on the machine running it
        // rather than against this list. A git that starts recording something
        // new turns it red on the platform where that matters, which is the
        // only way a list like this stays true.
        const KEEP: [&str; 10] = [
            "core.repositoryformatversion",
            "core.filemode",
            "core.bare",
            "core.logallrefupdates",
            "core.worktree",
            "core.ignorecase",
            "core.precomposeunicode",
            "core.symlinks",
            "extensions.objectformat",
            "extensions.refstorage",
        ];

        let listed = Command::new("git")
            .arg("--git-dir")
            .arg(gitdir)
            .args(["config", "--list", "--local", "--name-only"])
            .output()
            .with_context(|| format!("reading the config of {}", gitdir.display()))?;
        // Asked rather than assumed. Empty output means *no keys*; a listing
        // that failed means omh does not know what is in there, and the two
        // must not spell the same — this function's whole job is dropping what
        // it finds, so believing an empty answer it never got would sanitise
        // nothing and report success. The relaunch path is exactly where a key
        // worth dropping would be.
        anyhow::ensure!(
            listed.status.success(),
            "reading the config of {}: {}",
            gitdir.display(),
            String::from_utf8_lossy(&listed.stderr).trim()
        );
        // A **set**, because `--list --name-only` prints a key once per value.
        // Unsetting takes every value at once, so a multi-valued key arriving
        // twice meant a second `--unset-all` with nothing left to remove, exit
        // 5, empty stderr — and a launch aborted over a key that had already
        // gone. Measured against git 2.55.0.
        let listed = String::from_utf8_lossy(&listed.stdout);
        let keys: std::collections::BTreeSet<&str> = listed
            .lines()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .collect();
        for key in keys {
            if KEEP.contains(&key) || key == "user.name" || key == "user.email" {
                continue;
            }
            let dropped = Command::new("git")
                .arg("--git-dir")
                .arg(gitdir)
                .args(["config", "--unset-all", key])
                .output()
                .with_context(|| format!("dropping {key} from {}", gitdir.display()))?;
            anyhow::ensure!(
                dropped.status.success(),
                "git config --unset-all {key}: {}",
                String::from_utf8_lossy(&dropped.stderr).trim()
            );
        }

        for (key, value) in [("user.name", AUTHOR_NAME), ("user.email", AUTHOR_EMAIL)] {
            let set = Command::new("git")
                .arg("--git-dir")
                .arg(gitdir)
                .args(["config", key, value])
                .output()
                .with_context(|| format!("setting {key} on {}", gitdir.display()))?;
            anyhow::ensure!(
                set.status.success(),
                "git config {key}: {}",
                String::from_utf8_lossy(&set.stderr).trim()
            );
        }
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
        // With the seed, and for the reason the seed goes: session ids come
        // back around, and a replay point inherited by a stranger says a branch
        // has already been given commits it has never seen.
        let _ = std::fs::remove_file(&self.landed_record);
    }

    /// Keep the agent's own `git status` clean at launch.
    ///
    /// Advisory, and known to be: `git add -f` walks straight through it. It is
    /// here so an honest agent is not shown a secret to commit, not to stop a
    /// determined one — what stops the secret reaching the branch is the check
    /// on the host when work crosses back.
    ///
    /// Takes the directory rather than reading `self.gitdir`, because the
    /// first of its two callers runs while the repository is still being built
    /// under another name. The second is the fast path in `ensure`, which
    /// passes the finished gitdir on every relaunch.
    fn write_exclude(gitdir: &Path, excluded: &[String]) -> Result<()> {
        let info = gitdir.join("info");
        // Named, because this is the one write that can fail a launch which
        // would otherwise have been a no-op — the fast path in `ensure` did no
        // I/O at all before. `Permission denied (os error 13)` with no path is
        // not something a user can act on, and the directory it names is one
        // the agent can chmod.
        std::fs::create_dir_all(&info).with_context(|| format!("preparing {}", info.display()))?;
        // Just what the caller names. `container::plan` derives that from the
        // mounts it is about to make, which already covers omh's staged rules —
        // chaining `carry::STAGED_RULES` here as well only made the list
        // disagree with its own source when a capability changed.
        let body: String = excluded.iter().map(|n| format!("{n}\n")).collect();
        let at = info.join("exclude");
        std::fs::write(&at, body).with_context(|| format!("writing {}", at.display()))?;
        Ok(())
    }

    /// A signpost on the accidental path, and **not a wall** — this said "the
    /// one wall left standing" and that was wrong three ways, each one command
    /// long. Measured against git 2.55.0 and a reachable remote:
    ///
    /// | what the agent runs | remote |
    /// |---|---|
    /// | `git push` | refused, 0 commits |
    /// | `git push --no-verify` | **pushed** |
    /// | `git -c core.hooksPath=/dev/null push` | **pushed** |
    /// | `rm hooks/pre-push; git push` | **pushed**, until the mount |
    ///
    /// That last row is why `container::plan` mounts `GUEST_PRE_PUSH` read-only
    /// over this copy: the gitdir is writable because the agent commits into
    /// it, so without the mount the hook is a file the agent can simply take
    /// away. With it, `rm` gives `Resource busy` and overwriting gives
    /// `Read-only file system`.
    ///
    /// The first two rows are unaffected — neither flag reads the file — so
    /// this is still not a wall. Nothing here contains a determined agent, and
    /// nothing ever did: the container has `curl` and unrestricted egress, so a
    /// push was never the narrow path out.
    ///
    /// What it is for is the *honest* path, which is the likely one. git's own
    /// error for a repository with no remote suggests `git remote add`, so git
    /// walks the agent to the edge; this is what meets it there and says why in
    /// omh's words rather than leaving it to read a transport failure.
    ///
    /// git's own hook rather than a pattern over the command line, because git
    /// knows what a push is — though note that argument cuts both ways, and the
    /// `--no-verify` row above is the same "every shape an agent emits" problem
    /// the base set's retired pattern had.
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
        // Written here as well as mounted read-only over the top: the mount is
        // what the agent meets, and this is what a host-side reader would see
        // and what makes the gitdir self-contained if it is ever inspected
        // outside a container.
        std::fs::write(&hook, pre_push_hook())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }
}

/// Who the sandbox's own commits are authored as.
///
/// `.invalid` is reserved by RFC 2606 precisely so it can never resolve, which
/// is what you want on an address nobody should write to. The name says where
/// the commit was made rather than guessing at which harness made it — omh
/// supports several, and the seed is written before any of them starts.
const AUTHOR_NAME: &str = "omh sandbox";
const AUTHOR_EMAIL: &str = "sandbox@omh.invalid";

/// What the arrangement actually is, said once.
///
/// Two surfaces deliver it and they must not drift: the seed commit's message,
/// which every `git log` and editor timeline renders at the moment the agent is
/// working out what this repository is, and the `git-rules` section omh puts in
/// the agent's context every turn. `GIT_ABSENT` was one string for the same
/// reason, when the sentence it carried was the opposite one.
pub const ARRANGEMENT: &str = "This repository is the sandbox's own, and it is not the branch \
     anyone reviews. Commit as often as you like — that is what it is for, and `git reset --hard` \
     back to a checkpoint is yours to use. What reaches the person you are working with is the \
     state of the files, which they read with `omh s diff` and commit with `omh s commit` on the \
     host, pushing with `omh s push` when they are ready. Your commit messages here stay here.\n\n\
     There is nothing to push and no remote to push to, and adding one will not work either — \
     the config is not yours to write. That is the arrangement rather than a fault to repair. Say \
     so rather than offering to push, and do not offer to commit on the host — that is theirs to \
     do.";

/// The seed commit's message, which is a delivery surface and not a label.
///
/// Every `git log`, `git show` and editor timeline renders it, at the moment
/// the agent is working out what this repository is — which a rules section,
/// paid for once and then competing with everything after it, cannot reach.
pub fn seed_message() -> String {
    format!("The session starts here.\n\n{ARRANGEMENT}")
}

/// Why a push cannot work, said where the agent is trying to push.
const NO_PUSH: &str =
    "omh: nothing to push from here. This repository is the sandbox's own and has no remote — \
     your work reaches the outside through the host, where `omh s commit` puts it on the branch \
     and `omh s push` sends it. Say that rather than trying to push yourself.";

/// Config keys that turn `git` into "run whatever this repository says".
///
/// Every one of these is agent-writable — they live in `/omh/shadow/config`,
/// which is a read-write mount because the agent commits through it. Today
/// nothing host-side reads an *existing* shadow (`ensure` returns early, `reap`
/// only unlinks), so none of them can fire. That is a property of the call
/// sites, not of anything enforcing it.
///
/// It will stop being true: `Session::remove` already records that the refusal
/// `rm` owes the agent's checkpoints "belongs with the harvest that gives them
/// somewhere to go", and a harvest reads those commits **on the host, as the
/// user**. Neutralised here rather than there, because the person writing the
/// harvest will be reading a doc comment, and a doc comment is not a guard.
const NEUTRALISED: [&str; 6] = [
    // `ensure` has carried this since the seed commit and said why: a global
    // `commit.gpgsign = true` fails with `gpg failed to sign the data`, over a
    // commit the user never made in a repository they cannot see. The harvest
    // then added three more commit sites and read none of that — the
    // checkpoint, the replant and the stamp all died for anyone who signs by
    // default. `--no-verify` covers hooks and nothing of signing.
    "commit.gpgsign=false",
    "core.hooksPath=",
    "core.pager=cat",
    "core.fsmonitor=",
    "core.sshCommand=",
    "protocol.file.allow=never",
];

/// `git` in an ordinary repository — the user's checkout, or a worktree of it.
///
/// Separate from the gitdir/worktree helper below because the harvest works on
/// *both* sides: the sandbox's repository, which needs the explicit pair, and
/// the user's own, which does not. Carries the same neutralising flags, since
/// half of what it touches came from the sandbox.
fn git_in(at: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(at)
        .args(NEUTRALISED.iter().flat_map(|kv| ["-c", kv]))
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

fn git(gitdir: &Path, worktree: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(NEUTRALISED.iter().flat_map(|kv| ["-c", kv]))
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

    /// A hook the *agent* plants in its own gitdir must not run when omh
    /// touches that gitdir from the host.
    ///
    /// Nothing host-side reads an existing shadow today, so this cannot fire
    /// yet — which is exactly why it is worth pinning now. The harvest
    /// `Session::remove` already promises is a host-side reader of commits the
    /// agent wrote, and it will be written by someone reading a doc comment.
    #[test]
    fn a_hook_the_sandbox_plants_does_not_run_on_the_host() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // what an agent with a writable gitdir can arrange
        let planted = shadow_dir.join("planted");
        let hooks = shadow_dir.join("evil-hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", planted.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // written the way the agent would: into the repository's own config
        Command::new("git")
            .arg("--git-dir")
            .arg(&s.gitdir)
            .args(["config", "core.hooksPath"])
            .arg(&hooks)
            .output()
            .unwrap();

        let _ = git(&s.gitdir, &wt, &["checkout", "-q", "--", "."]);

        assert!(
            !planted.exists(),
            "the sandbox's own config decided what ran on the host"
        );
    }

    /// "One string, two deliveries" is the reason `ARRANGEMENT` exists, and
    /// only one of the two was pinned. The rules side is asserted three times
    /// over; the seed commit's side was asserted nowhere, and replacing
    /// `seed_message()` with a bare "The session starts here." left the whole
    /// suite green.
    ///
    /// This is the delivery the module doc calls the one a rules section
    /// "cannot reach" — it arrives on `git log`, at the moment the agent is
    /// working out what this repository is — so losing it silently is losing
    /// the argument for the refactor.
    #[test]
    fn the_seed_commit_carries_the_arrangement_to_git_log() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        let said = git(&s.gitdir, &wt, &["log", "-1", "--format=%B"]).unwrap();
        assert!(
            said.contains(ARRANGEMENT),
            "the agent reads this on `git log` and nowhere else: {said}"
        );
    }

    /// The whole point: the agent's commits reach the branch with the messages
    /// it wrote, and authored as the sandbox rather than as whoever the sandbox
    /// claimed to be.
    #[test]
    fn a_harvest_lands_the_agents_commits_under_the_sandboxs_name() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // the agent works, and claims to be someone it is not
        git(
            &s.gitdir,
            &wt,
            &["config", "user.name", "Nathanael Cherrier"],
        )
        .unwrap();
        git(
            &s.gitdir,
            &wt,
            &["config", "user.email", "nathanael@mindsers.it"],
        )
        .unwrap();
        std::fs::write(wt.join("f.txt"), "one\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "Fix the tap guard"]).unwrap();
        std::fs::write(wt.join("helper.rs"), "fn h() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "Extract helper"]).unwrap();
        std::fs::write(wt.join("tail.rs"), "fn t() {}").unwrap(); // never checkpointed

        let landed = s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap();

        let log = git_in(&checkout, &["log", "--format=%an|%s", "main..omh/s01"]).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(landed, lines.len(), "reported count is what landed: {log}");
        assert!(
            lines.iter().any(|l| l.ends_with("|Extract helper")),
            "the agent's own messages are the point: {log}"
        );
        assert!(
            lines.iter().all(|l| l.starts_with(AUTHOR_NAME)),
            "the sandbox says who it is on the way in, whatever it claimed: {log}"
        );
        assert!(
            git_in(&checkout, &["show", "omh/s01:tail.rs"]).is_ok(),
            "work the agent never checkpointed is still its work"
        );
        assert!(
            git_in(
                &checkout,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    "refs/omh/s01-scratch/harvest"
                ]
            )
            .is_err(),
            "the fetched ref goes once the branch has the work, or the \
             pre-curation objects stay reachable in the user's repository"
        );
    }

    /// A carried secret must not reach the branch, and a path check alone
    /// closes one of the ways it gets there. Measured before this existed: a
    /// path strip caught `git add -f .env` and let through a copy under another
    /// name and a value pasted into source.
    ///
    /// Refused rather than stripped. Removing the path leaves an empty commit
    /// with a misleading message and does nothing about the other shapes, and
    /// rewriting the agent's work to hide a secret is the user's call.
    #[test]
    fn a_harvest_refuses_a_commit_holding_something_you_carried_in() {
        for (name, plant) in [
            ("force-added", "add-f"),
            ("copied under another name", "copy"),
            ("pasted into source", "inline"),
            ("written into a commit message", "message"),
        ] {
            let (d, wt, shadow_dir) = fixture();
            let checkout = d.path().join("checkout");
            let s = Shadow::new(&shadow_dir, "s01");
            s.ensure(&wt, &[".env".to_string()]).unwrap();
            // the checkout is where omh reads the bytes it carried
            std::fs::write(checkout.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();
            std::fs::write(wt.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();

            match plant {
                "add-f" => {
                    git(&s.gitdir, &wt, &["add", "-f", ".env"]).unwrap();
                }
                "copy" => {
                    std::fs::copy(wt.join(".env"), wt.join("config.bak")).unwrap();
                    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
                }
                "inline" => {
                    std::fs::write(wt.join("k.rs"), "const K = \"API_TOKEN=ghp_abc123def456\";")
                        .unwrap();
                    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
                }
                // The door `-S` cannot see: a pickaxe reads diff content and
                // never the message. Measured — it finds nothing here.
                _ => {
                    std::fs::write(wt.join("note.rs"), "fn n() {}").unwrap();
                    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
                }
            }
            let subject = if plant == "message" {
                "note: API_TOKEN=ghp_abc123def456"
            } else {
                "Save config"
            };
            git(&s.gitdir, &wt, &["commit", "-qm", subject]).unwrap();

            let err = s
                .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], false)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("carried"),
                "{name}: a harvest must refuse it, not carry it: {err}"
            );
            assert!(
                git_in(&checkout, &["log", "--oneline", "main..omh/s01"])
                    .unwrap()
                    .trim()
                    .is_empty(),
                "{name}: and the branch must be untouched"
            );
        }
    }

    /// A secret is matched as the bytes it is, not as a pattern.
    ///
    /// The needle carries a `*` deliberately, and the choice is the whole
    /// worth of the test. `--grep` reads a pattern in whatever language
    /// `grep.patternType` names, and `*` is a quantifier in all three of them,
    /// so this is red on any machine. A `+` would not be: it is literal under
    /// `basic`, which is the default, and only bites the people who set
    /// `extended` or `perl` — this test was written with one and passed for
    /// the author's dotfiles rather than for git.
    ///
    /// Planted in the **message only**, because that is the one door `-S`
    /// cannot see: the pickaxe is already a fixed string and would have caught
    /// it in a diff, and the test would then have proved nothing about the
    /// path it exists for.
    #[test]
    fn a_secret_that_looks_like_a_pattern_is_still_caught() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();

        // `*` and `/` are ordinary base64. As a pattern this matches `KEY=acd…`
        // and `KEY=abbcd…` — never the literal bytes the agent wrote.
        let secret = "KEY=ab*cd/ef12345==";
        std::fs::write(checkout.join(".env"), format!("{secret}\n")).unwrap();
        std::fs::write(wt.join(".env"), format!("{secret}\n")).unwrap();

        std::fs::write(wt.join("note.rs"), "fn n() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(
            &s.gitdir,
            &wt,
            &["commit", "-qm", &format!("ship with {secret}")],
        )
        .unwrap();

        let err = s
            .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("carried"),
            "a secret that is its own regex still has to be refused: {err}"
        );
        assert!(
            git_in(&checkout, &["log", "--oneline", "main..omh/s01"])
                .unwrap()
                .trim()
                .is_empty(),
            "and the branch must be untouched"
        );
    }

    /// A carried line that is not valid regex must not break the harvest.
    ///
    /// The same defect from the other side, and this one takes the whole
    /// feature down rather than letting something through: an unbalanced `[` is
    /// a syntax error to `--grep`, so `git log` exits 128, the harvest fails,
    /// and `--keep` stays dead for that session until the file changes.
    ///
    /// Measured under every `grep.patternType` — `fatal: command line,
    /// 'SECRET=a[bc': brackets ([ ]) not balanced` under `basic` and
    /// `extended`, `missing terminating ] for character class` under `perl`.
    /// The wording moves with the setting; the exit code does not.
    #[test]
    fn a_carried_line_that_is_not_a_regex_does_not_break_the_harvest() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();

        let carried = "SECRET_TOKEN=a[bcdefgh";
        std::fs::write(checkout.join(".env"), format!("{carried}\n")).unwrap();
        std::fs::write(wt.join(".env"), format!("{carried}\n")).unwrap();

        std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

        let landed = s
            .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], false)
            .expect("a carried line nobody committed is not a reason to refuse");
        assert_eq!(landed, 1, "the agent's commit still has to land");
    }

    /// Landing the same work twice is not landing it twice.
    ///
    /// The harvest replayed from the seed every time, and nothing recorded what
    /// had already been kept — so a second `--keep` offered commits that were
    /// on the branch already. Whether that duplicated them or died applying
    /// them depended on whether the patches still fitted; measured against git
    /// 2.55.0, an edit to a line a later commit also touched conflicts, and
    /// `--keep` reported nothing but `Could not apply`.
    ///
    /// Landing twice must be a no-op, and the branch must not move.
    #[test]
    fn harvesting_twice_keeps_nothing_the_second_time() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        std::fs::write(wt.join("f.txt"), "base\nadded\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "Add a line"]).unwrap();
        std::fs::write(wt.join("f.txt"), "base\nadded\nmore\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "Add another"]).unwrap();

        assert_eq!(s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap(), 2);
        let tip = git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap();

        assert_eq!(
            s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap(),
            0,
            "there is nothing new to keep, and saying so is the whole job"
        );
        assert_eq!(
            git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap(),
            tip,
            "and a branch with nothing to add must not move"
        );
    }

    /// The second harvest takes what the first one did not.
    ///
    /// The loop the replay point exists for: work, keep, work again, keep
    /// again. Only the new commits land, in order, once each.
    ///
    /// **This is the shape of the loop, not the guard for it**, and that is
    /// measured rather than assumed: with the replay point neutralised it stays
    /// green. Replaying a round that is already on the branch gives git a patch
    /// whose id is upstream, and it drops it — so the incremental path repairs
    /// itself and says nothing.
    ///
    /// Editing the same line each round instead of appending was tried, on the
    /// theory that a stale context would make the replay conflict. It does not:
    /// the patch-id match happens first, and the test stays green that way too.
    /// The guard is `harvesting_twice_keeps_nothing_the_second_time`, where
    /// nothing is new and the replayed patch has nowhere to go.
    ///
    /// Recorded so the next person does not spend the same hour proving it.
    #[test]
    fn a_second_harvest_takes_only_what_is_new() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        std::fs::write(wt.join("f.txt"), "base\nfirst\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "The first round"]).unwrap();
        assert_eq!(s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap(), 1);

        std::fs::write(wt.join("f.txt"), "base\nfirst\nsecond\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "The second round"]).unwrap();

        assert_eq!(
            s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap(),
            1,
            "only the round that has not landed yet"
        );
        let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
        let subjects: Vec<&str> = log.lines().collect();
        assert_eq!(
            subjects,
            vec!["The second round", "The first round"],
            "each round lands once, in order: {log}"
        );
    }

    /// A key with two values is one key, and dropping it is one job.
    ///
    /// `config --list --name-only` prints a key once **per value**, so a
    /// multi-valued key arrives twice — and `--unset-all` removes every value
    /// on the first call, leaving the second to exit 5 with empty stderr. Read
    /// as a failure that aborts the launch, which is what it did: a shadow from
    /// before the config was mounted read-only, where an agent had ever run
    /// `git config --add` twice, could not be relaunched at all. The error
    /// named a key and said nothing else.
    ///
    /// The module's own rule, from the fetch a few hundred lines up: following
    /// omh's instructions must not brick omh. Neither may upgrading omh.
    #[test]
    fn a_key_with_two_values_does_not_brick_the_next_launch() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // what a shadow from before the read-only mount can hold
        git(&s.gitdir, &wt, &["config", "--add", "core.gitproxy", "one"]).unwrap();
        git(&s.gitdir, &wt, &["config", "--add", "core.gitproxy", "two"]).unwrap();
        assert_eq!(
            git(
                &s.gitdir,
                &wt,
                &["config", "--list", "--local", "--name-only"]
            )
            .unwrap()
            .lines()
            .filter(|k| k.trim() == "core.gitproxy")
            .count(),
            2,
            "the precondition is that git lists the key once per value"
        );

        s.ensure(&wt, &[])
            .expect("a key with two values is still just a key to drop");

        let config = std::fs::read_to_string(s.gitdir.join("config")).unwrap();
        assert!(!config.contains("gitproxy"), "and it is gone:\n{config}");
    }

    /// Refreshing keeps what git records about the repository itself.
    ///
    /// The allowlist cannot be a list of keys someone remembered: `git init`
    /// writes what it detected about *this* filesystem, and that differs by
    /// platform — `core.ignorecase` and `core.precomposeunicode` on macOS,
    /// neither on a case-sensitive Linux box, `extensions.objectformat` in a
    /// sha256 repository. Drop one and the repository git reads is not the one
    /// git made: paths that differ only in case become two files, accented
    /// filenames flip between NFC and NFD and read as modified.
    ///
    /// So the guard is measured against a fresh `git init` on the machine
    /// running it, rather than against a list in this file. A git that starts
    /// recording something new turns this red on the platform where it matters.
    #[test]
    fn refreshing_keeps_what_git_records_about_the_repository() {
        let (d, wt, shadow_dir) = fixture();
        let pristine = d.path().join("pristine.git");
        Command::new("git")
            .args(["init", "-q", "--bare", "--template="])
            .arg(&pristine)
            .output()
            .unwrap();
        let listed = Command::new("git")
            .arg("--git-dir")
            .arg(&pristine)
            .args(["config", "--list", "--local", "--name-only"])
            .output()
            .unwrap();
        let expected: Vec<String> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(str::trim)
            .filter(|k| !k.is_empty() && *k != "core.bare")
            .map(str::to_string)
            .collect();
        assert!(
            !expected.is_empty(),
            "a fresh `git init` records something, or this test is vacuous"
        );

        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        s.ensure(&wt, &[]).unwrap();

        let kept = git(
            &s.gitdir,
            &wt,
            &["config", "--list", "--local", "--name-only"],
        )
        .unwrap();
        for key in expected {
            assert!(
                kept.lines().any(|k| k.trim() == key),
                "{key} is something git recorded about this repository and omh \
                 dropped it. kept:\n{kept}"
            );
        }
    }

    /// The sandbox's config is omh's again at every launch.
    ///
    /// The gitdir is mounted read-write because the agent commits through it,
    /// so every key in it is the agent's to set — and several of them turn
    /// `git` into *run whatever this repository says*. `NEUTRALISED` answers
    /// that for the calls omh makes, but only for the ones that remember to,
    /// and only host-side: inside the container the agent reads its own config
    /// with nothing filtering it.
    ///
    /// So the file becomes omh's own view again on every launch, and what the
    /// agent set in between does not survive. Not because a relaunch is a
    /// boundary — it is not, the agent can set it again a second later — but
    /// because a key that persists is one omh will still be reading a week
    /// later, long after whatever wrote it.
    #[test]
    fn the_sandboxs_config_is_omhs_again_at_every_launch() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // the agent makes its repository into one that runs things
        for (key, value) in [
            ("diff.pwn.textconv", "/bin/sh"),
            ("core.sshCommand", "/bin/sh"),
            ("core.hooksPath", "/tmp/mine"),
        ] {
            git(&s.gitdir, &wt, &["config", key, value]).unwrap();
        }
        std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "a checkpoint"]).unwrap();
        let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

        s.ensure(&wt, &[]).unwrap();

        let config = std::fs::read_to_string(s.gitdir.join("config")).unwrap();
        for key in ["textconv", "sshCommand", "hooksPath"] {
            assert!(
                !config.contains(key),
                "{key} survived a relaunch:\n{config}"
            );
        }
        assert_eq!(
            git(&s.gitdir, &wt, &["config", "user.email"])
                .unwrap()
                .trim(),
            AUTHOR_EMAIL,
            "and the identity omh needs to commit at all is still there"
        );
        assert_eq!(
            git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
            checkpoint,
            "and the agent's work is untouched"
        );
    }

    /// A record omh cannot read is not a session that never landed.
    ///
    /// The two failures this covers arrive by different routes and meet in the
    /// same place. **Unreadable** is a permissions or I/O fault on a record that
    /// exists. **Empty** is what an interrupted write leaves: `fs::write`
    /// truncates before it writes, so a process killed in that window leaves
    /// zero bytes where a commit id was — the very window this file documents a
    /// few dozen lines above, in the code that does the writing.
    ///
    /// Either one read as *never harvested* replays from the seed and skips the
    /// ancestry check with it, offering the branch everything it already has.
    /// That is the defect the record exists to close, so neither may spell the
    /// same as absent.
    #[test]
    fn a_record_omh_cannot_read_is_not_a_session_that_never_landed() {
        for (name, break_it) in [
            ("empty", 0usize),
            #[cfg(unix)]
            ("unreadable", 1usize),
        ] {
            let (d, wt, shadow_dir) = fixture();
            let checkout = d.path().join("checkout");
            let s = Shadow::new(&shadow_dir, "s01");
            s.ensure(&wt, &[]).unwrap();
            std::fs::write(wt.join("f.txt"), "base\nfirst\n").unwrap();
            git(&s.gitdir, &wt, &["commit", "-qam", "The first round"]).unwrap();
            s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap();
            let tip = git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap();

            match break_it {
                0 => std::fs::write(&s.landed_record, "").unwrap(),
                _ => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            &s.landed_record,
                            std::fs::Permissions::from_mode(0o000),
                        )
                        .unwrap();
                    }
                }
            }

            std::fs::write(wt.join("f.txt"), "base\nfirst\nsecond\n").unwrap();
            git(&s.gitdir, &wt, &["commit", "-qam", "The second round"]).unwrap();

            let outcome = s.harvest(&checkout, &wt, "omh/s01", &[], false);
            assert!(
                outcome.is_err(),
                "{name}: a record omh cannot read must not read as a session that \
                 never landed"
            );
            assert_eq!(
                git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap(),
                tip,
                "{name}: and the branch must not move on a refusal"
            );

            #[cfg(unix)]
            if break_it == 1 {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &s.landed_record,
                    std::fs::Permissions::from_mode(0o644),
                );
            }
        }
    }

    /// Removing a session takes its replay point with it.
    ///
    /// Ids come back around — `next_id` is the highest `sNN` plus one — so a
    /// record left behind is inherited by a session that has nothing to do with
    /// it, and says a branch has already been handed commits it has never seen.
    /// The seed goes for this reason and this goes with it.
    #[test]
    fn reaping_takes_the_replay_point_with_it() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        std::fs::write(wt.join("f.txt"), "base\nwork\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "Some work"]).unwrap();
        s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap();
        assert!(
            s.landed().unwrap().is_some(),
            "the precondition is that a harvest recorded one"
        );

        s.reap();

        assert!(
            Shadow::new(&shadow_dir, "s01").landed().unwrap().is_none(),
            "the next session to take this id must start from its own seed"
        );
    }

    /// A sandbox that rewound past what you kept is refused, not replayed.
    ///
    /// `git reset --hard` is one of the four commands this repository exists to
    /// give back, so an agent dropping a checkpoint below the point omh already
    /// harvested is ordinary. What is not ordinary is what a harvest could do
    /// about it: the record names a commit the history no longer reaches, and
    /// replaying from the seed instead would offer the branch work it already
    /// has. Neither is omh's to choose, so it stops and names the way out.
    #[test]
    fn a_sandbox_that_rewound_past_what_you_kept_is_refused() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        let start = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

        std::fs::write(wt.join("f.txt"), "base\nkept\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "Work that gets kept"]).unwrap();
        s.harvest(&checkout, &wt, "omh/s01", &[], false).unwrap();

        // the agent rewinds behind what omh already took
        git(&s.gitdir, &wt, &["reset", "-q", "--hard", start.trim()]).unwrap();
        std::fs::write(wt.join("g.txt"), "different\n").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "A different direction"]).unwrap();

        let err = s
            .harvest(&checkout, &wt, "omh/s01", &[], false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("omh s commit -m"),
            "a refusal has to say what to do instead: {err}"
        );
    }

    /// A carried file must never be **tracked** in the seed, and the reason is
    /// not the one you would guess.
    ///
    /// Tracking it looks like a straight improvement: a tracked file survives
    /// `git clean -fdx` without needing a mount, and without the mount the
    /// agent can edit it with `sed -i` and `mv`, which a mountpoint refuses
    /// with `Device or resource busy`. Measured, all of that is true.
    ///
    /// What is also true, and settles it: a tracked file is in the tree of
    /// **every commit that follows it**, so any fetch that brings one agent
    /// commit brings the file. The harvest fetches this repository into the
    /// *user's own*, so a carried secret in the seed is copied into the real
    /// repository by every harvest that gets past `preflight` — measured,
    /// readable there with `git cat-file -p`.
    ///
    /// Stated that way on purpose. "A fetch takes every reachable object" is
    /// true and invites three rebuttals that all fail: `--depth=1` fetches one
    /// commit and still carries the blob, because it is in that commit's tree;
    /// there is no transport that fetches a range; and the seed cannot be
    /// excluded because it is the rebase base `replant` needs. `--filter=blob:none`
    /// is worse than none of them — the shadow sets no `uploadpack.allowFilter`,
    /// so git warns, sends the blob anyway, and leaves the user's repository a
    /// promisor pointing at a directory `omh s rm` deletes.
    ///
    /// On the success path it is unreachable afterwards, because the scratch
    /// ref is deleted, and "gc will get it eventually" is not a thing to say
    /// about somebody's credentials. On a *failure* path it is worse than that:
    /// the ref is kept deliberately — that is what makes a failed replant
    /// recoverable — so the secret would sit reachable from a live ref in the
    /// user's repository until someone noticed it. Every refusal after the
    /// fetch lands there: a carried file in a commit, a branch that moved, a
    /// replant that conflicted.
    ///
    /// So the mount stays and `sed -i` stays broken, and this guards the trade
    /// against being quietly reversed by someone fixing the visible half.
    #[test]
    fn a_carried_file_is_never_in_the_seed_the_harvest_fetches() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        std::fs::write(wt.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();
        std::fs::write(wt.join("cert.pem"), "BEGIN-ghp_abc123def456-END\n").unwrap();
        // Two, because a partially-effective exclusion passes a test that
        // carries one.
        s.ensure(&wt, &[".env".to_string(), "cert.pem".to_string()])
            .unwrap();

        let tracked = git(&s.gitdir, &wt, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
        // `ls-files`, not only `ls-tree`: staging the file without committing it
        // is the *other* way to make `git clean` spare it, and it leaves the
        // tree clean. `harvest` then runs `add -A .` and commits the index
        // before it fetches, so the index-only variant leaks too — through a
        // commit omh makes itself.
        let staged = git(&s.gitdir, &wt, &["ls-files"]).unwrap();
        for probe in [&tracked, &staged] {
            assert!(
                !probe.contains(".env") && !probe.contains("cert.pem"),
                "the seed must neither track nor stage a carried file: {probe}"
            );
        }

        // and the half that matters — what a harvest would carry across
        git_in(
            &checkout,
            &[
                "-c",
                "protocol.file.allow=always",
                "fetch",
                "-q",
                &s.gitdir.to_string_lossy(),
                "+HEAD:refs/omh/probe",
            ],
        )
        .unwrap();
        let objects = git_in(&checkout, &["rev-list", "--objects", "refs/omh/probe"]).unwrap();
        // Non-vacuity. An enumeration that returns nothing passes every
        // assertion below without looking at anything, and one typo in the
        // refspec is all that takes.
        assert!(
            !objects.trim().is_empty(),
            "the probe enumerated no objects, so it proved nothing"
        );
        let mut read_a_blob = false;
        for line in objects.lines() {
            let oid = line.split_whitespace().next().unwrap_or_default();
            // `unwrap`, not `unwrap_or_default`: an unreadable object would
            // otherwise pass this assertion as an empty string, which is the
            // vacuous pass this test exists to not be.
            let body = git_in(&checkout, &["cat-file", "-p", oid]).unwrap();
            read_a_blob |= body.contains("base");
            assert!(
                !body.contains("ghp_abc123def456"),
                "a fetch put the carried secret in the user's repository: {line}"
            );
        }
        assert!(
            read_a_blob,
            "the enumeration never read file content, so a leak in one would \
             not have been seen"
        );
    }

    /// Curation is the flag's headline behaviour and nothing executed it: every
    /// other test here passes `curate: false` while `--keep` only ever passes
    /// `true`. Deleting the `-i` left the suite green.
    ///
    /// A sequence editor that *edits* the todo, not one that accepts it. An
    /// editor that exits without touching the list is behaviourally identical
    /// to `-q`, so it proves the branch was taken and nothing about what it
    /// does — this drops a line, which is the whole point of opening the list.
    ///
    /// It also pins the count. Reported from the commits *fetched*, "kept 3"
    /// printed over a branch that got 1.
    #[test]
    fn curating_drops_what_the_user_drops_and_says_so() {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        // Separate files, so dropping the middle commit is a clean drop
        // rather than a conflict — the curation is what is under test here.
        for m in ["one", "two", "three"] {
            std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
            git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
            git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
        }

        // the user opens the list and deletes the middle pick
        let editor = d.path().join("drop-one.sh");
        std::fs::write(&editor, "#!/bin/sh\nsed -i.bak '2d' \"$1\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git_in(
            &checkout,
            &["config", "sequence.editor", &editor.to_string_lossy()],
        )
        .unwrap();

        let landed = s.harvest(&checkout, &wt, "omh/s01", &[], true).unwrap();

        let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
        let on_branch = log.lines().count();
        assert_eq!(on_branch, 2, "one of the three was dropped: {log}");
        assert_eq!(
            landed, on_branch,
            "the number reported is what landed, not what was offered: {log}"
        );
    }

    /// Two of the three states that made a harvest report success while leaving
    /// commits behind — measured against the replant that ran without them.
    /// The third, an interrupted rebase, has its own test below.
    #[test]
    fn a_harvest_refuses_a_history_it_cannot_see_all_of() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        let cp = |m: &str| {
            std::fs::write(wt.join("f.txt"), format!("{m}\n")).unwrap();
            git(&s.gitdir, &wt, &["commit", "-qam", m]).unwrap();
        };
        cp("first");
        cp("second");
        s.preflight(&wt).expect("a clean history harvests");

        // the agent looks at an old checkpoint and does not come back
        git(&s.gitdir, &wt, &["checkout", "-q", "HEAD~1"]).unwrap();
        let err = s.preflight(&wt).unwrap_err().to_string();
        assert!(err.contains("detached"), "{err}");
        assert!(err.contains(&s.branch), "say how to put it back: {err}");

        // a branch it made and wandered off
        git(&s.gitdir, &wt, &["checkout", "-q", &s.branch]).unwrap();
        git(&s.gitdir, &wt, &["branch", "aside", "HEAD"]).unwrap();
        git(&s.gitdir, &wt, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
        let err = s.preflight(&wt).unwrap_err().to_string();
        assert!(
            err.contains("no branch it is on can reach"),
            "stranded commits are the ones no other check sees: {err}"
        );
    }

    /// An interrupted rebase leaves HEAD on something that is neither the work
    /// nor a decision anyone made, and the marker is right there in the gitdir.
    #[test]
    fn a_harvest_refuses_a_repository_left_mid_rebase() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        std::fs::create_dir_all(s.gitdir.join("rebase-merge")).unwrap();

        let err = s.preflight(&wt).unwrap_err().to_string();
        assert!(
            err.contains("rebase-merge"),
            "name what is in progress: {err}"
        );
    }

    /// The seed is the fixed point a replant measures from, and it is recorded
    /// on the host precisely so the sandbox cannot move it.
    #[test]
    fn the_seed_survives_the_sandbox_deleting_everything_it_can_reach() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        let seed = s.seed().unwrap();

        let _ = git(&s.gitdir, &wt, &["tag", "-d", "session-start"]);
        let _ = std::fs::remove_dir_all(s.gitdir.join("refs/tags"));

        assert_eq!(s.seed().unwrap(), seed);
        assert!(!seed.is_empty());
    }

    /// Checkpointing is the feature, and a checkpoint is a commit, and git
    /// refuses to commit without an identity. The container carries no global
    /// git config, so unless the repository brings its own the agent's first
    /// `git commit` dies on `Author identity unknown` — the whole point of the
    /// module, unavailable, on a machine that has never been configured.
    ///
    /// Asserted as *the repository carries an identity*, not as "a commit
    /// works here", because a commit working here proves nothing. git invents
    /// an identity from the OS user and hostname when it can, so on a developer
    /// machine the commit succeeds with no config at all — deleting the two
    /// lines this guards left it green, while CI, whose runner has an empty
    /// gecos field, failed on `empty ident name`. A test that cannot go red on
    /// the machine you are writing it on is decoration.
    ///
    /// The commit is still made, because config that is set and not honoured
    /// would be its own bug.
    #[test]
    fn the_agent_can_commit_without_any_identity_of_its_own() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();

        // a file the seed already tracks, so `-a` stages it
        std::fs::write(wt.join("f.txt"), "base\nthe agent's work\n").unwrap();
        let out = Command::new("git")
            .current_dir(&wt)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .arg("--git-dir")
            .arg(&s.gitdir)
            .arg("--work-tree")
            .arg(&wt)
            .args(["commit", "-q", "-am", "a checkpoint"])
            .output()
            .unwrap();

        assert!(
            out.status.success(),
            "a sandbox with no git identity must still be able to check point: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        // The assertion that can actually fail here.
        for (key, want) in [("user.name", AUTHOR_NAME), ("user.email", AUTHOR_EMAIL)] {
            let got = git(&s.gitdir, &wt, &["config", "--local", key]).unwrap_or_default();
            assert_eq!(
                got.trim(),
                want,
                "the repository has to bring its own {key}; nothing in a \
                 container supplies one"
            );
        }
    }

    /// The exclude list follows the mounts, and the mounts change under it.
    ///
    /// omh derives what the sandbox's repository must not track from the mounts
    /// it is about to make, and wrote that list once — when the repository was
    /// created. Switch a capability on afterwards and the mount it adds inside
    /// `/work` is a file the existing sandbox neither tracks nor excludes, so
    /// the agent's own `git add -A` commits omh's rendered document — MCP
    /// environment and all — into a history `omh s commit --keep` replays onto
    /// the branch.
    ///
    /// Asserted as the property that matters rather than as a line in a file:
    /// the document cannot be staged. A test that greps `info/exclude` passes
    /// for a list that git never reads.
    #[test]
    fn a_capability_added_later_is_still_kept_out_of_the_sandboxs_history() {
        let (_d, wt, shadow_dir) = fixture();
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();

        std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A"]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-q", "-m", "a checkpoint"]).unwrap();
        let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

        // the next launch mounts one more document inside /work
        let grown = [".env".to_string(), ".mcp.json".to_string()];
        s.ensure(&wt, &grown).unwrap();

        // what the mount would put there, credentials and all
        std::fs::write(wt.join(".mcp.json"), "{\"env\":{\"TOKEN\":\"sk-live-42\"}}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        let staged = git(&s.gitdir, &wt, &["diff", "--cached", "--name-only"]).unwrap();

        assert!(
            !staged.contains(".mcp.json"),
            "a document omh mounted is not the agent's work to commit: staged {staged:?}"
        );
        assert_eq!(
            git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
            checkpoint,
            "and refreshing the list must not disturb what the agent already did"
        );
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
