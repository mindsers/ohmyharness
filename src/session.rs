//! Sessions are git worktrees on their own branch.
//!
//! This is the part that makes the sandbox real. The container protects your
//! *host*; the worktree protects your *repo*. Your working tree is never
//! mounted, so an agent cannot touch uncommitted work or `main` — review is a
//! plain `git diff`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether a thing this command is named for actually went.
///
/// **Not a bool.** `No` carries why, because a removal that did not happen and
/// a removal nobody could explain are the same news to a caller that only has
/// a flag — and this is the value `rm` prints to somebody deciding what to
/// clean up by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gone {
    Yes,
    No(String),
}

/// What a removal actually did, part by part.
///
/// One value rather than a tuple, because the parts are not independent: the
/// branch news is only as good as the worktree news beside it, and a caller
/// holding two loose values reads one without the other. Each field is the
/// answer to "is it gone", never "was it attempted".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// What became of `omh/<id>`.
    pub branch: Removed,
    /// Asked of the disk after the fact, not of git's exit code.
    pub worktree: Gone,
    /// The sandbox's own repository — the one holding commits nothing else
    /// points at, which is why its failure is worth a sentence.
    pub shadow: Gone,
}

/// What `remove` did with the branch, so the caller can report it truthfully
/// rather than always claiming the branch was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    /// Commits nobody has reviewed; the branch outlives the session.
    ///
    /// Carries the count that decided it, and `None` when git could not take
    /// one — a branch kept because it holds three commits is an invitation to
    /// review them, and one kept because omh could not tell is a question. The
    /// caller used to re-ask `commits` to tell them apart, which is two answers
    /// to one question and a window for them to disagree.
    BranchKept(Option<usize>),
    /// Nothing was committed, so the branch preserved nothing.
    BranchDropped,
    /// There was no branch to speak of.
    ///
    /// A scratch session (`omh auth`, `omh doctor`) never had one, and neither
    /// did a session id nothing ever created, which `omh s rm` reaches because
    /// it builds a session rather than looking one up. Reporting *kept* or
    /// *dropped* there is a claim about work that never existed.
    NoBranch,
}

/// What `commit` does about files omh copied in from the checkout.
///
/// `carry_in` is for what a worktree does not get — a tracked file is already
/// there, so listing one is a misconfiguration. It is also the only path by
/// which a secret reaches the agent, and the two compose badly: a tracked path
/// in the list arrives as an ordinary modification, because `carry`'s
/// `info/exclude` is gitignore semantics and says nothing about tracked files.
/// Committing it publishes whatever local edit the user was carrying.
///
/// `carry` warns at launch, where the mistake is made. This is the backstop for
/// a session already running when the list changed.
pub struct Carried<'a> {
    paths: &'a [String],
    skip: bool,
}

impl<'a> Carried<'a> {
    /// Stop and name them. The default, because dropping a change silently is
    /// the second silent behaviour, not the fix for the first.
    pub fn refusing(paths: &'a [String]) -> Self {
        Self { paths, skip: false }
    }

    /// Leave them out and commit the rest — `--skip-carried`. Without it a
    /// tracked carried file makes a session you can never commit from.
    pub fn skipping(paths: &'a [String]) -> Self {
        Self { paths, skip: true }
    }

    /// Carried paths that the staging step actually picked up. A path is
    /// normalised the way `carry` writes it, and a carried *directory* matches
    /// everything under it.
    fn staged_among<'s>(&self, staged: &'s str) -> Vec<&'s str> {
        staged
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter(|line| {
                self.paths.iter().any(|p| {
                    let p = p.trim().trim_end_matches('/');
                    *line == p || line.starts_with(&format!("{p}/"))
                })
            })
            .collect()
    }
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

/// Where a ref points, as a commit id.
pub fn head_of(repo: &Path, what: &str) -> Result<String> {
    Ok(git(repo, &["rev-parse", what])?.trim().to_string())
}

/// Point a ref at a tree, so a merge can label it with something readable.
///
/// `refs/<id>` rather than a namespace of omh's, because the name ends up
/// **inside somebody's source file**: measured, `merge-tree` labels each side
/// of a conflict with the string it was handed, so `refs/omh/sync/s01` would
/// put omh's plumbing in the marker an agent opens. `s01` is a session, which
/// is a word the user already has.
///
/// A ref pointing straight at a tree is unusual and git accepts it — measured,
/// including that the short name resolves. It is removed as soon as the merge
/// returns, and force-removed before the next one, the shape `harvest` uses
/// for its own scratch ref.
pub fn name_tree(repo: &Path, name: &str, tree: &str) -> Result<()> {
    let _ = git(repo, &["update-ref", "-d", name]);
    git(repo, &["update-ref", name, tree]).map(|_| ())
}

pub fn unname_tree(repo: &Path, name: &str) -> Result<()> {
    git(repo, &["update-ref", "-d", name]).map(|_| ())
}

/// What a three-way merge produced, and what it could not settle.
///
/// A tree either way: `merge-tree` writes one even when it conflicts, with the
/// markers already in the conflicted files. That is the whole reason this
/// mechanism works — a conflict is *text*, and text crosses into the sandbox
/// safely where a commit never could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub tree: String,
    /// Paths git could not merge, in the order it reported them.
    pub conflicted: Vec<String>,
}

/// Merge three trees in the user's own repository, touching nothing.
///
/// Measured 2026-08-23 against git 2.55.0, with a dirty worktree, to check the
/// claim this design rests on: `merge-tree --write-tree` writes the merged tree
/// into the object store and leaves the worktree, the index and every ref
/// exactly as they were. Clean merges exit 0 and print the tree; a conflict
/// exits 1 and prints the tree, then the conflicted stages, then messages.
/// Both are answers, so the status distinguishes them rather than failing.
///
/// **The labels are the argument strings.** Measured: passing the branch name
/// gives `<<<<<<< main`, and passing a bare tree gives its object id. The
/// design expected to be stuck with ids on both sides; naming the session's
/// side costs one scratch ref, and the marker an agent opens then reads
/// `>>>>>>> s01` instead of forty hex characters.
///
/// `-z` for the paths, for the reason `Session::changed` gives: git quotes a
/// path that needs it, and a name omh cannot open is a name it must not print.
pub fn merge_three(repo: &Path, base: &str, theirs: &str, ours: &str) -> Result<Merged> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "merge-tree",
            "--write-tree",
            "-z",
            "--merge-base",
            base,
            theirs,
            ours,
        ])
        .output()
        .context("running git merge-tree")?;
    // Not `success()`: exit 1 is a conflict, which is an answer. Anything else
    // — a git too old for `--write-tree`, an unreadable object — is not.
    let code = out.status.code();
    anyhow::ensure!(
        code == Some(0) || code == Some(1),
        "git merge-tree: {}",
        crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
    );

    let said = String::from_utf8_lossy(&out.stdout);
    let mut records = said.split('\0');
    let tree = records
        .next()
        .map(str::trim)
        .filter(|tree| !tree.is_empty())
        .context("git merge-tree wrote no tree")?
        .to_string();

    // Then one record per conflicted stage — `<mode> <oid> <stage>\t<path>` —
    // until an empty one. Three stages name the same path, so the same path
    // arrives up to three times.
    let mut conflicted: Vec<String> = Vec::new();
    for record in records {
        if record.is_empty() {
            break;
        }
        let Some((_, path)) = record.split_once('\t') else {
            continue;
        };
        if !conflicted.iter().any(|seen| seen == path) {
            conflicted.push(path.to_string());
        }
    }
    Ok(Merged { tree, conflicted })
}

/// Which of the two answers a review wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The shape of the change: which files, how much.
    Summary,
    /// The change itself.
    Patch,
}

impl What {
    pub(crate) fn flag(self) -> &'static str {
        match self {
            What::Summary => "--stat",
            What::Patch => "-p",
        }
    }
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
        //
        // The start point is a **commit**, never the name it came from, and
        // that is the whole correctness of the second arm. `worktree add -b
        // <branch> <path> <base>` looks unambiguous and is not: when `base` has
        // no local ref but exactly one remote has it, git's DWIM takes over,
        // creates a branch named after the base, and ignores `-b`. Measured
        // against git 2.55.0 — the session landed on `main`, tracking
        // `origin/main`, which is the one branch the worktree exists to keep an
        // agent away from. `--no-guess-remote` does not switch it off; a
        // resolved commit leaves nothing to guess.
        // Resolved inside this arm and nowhere else. Resuming reattaches to a
        // branch that already exists and asks git nothing about `base`, so
        // taking the start point first made a session that was resumable
        // yesterday fail today because the trunk was renamed in between — and
        // fail talking about a base the command that ran never reads.
        // `Ok(true)` and not "not an error": an unanswerable question falls to
        // the create path exactly as it did before, and git refuses loudly
        // either way. Only `remove` needs the third answer, because only
        // `remove` reports on it.
        let args: Vec<String> = if matches!(self.branch_exists(repo), Ok(true)) {
            ["worktree", "add", &path, &branch]
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            // Explicit base: without it git branches from whatever HEAD is,
            // which produces a session whose diff has the wrong baseline.
            vec![
                "worktree".into(),
                "add".into(),
                "-b".into(),
                branch.clone(),
                path.clone(),
                start_point(repo, base)?,
            ]
        };
        git_owned(repo, &args)
            .with_context(|| format!("creating worktree for session {}", self.id))?;

        // Asked rather than assumed. Every review path already refuses a
        // worktree that is not on its branch, and each of them refuses *later*,
        // after an agent has worked in it. This is the same question at the one
        // moment the answer is still free.
        //
        // Unreachable as the code stands — a resolved commit leaves git nothing
        // to guess with, and the resume arm names a branch `branch_exists` just
        // confirmed. It is here for the next person who passes a name to
        // `worktree add`, which is how this broke the first time.
        //
        // **And it takes the worktree with it.** `ensure` opens by treating an
        // existing directory as a finished session, so a failure that left one
        // behind would be waved through by the very next launch: the session
        // omh refused to hand back once would be handed back silently, on the
        // wrong branch, which is worse than never having checked.
        let head = git(&self.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let wrong = match &head {
            Ok(head) => head.trim() != branch,
            Err(_) => true,
        };
        if wrong {
            let _ = git(repo, &["worktree", "remove", "--force", &path]);
            let head = head?;
            anyhow::bail!(
                "git put {} on {} rather than {branch}; omh will not hand back a session \
                 it cannot review",
                self.id,
                head.trim()
            );
        }
        Ok(())
    }

    /// Whether `omh/<id>` is there — with "git could not answer" kept apart
    /// from "there is no branch".
    ///
    /// `.is_ok()` on the whole command could not tell an absent branch from a
    /// git that would not run or a path that is not a repository. Both read as
    /// "no branch", and a branch holding unreviewed work then dropped out of
    /// the report entirely; the caller falls the safe way on `Err` instead.
    ///
    /// **The third answer is narrower than it looks, and this is measured.**
    /// An *unreadable ref directory* is indistinguishable from an absent ref
    /// at git's own level: `show-ref --verify` says `not a valid ref` for
    /// both, `rev-parse --verify` says `Needed a single revision` for both,
    /// and `for-each-ref` returns empty for both. So `Ok(false)` here means
    /// "git says there is no such ref", which is not quite "there is no such
    /// ref". What `Err` catches is the layer above: no repository, or a git
    /// that would not run at all.
    ///
    /// Without `--quiet`, because a suppressed diagnostic makes the error arm
    /// carry no reason to print.
    fn branch_exists(&self, repo: &Path) -> Result<bool, String> {
        let Some(branch) = &self.branch else {
            return Ok(false);
        };
        match git(
            repo,
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        ) {
            Ok(_) => Ok(true),
            // `show-ref --verify` exits 1 with nothing on stdout for a ref
            // that is simply not there, which is the ordinary answer.
            Err(e) => {
                let why = format!("{e:#}");
                if why.contains("not a valid ref") || why.trim().ends_with(':') {
                    Ok(false)
                } else {
                    Err(why)
                }
            }
        }
    }

    /// How many commits `base` has that this session does not. A session that
    /// silently drifts behind trunk makes the agent work against stale code.
    ///
    /// `Ok(0)` and *cannot tell* are different answers, for the reason
    /// `commits` below is a `Result`: this asks git the same question, in the
    /// same checkouts, and fails in the same ones. Nothing destructive reads
    /// it — but it is rendered beside a column that now says `?` for exactly
    /// this failure, and it was emitted into JSON as `"behind": 0`, which is a
    /// number omh did not have.
    pub fn behind(&self, repo: &Path, base: &str) -> Result<usize> {
        let Some(branch) = &self.branch else {
            return Ok(0);
        };
        let out = git(repo, &["rev-list", "--count", &format!("{branch}..{base}")])?;
        out.trim()
            .parse()
            .with_context(|| format!("counting how far {branch} is behind {base}"))
    }

    /// Commits on this session's branch that are not already in `base`.
    ///
    /// The question `remove` needs answered: whether keeping the branch would
    /// preserve anything.
    ///
    /// **An error is never zero.** A git that would not answer is not a git
    /// that answered *none*, and this used to break that with more at stake
    /// than most, because the caller acts on the answer by deleting a branch.
    ///
    /// The rule was stated on `uncommitted` until #59 removed that method, at
    /// which point this citation pointed at nothing — so it is stated here,
    /// where it is relied on. `changed` carries it too: it returns the paths
    /// or the failure, never an empty list standing in for both.
    ///
    /// `rev-list <base>..<branch>` fails whenever either end does not resolve.
    /// The base end is the reachable one for a live session: a repo that
    /// renamed its trunk, or a clone whose `main` exists only as
    /// `origin/main`, which `default_branch` will happily name. Read as zero,
    /// that failure spelled *no commits*, and `omh s rm` deleted a branch
    /// holding work nobody had reviewed while reporting it had preserved
    /// nothing.
    pub fn commits(&self, repo: &Path, base: &str) -> Result<usize> {
        let Some(branch) = &self.branch else {
            return Ok(0);
        };
        let out = git(repo, &["rev-list", "--count", &format!("{base}..{branch}")])?;
        out.trim()
            .parse()
            .with_context(|| format!("counting commits on {branch}"))
    }

    /// Remove the session: its worktree, the repository the sandbox had, and
    /// the branch only when it holds nothing to review.
    ///
    /// The shadow goes because session ids come back around — `next_id` is the
    /// highest `sNN` among the worktrees plus one — and a shadow that outlives
    /// its session is adopted by whoever inherits the name, opening a new agent
    /// on someone else's history against a seed naming a tree it never had.
    ///
    /// Taken as an argument rather than reached for, so that removing a session
    /// and forgetting its shadow is not something a caller can express.
    pub fn remove(&self, repo: &Path, base: &str, shadows: &Path) -> Result<Removal> {
        // Decided before the worktree goes, because afterwards the branch is
        // the only thing left to ask.
        //
        // A question git could not answer keeps the branch. Dropping one is
        // irreversible and justified by exactly one fact — that it holds
        // nothing — so anything short of that fact has to fall the other way.
        //
        // Asked of a branch that exists, and that test is not a formality:
        // `rev-list <base>..<branch>` fails for a missing *branch* exactly as
        // it does for a missing base, so an id nothing ever created answered
        // "cannot count" and was reported as a branch kept — over a branch that
        // was never there, with a `git log` line that fails the same way.
        let outcome = match &self.branch {
            None => Removed::NoBranch,
            // A question git could not answer keeps the branch, for the same
            // reason a count it could not make does.
            Some(_) => match self.branch_exists(repo) {
                Err(_) => Removed::BranchKept(None),
                Ok(false) => Removed::NoBranch,
                Ok(true) => match self.commits(repo, base) {
                    Ok(0) => Removed::BranchDropped,
                    Ok(n) => Removed::BranchKept(Some(n)),
                    Err(_) => Removed::BranchKept(None),
                },
            },
        };

        // **Asked of the disk, not of git's exit code.** The fallback used to
        // run only when `git worktree remove` reported failure — so a git that
        // exited 0 while leaving the directory behind (files it could not
        // delete, an owner it is not) was taken at its word, and `rm` reported
        // a removed session over a worktree still on disk. Nothing verified
        // the thing this function is named for actually happened.
        let _ = git(
            repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.worktree.to_string_lossy(),
            ],
        );
        // git can also lose the registration while the directory survives, and
        // then refuses with "is not a working tree" — leaving a session that
        // can never be removed. Either way, what matters is whether it is gone.
        //
        // Asked with `still_there` and not `Path::exists`, for the same reason
        // again one layer down: `exists` is `metadata().is_ok()`, so a parent
        // that cannot be traversed answered "absent" and this reported a
        // removal it never performed. Every step below re-asks rather than
        // trusting a return value.
        let worktree = match still_there(&self.worktree) {
            Ok(false) => Gone::Yes,
            Err(why) => Gone::No(format!("omh could not tell whether it is there: {why}")),
            Ok(true) => {
                let why = std::fs::remove_dir_all(&self.worktree).err();
                match still_there(&self.worktree) {
                    Ok(false) => Gone::Yes,
                    Err(e) => Gone::No(format!("omh could not tell whether it is there: {e}")),
                    // `remove_dir_all` reporting success over a directory that
                    // is still there is the same class of claim as git's exit
                    // code, so it gets the same treatment.
                    Ok(true) => Gone::No(match why {
                        Some(e) => format!("{e}"),
                        None => "it is still there".to_string(),
                    }),
                }
            }
        };
        let _ = git(repo, &["worktree", "prune"]);

        // A branch carrying commits outlives its worktree on purpose: removing
        // a session must never destroy work nobody has reviewed. A branch
        // carrying none holds nothing to review — `--force` above has already
        // discarded anything uncommitted — so keeping it only leaves a dead ref
        // behind after every abandoned session.
        let mut outcome = outcome;
        if outcome == Removed::BranchDropped {
            if let Some(branch) = &self.branch {
                let _ = git(repo, &["branch", "-D", branch]);
                // **Asked of git, not assumed** — the same rule the worktree
                // above follows. `branch -D` refuses over an unwritable ref
                // store, a stale lock, or a registration that outlived its
                // directory, and the variant was decided before any of that
                // could be known. Reporting a dropped branch that is still
                // there fills `omh/` with refs omh has said are gone.
                if !matches!(self.branch_exists(repo), Ok(false)) {
                    outcome = Removed::BranchKept(None);
                }
            }
        }

        // Unconditionally, and unlike the branch. A branch can hold commits
        // nobody has reviewed, which is why it survives.
        //
        // The shadow holds the agent's own checkpoints, and for its *tip* the
        // content is already in the worktree the line above discarded. That is
        // not true of the ones behind it: after a `git reset --hard` — one of
        // the four commands this whole feature exists to restore — the
        // pre-rollback checkpoints live nowhere else.
        //
        // This still destroys them without a word, and that is now the *last*
        // step of a command that refuses to reach it. `may_remove` (#58) counts
        // what this repository holds that no branch has — `--all --reflog`, so
        // a rollback is exactly the case it sees — and stops before anything
        // is taken down unless the user said `--force`. The silence here is
        // the silence of a decision already made upstairs.
        let shadow = crate::shadow::Shadow::new(shadows, &self.id).reap();
        Ok(Removal {
            branch: outcome,
            worktree,
            shadow,
        })
    }

    /// What this session changed, against the point it forked from.
    ///
    /// Against the **working tree**, not the branch tip, because the agent's
    /// commits are not this branch's — they go to the sandbox's own repository,
    /// and reach here only when the user asks with `omh s commit --keep`. `base...branch` answered with an
    /// empty string for the whole span of a session, right up until the user
    /// ran `omh s commit`, while the rules told the agent the user reviews
    /// before committing. A review command that is silent about the work it
    /// exists to show is worse than one that does not exist.
    ///
    /// The merge base, not `base` itself, so trunk moving under a running
    /// session cannot manufacture changes it never made — the property the old
    /// three-dot form had and this has to keep.
    ///
    /// Through a throwaway index, because `git diff <commit>` reports tracked
    /// paths only and a file the agent *created* is untracked until somebody
    /// stages it — which is most of what there is to review. `add -A` against
    /// `GIT_INDEX_FILE` gets the whole worktree without touching the index the
    /// user's own git commands share.
    pub fn diff(&self, base: &str, what: What) -> Result<String> {
        self.reviewing(base, |worktree, index, merge_base| {
            git_with_index(
                worktree,
                index,
                &["diff", "--cached", what.flag(), merge_base],
            )
        })
    }

    /// How many commits one point is ahead of another.
    ///
    /// For saying *trunk moved 12 commits*, which is the number a person wants
    /// before they read anything else.
    pub fn commits_between(&self, repo: &Path, from: &str, to: &str) -> Result<usize> {
        git(repo, &["rev-list", "--count", &format!("{from}..{to}")])?
            .trim()
            .parse()
            .with_context(|| format!("counting what arrived between {from} and {to}"))
    }

    /// The session's files as a tree object, ready to be merged against.
    ///
    /// Built from the same throwaway index a review is, so what `sync` merges
    /// is what `diff` would have shown — omh's own mounted paths out, the
    /// agent's work in. Nothing is committed and no ref moves; a tree is just
    /// bytes in the object store until something points at it.
    pub fn tree(&self, base: &str) -> Result<String> {
        self.reviewing(base, |worktree, index, _| {
            Ok(git_with_index(worktree, index, &["write-tree"])?
                .trim()
                .to_string())
        })
    }

    /// Put a merged tree's files in the worktree, leaving omh's own alone.
    ///
    /// Through a throwaway index again, so the session's real index is the
    /// baseline step's business and nothing here half-moves it. `checkout-index
    /// -f -a` writes every path the tree holds.
    ///
    /// `STAGED_RULES` are omh's, not the project's: `reviewing` unstages them
    /// so they never reach the tree from the session's side, but trunk may
    /// carry a real `AGENTS.md` of its own — and the session's copy is a
    /// placeholder omh mounts over on the next launch. Writing trunk's file
    /// there would put the project's rules where omh is about to mount, and
    /// the agent would read whichever won.
    ///
    /// **And removes what the merge removed.** `checkout-index -a` writes
    /// every path the tree holds and touches nothing else, so a file trunk
    /// deleted stayed in the worktree: `diff` then showed the session adding
    /// it back and the next `commit` would have landed it as the agent's.
    /// The paths to unlink are exactly those in `was` — the session's tree
    /// before the merge — and not in `tree` after it. A file the session
    /// changed and trunk deleted is a conflict, and `merge-tree` keeps the
    /// changed version in the tree, so it is never on this list: omh does
    /// not settle a conflict in trunk's favour by deleting the evidence.
    ///
    /// Three more never leave, whatever the trees say. omh's placeholders,
    /// because they are omh's and not the project's. A path git ignores in
    /// this worktree — a carried `.env` is excluded by `carry::apply`, and
    /// a tracked file is never reported ignored — because an ignored file
    /// is not one trunk had to delete. And nothing outside the worktree,
    /// because a path is a string git handed back and the worktree is the
    /// only directory this has any business in — refused rather than
    /// joined, though `diff-tree` writes relative paths only.
    pub fn materialise(&self, was: &str, tree: &str) -> Result<()> {
        let index = tempfile::NamedTempFile::new().context("staging a merge index")?;
        let index = index.path();
        git_with_index(&self.worktree, index, &["read-tree", tree])?;
        let unstage = unstage_rules_args();
        let unstage: Vec<&str> = unstage.iter().map(String::as_str).collect();
        git_with_index(&self.worktree, index, &unstage)?;
        git_with_index(&self.worktree, index, &["checkout-index", "-f", "-a"])?;

        let gone = git(
            &self.worktree,
            &[
                "diff-tree",
                "-r",
                "--name-only",
                "--diff-filter=D",
                "-z",
                was,
                tree,
            ],
        )?;
        let hidden = crate::carry::hidden_in_the_worktree();
        for path in gone.split(' ').filter(|p| !p.is_empty()) {
            let rel = Path::new(path);
            anyhow::ensure!(
                rel.is_relative()
                    && rel
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "refusing to remove `{}`: not a plain path inside the session",
                crate::out::untrusted(path)
            );
            if hidden.contains(&path) || ignored_here(&self.worktree, path)? {
                continue;
            }
            let at = self.worktree.join(rel);
            match std::fs::remove_file(&at) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("removing {} — trunk deleted it", at.display()))
                }
            }
            // The directories it leaves empty, up to the worktree, the way a
            // checkout would. Best effort: a directory holding anything else
            // is not empty and `remove_dir` refuses it, which is the answer.
            let mut dir = at.parent();
            while let Some(d) = dir {
                if d == self.worktree || std::fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        }
        Ok(())
    }

    /// Move what `diff` measures against, so a sync does not read as the
    /// agent's work.
    ///
    /// Without this the session's branch still points at the old trunk, and
    /// every review from here on shows trunk's changes as though the agent had
    /// made them — which is the same lie a harvest would then replay onto the
    /// branch.
    ///
    /// `--create-reflog` and an expected old value, because this is a ref move
    /// on the user's own branch: if anything else moved it since the sync
    /// began, omh refuses rather than winning the race. Commits the branch
    /// already carries are the user's, so they are replayed onto the new base
    /// first, in a scratch worktree — and a conflict there refuses, since
    /// resolving someone's committed work is not omh's to do.
    pub fn move_baseline(&self, repo: &Path, onto: &str, was: &str) -> Result<()> {
        let branch = self
            .branch
            .as_deref()
            .context("a scratch session has no branch to move")?;

        // Commits the branch already carries are the user's — `omh sNN commit`
        // put them there. Pointing the branch at the new base would delete
        // them, so they are replayed onto it first and a conflict refuses:
        // resolving someone's committed work is not omh's to do, and the
        // sandbox's files are not at risk either way because nothing has been
        // written yet when this runs.
        let mine = git(repo, &["rev-list", "--count", &format!("{was}..{branch}")])?
            .trim()
            .parse::<usize>()
            .with_context(|| format!("counting what {branch} carries beyond its base"))?;
        let tip = match mine {
            0 => onto.to_string(),
            _ => {
                let common = git(repo, &["rev-parse", "--git-common-dir"])?;
                let scratch = repo
                    .join(common.trim())
                    .join(format!("omh-sync-{}", self.id));
                let _ = git(
                    repo,
                    &["worktree", "remove", "--force", &scratch.to_string_lossy()],
                );
                git(
                    repo,
                    &[
                        "worktree",
                        "add",
                        "-q",
                        "--detach",
                        &scratch.to_string_lossy(),
                        branch,
                    ],
                )?;
                let replayed = git(&scratch, &["rebase", "-q", "--onto", onto, was])
                    .and_then(|_| git(&scratch, &["rev-parse", "HEAD"]))
                    .map(|tip| tip.trim().to_string())
                    .with_context(|| {
                        format!(
                            "{branch} carries {mine} commit{} of your own and they do not \
                             replay onto the new base cleanly. Nothing was changed — resolve \
                             it yourself with `git -C {} rebase --onto {onto} {was} {branch}`",
                            if mine == 1 { "" } else { "s" },
                            repo.display()
                        )
                    });
                let _ = git(
                    repo,
                    &["worktree", "remove", "--force", &scratch.to_string_lossy()],
                );
                replayed?
            }
        };

        git(
            repo,
            &[
                "update-ref",
                "--create-reflog",
                &format!("refs/heads/{branch}"),
                &tip,
                was,
            ],
        )
        .with_context(|| {
            format!("{branch} moved while omh was syncing — nothing was changed on it")
        })?;
        // The worktree's files are already the merged ones; this makes the
        // index agree with the branch it now sits on, so `status` shows the
        // conflicts and nothing else.
        git(&self.worktree, &["reset", "-q", "--mixed"])?;
        Ok(())
    }

    /// The patch, on the terminal, through the user's own pager.
    ///
    /// Inherited stdio rather than captured output: git then does its own
    /// paging and colouring, which is the pattern `commit`'s editor path
    /// already uses, and reimplementing either is how a diff ends up subtly
    /// unlike every other diff the user reads.
    pub fn stream_diff(&self, base: &str, colour: &str) -> Result<()> {
        self.reviewing(base, |worktree, index, merge_base| {
            let status = Command::new("git")
                .current_dir(worktree)
                .env("GIT_INDEX_FILE", index)
                .args([
                    "diff",
                    "--cached",
                    "-p",
                    &format!("--color={colour}"),
                    merge_base,
                ])
                .status()
                .context("running git diff")?;
            // `git diff` exits 1 for "there were differences" only with
            // `--exit-code`, which nothing here passes, so a non-zero status is
            // a real failure and not the ordinary answer. Measured on a pty:
            // a pager that quits early — `q` in less, or `| head` — still
            // leaves git at 0, so this does not misfire on an ordinary read.
            anyhow::ensure!(status.success(), "git diff exited {status}");
            Ok(())
        })
    }

    /// The index a review is taken from, and the point it is taken against.
    ///
    /// **One setup, not two.** A summary, a patch and a paged patch are three
    /// answers to one question, and the first version of `-p` built its own
    /// copy of this — seven steps, faithfully reproduced except for the guard
    /// below, which it dropped. `omh sNN diff` then refused on a detached
    /// worktree and `omh sNN diff -p` printed an empty patch and exited 0,
    /// one flag apart. Four reviewers found it independently. Passing the
    /// index to a closure is what makes the guard impossible to skip rather
    /// than merely present in two places today.
    fn reviewing<T>(
        &self,
        base: &str,
        take: impl FnOnce(&Path, &Path, &str) -> Result<T>,
    ) -> Result<T> {
        let branch = self
            .branch
            .as_deref()
            .context("a scratch session has no branch")?;

        // The same guard `commit` carries, for the same reason and with more at
        // stake. A worktree left detached — `git checkout <sha>` to look at
        // something, a bisect abandoned halfway — no longer has the branch's
        // committed work *on disk*, and a diff taken from the worktree reports
        // what is there. So the session's own commits go missing from the
        // review and the command still exits 0.
        //
        // Answering against the branch instead does not rescue it: `checkout`
        // removed the files, `add -A` reports the worktree it was given, and
        // both forms print the same short answer. Measured, rather than argued
        // — the choice of ref is not what is wrong, the worktree is. So this
        // refuses instead, and a review that cannot be trusted is never handed
        // over as one that can.
        let head = git(&self.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        anyhow::ensure!(
            head.trim() == branch,
            "{} is on {} rather than {branch}; omh will not report a diff from a worktree that \
             left its branch — the session's commits are not in it. Put it back with `git -C {} \
             checkout {branch}`",
            self.id,
            head.trim(),
            self.worktree.display()
        );

        let merge_base = git(&self.worktree, &["merge-base", base, branch])?;
        let merge_base = merge_base.trim();

        let index = tempfile::NamedTempFile::new().context("staging a diff index")?;
        // `read-tree HEAD` first, for two reasons neither of which is obvious
        // and both of which were measured. It is not cosmetic: an earlier
        // comment here claimed an empty index would report every path as added,
        // which is false — `diff --cached` compares blob hashes, so unchanged
        // paths produce no diff either way, and a maintainer who tested that
        // claim would have deleted the line.
        //
        // One: `NamedTempFile` leaves a **zero-byte** file, and git rejects one
        // as an index — `fatal: index file smaller than expected`. A path that
        // does not exist would be fine; the one we are handed is not.
        //
        // Two, and the reason the guard below exists: `add -A` skips a path
        // `info/exclude` names, so against an empty index that path is absent
        // and the diff calls it a deletion — a file reported as removed while
        // it sits on disk. `carry` writes that exclude file, and a `carry_in`
        // entry naming an already-tracked path is a misconfiguration it warns
        // about rather than refuses, so the case is reachable.
        let index = index.path();
        git_with_index(&self.worktree, index, &["read-tree", "HEAD"])?;
        git_with_index(&self.worktree, index, &["add", "-A", "."])?;

        // The same unstage `commit` does, from the same list, because the
        // reason is the same: omh mounts its rules over a placeholder it writes
        // into the worktree, and `info/exclude` cannot hide that placeholder
        // when the repo tracks the filename. Reading the worktree meant a
        // review opened with `AGENTS.md | 1 -` — omh's own scaffolding shown as
        // the agent emptying the project's rules, in this repo among others.
        //
        // Reading one list is the point: a review that showed what a commit
        // would not carry is a review of something nobody is going to get.
        let unstage = unstage_rules_args();
        let unstage: Vec<&str> = unstage.iter().map(String::as_str).collect();
        git_with_index(&self.worktree, index, &unstage)?;

        take(&self.worktree, index, merge_base)
    }

    /// Stage the agent's work in the worktree and commit it onto the branch.
    ///
    /// Runs in the worktree rather than the checkout. On the host its `.git`
    /// pointer resolves, which is the whole reason this can be a plain git call
    /// — inside the sandbox that pointer leads nowhere and none of this works.
    pub fn commit(&self, message: Option<&str>, carried: Carried) -> Result<()> {
        let branch = self
            .branch
            .as_deref()
            .context("a scratch session has no branch to commit to")?;

        // The worktree is where the agent works, but nothing guarantees it is
        // still on the branch this session is named for — `worktree add -b` is
        // overridden by git's DWIM when the base exists only as `origin/<base>`,
        // and a session can be left detached mid-rebase. Committing anyway puts
        // the work on whatever HEAD happens to be, which has been `main`, and
        // reports the branch it did not touch.
        let head = git(&self.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        anyhow::ensure!(
            head.trim() == branch,
            "{} is on {} rather than {branch}; omh will not commit to a branch it did not open",
            self.id,
            head.trim()
        );

        git(&self.worktree, &["add", "-A", "."])?;

        // A backstop, not the fix. The rules are mounted rather than written
        // into the worktree, so on a healthy launch there is nothing here to
        // take back — but a bind mount's destination has to exist, and whether
        // the runtime creates that placeholder inside `/work` is unverified
        // against a real container. Cheap insurance against the answer being
        // "yes", and against a backend that cannot mount a single file.
        //
        // An unstage rather than an `:(exclude)` pathspec: naming a path that
        // way still counts as naming it, and git answers "the following paths
        // are ignored by one of your .gitignore files" about the very file the
        // pathspec was written to leave alone.
        git_owned(&self.worktree, &unstage_rules_args())?;

        // Asked *after* staging, and against the index rather than the worktree:
        // `git diff` says nothing about untracked files, so a session whose only
        // work is new files reads as clean when the question comes first. That
        // is the shape of `e0a41b8`, where a release published an empty tap and
        // reported success.
        let staged = git(&self.worktree, &["diff", "--cached", "--name-only"])?;

        // A carried file only reaches the index when the repo tracks it, and
        // then it is the user's own local edit — possibly the secret they were
        // carrying. Refused rather than dropped: omh cannot tell a credential
        // from a deliberate change, and silently discarding either is worse
        // than stopping.
        let from_your_checkout = carried.staged_among(&staged);
        if !from_your_checkout.is_empty() {
            anyhow::ensure!(
                carried.skip,
                "{} is listed in carry_in and git tracks it, so what is in the worktree \
                 is your local copy rather than the branch's.\n  omh will neither publish \
                 that nor drop it silently.\n\n  \
                 fix the cause:  omh set carry_in        (carry_in is for files git does not \
                 track; a tracked file is already in the worktree)\n  \
                 or just this once:  omh s commit --skip-carried",
                from_your_checkout.join(", ")
            );
            let unstage: Vec<String> = ["reset", "-q", "--"]
                .iter()
                .map(|s| s.to_string())
                .chain(from_your_checkout.iter().map(|p| p.to_string()))
                .collect();
            git_owned(&self.worktree, &unstage)?;
        }

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

    /// The paths behind that count.
    ///
    /// The same `status` the count runs, kept rather than discarded: `omh s`
    /// asks this question once per session already, and two sessions changing
    /// one file is the collision git will not mention until a merge.
    ///
    /// **`-z`, so nothing has to be un-quoted.** The first version parsed the
    /// human format and got three things wrong, all measured against git
    /// 2.55.0: git C-quotes any path needing it, so `café.rs` arrived as
    /// `caf\303\251.rs` and was printed at a user that way; `trim_matches('"')`
    /// strips *every* leading and trailing quote, so a file named `lead"` came
    /// back as `lead\`; and the ` -> ` split ran on every line rather than only
    /// on renames, so an ordinary file named `a -> b.rs` was reported as
    /// `b.rs` — a path that does not exist, and a collision with any session
    /// genuinely touching `b.rs`.
    ///
    /// `-z` emits NUL-separated records and never quotes. A rename is two
    /// records — the new name, then the old — so the old one is skipped by
    /// looking at the status rather than at the path.
    ///
    /// **An error is never an empty list**, the rule `commits` relies on a few
    /// hundred lines up: a git that would not answer is not a session that
    /// changed nothing, and every caller here renders those differently.
    pub fn changed(&self) -> Result<Vec<String>> {
        let out = git_owned(&self.worktree, &status_args())?;
        let mut records = out.split('\0').filter(|r| !r.is_empty());
        let mut changed = Vec::new();
        while let Some(record) = records.next() {
            // `XY ` then the path. Two ASCII status characters and a space, so
            // byte 3 is always a character boundary.
            let Some((status, path)) = record.split_at_checked(3) else {
                continue;
            };
            // A rename or a copy carries the name it came from as the next
            // record. That name is not on disk, so it is not something another
            // session can be changing.
            if status.contains('R') || status.contains('C') {
                records.next();
            }
            changed.push(path.to_string());
        }
        Ok(changed)
    }

    /// The branch on origin this session has already been pushed to.
    ///
    /// Read from `branch.<b>.remote`/`.merge` rather than `@{u}`, which resolves
    /// against **HEAD**: a detached worktree would report no upstream for a
    /// branch that demonstrably has one. Anything tracking a remote that is not
    /// origin is an error rather than a name, because reusing it would push to a
    /// different remote than the one it came from.
    pub fn published_as(&self) -> Result<Option<String>> {
        let Some(branch) = self.branch.as_deref() else {
            return Ok(None);
        };
        // `config --get` exits non-zero when unset, which is the common case.
        let remote = git(
            &self.worktree,
            &["config", "--get", &format!("branch.{branch}.remote")],
        )
        .unwrap_or_default();
        let remote = remote.trim();
        if remote.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(
            remote == "origin",
            "{branch} tracks {remote}, not origin — name the branch explicitly:\n  omh s push <name>"
        );
        let merge = git(
            &self.worktree,
            &["config", "--get", &format!("branch.{branch}.merge")],
        )
        .unwrap_or_default();
        Ok(merge
            .trim()
            .strip_prefix("refs/heads/")
            .filter(|name| !name.is_empty())
            .map(str::to_string))
    }

    /// Commits this session has that origin does not.
    ///
    /// `Ok(None)` means it has never been pushed, which is a different state
    /// from zero: one says name it and push, the other says you are done. `Err`
    /// is a third — git could not answer — and the caller must not render it as
    /// either of the first two.
    pub fn unpushed(&self) -> Result<Option<usize>> {
        let Some(branch) = self.branch.as_deref() else {
            return Ok(None);
        };
        let Some(target) = self.published_as()? else {
            return Ok(None);
        };
        let out = git(
            &self.worktree,
            &[
                "rev-list",
                "--count",
                &format!("refs/remotes/origin/{target}..{branch}"),
            ],
        )?;
        Ok(Some(
            out.trim().parse().context("counting unpushed commits")?,
        ))
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

        let target = match name {
            Some(name) => name.to_string(),
            None => self.published_as()?.with_context(|| {
                format!(
                    "{branch} is a session id, not a branch name\n  name it:  omh s push <name>"
                )
            })?,
        };

        // No `-u` here. Recording the upstream is what makes `omh s` report the
        // branch as published, and doing it in the same breath as the push means
        // a push that never reached origin still leaves that claim behind, with
        // nothing to roll it back. Set it below, once there is something true to
        // record.
        git(
            &self.worktree,
            &["push", "origin", &format!("{branch}:refs/heads/{target}")],
        )?;

        // Read it back from origin before calling this a success. `git push`
        // reports on the push URL, which need not be the fetch URL a reviewer
        // opens the PR from — the same green-and-wrong shape as `e0a41b8`, where
        // a release job passed while the tap it published to stayed empty.
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

        git(
            &self.worktree,
            &[
                "branch",
                "--set-upstream-to",
                &format!("origin/{target}"),
                branch,
            ],
        )?;
        Ok(target)
    }
}

/// The files omh wrote into the worktree, kept out of the user's work.
///
/// `carry`'s `info/exclude` covers these only while they are untracked, and a
/// repo that commits its own `CLAUDE.md` — normal for one whose users run agent
/// harnesses — has omh's copy written over a tracked file, which gitignore
/// semantics say nothing about. Without this, omh's generated rules land on top
/// of the project's own conventions in the user's commit, and a session where
/// the agent did nothing still looks like it has work in it.
fn rules_pathspec() -> Vec<String> {
    std::iter::once(".".to_string())
        .chain(
            crate::carry::STAGED_RULES
                .iter()
                .map(|name| format!(":(exclude){name}")),
        )
        .collect()
}

fn unstage_rules_args() -> Vec<String> {
    ["reset", "-q", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(crate::carry::STAGED_RULES.iter().map(|n| n.to_string()))
        .collect()
}

fn status_args() -> Vec<String> {
    ["status", "--porcelain", "-z", "-uall", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(rules_pathspec())
        .collect()
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

/// The commit a base name means here.
///
/// `main` and `origin/main` are one branch to a person and two refs to git, and
/// a checkout can easily have only the second — a clone made with `--branch`,
/// or one whose local trunk was deleted after a merge. Both are tried, in that
/// order, because a local branch is the one the user can move and therefore the
/// one they mean.
///
/// Returns the commit rather than the name that found it. Every caller wants a
/// fixed point: a name is re-resolved by whatever git command receives it, and
/// that is exactly how `worktree add -b` ends up guessing.
fn start_point(repo: &Path, base: &str) -> Result<String> {
    // Two failures wear this message, and it names both rather than picking the
    // likelier one. `rev-parse --quiet` exits non-zero for a ref that is not
    // there *and* for a checkout git cannot read at all, so a confident "no
    // such branch" would send someone hunting a spelling mistake while their
    // object store is the thing that is broken.
    let resolved = resolvable(repo, base).with_context(|| {
        format!("cannot resolve {base} here — no branch, tag or commit by that name, or a checkout git could not read")
    })?;
    Ok(git(
        repo,
        &["rev-parse", "--verify", &format!("{resolved}^{{commit}}")],
    )?
    .trim()
    .to_string())
}

/// The spelling of `base` that git can answer about here, if either can.
fn resolvable(repo: &Path, base: &str) -> Option<String> {
    [base.to_string(), format!("origin/{base}")]
        .into_iter()
        .find(|name| {
            git(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{name}^{{commit}}"),
                ],
            )
            .is_ok()
        })
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
            // The spelling that resolves, not the bare name. What comes back
            // here is handed straight to `rev-list`, `merge-base` and
            // `worktree add` by every caller, and in a checkout whose local
            // trunk is absent — a clone made with `--branch`, or one whose
            // `main` was deleted after a merge — the bare name resolves to
            // nothing. `s diff` could then take no merge base and `commits`
            // could take no count, which is the failure that used to end with
            // a deleted branch.
            //
            // `origin/main` is a fine thing to print, too: it is where the
            // session is measured from, said exactly.
            if !name.is_empty() {
                if let Some(resolved) = resolvable(repo, name) {
                    return resolved;
                }
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

/// The session `omh s attach` should land in when none is named: the most
/// recently created one.
pub fn current(worktrees_dir: &Path) -> Option<String> {
    list(worktrees_dir).pop()
}

/// Resolve which session to use. Creating a fresh one on every launch would
/// defeat persistence entirely — you would never reattach to the agent you left
/// running — so a new session is something you ask for.
pub fn pick(worktrees_dir: &Path, start: Start) -> String {
    match start {
        Start::Named(id) => id.to_string(),
        Start::Fresh => next_id(worktrees_dir),
    }
}

/// Where a session records which harness it ran.
///
/// A leading dot, and not for tidiness: `Paths::staging` is
/// `runs()/<session>/<harness>`, so this directory's other children are named
/// after harnesses. `runs()/<id>/harness` and the staging directory of an
/// adapter called `harness` would be the same path — a file where a directory
/// is about to go. `last-used` has the same latent shape and has never been
/// hit; there is no reason to add a second, likelier word to the collision.
fn harness_marker(run_dir: &Path, session: &str) -> PathBuf {
    run_dir.join(session).join(".harness")
}

/// Record which harness a session ran, so it can be rejoined as itself.
///
/// The staging directory already carries the name — `runs()/<id>/<harness>`
/// survives `down`, which removes the container and nothing else — so this is
/// not the only trace. It is the only *unambiguous* one: a session that has run
/// two harnesses has two staging directories and no way to say which was last,
/// and staging is a side effect that nothing promises to keep. Neither is true
/// of a marker written on purpose.
///
/// Nothing else knows at all. `Session` is an id, a branch and a worktree; the
/// container is `omh-<repo>-<id>` with no harness in the name; and the image
/// tag needs a container to inspect, which `omh sNN down` removes.
///
/// Written beside `last-used`, so `omh sNN rm` — which removes the whole run
/// directory — takes it away with everything else.
pub fn remember_harness(run_dir: &Path, session: &str, harness: &str) -> std::io::Result<()> {
    let path = harness_marker(run_dir, session);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, harness)
}

/// What omh recorded about the harness a session ran.
///
/// Three answers, because *there is no record* and *omh could not read the
/// record* are different news and the second is not the caller's fault. An
/// `Option` here would render a failed read as a confident claim about
/// history, which is the shape `image::Stamp` was introduced to stop one
/// module over — its `Read` means "genuinely older than the check" and its
/// `Unknown(String)` carries why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// omh recorded this name.
    Harness(String),
    /// No run directory, or no marker in it. The honest reading of a session
    /// older than this feature — and of one started by `omh s attach`, which
    /// makes a session for an editor and never runs a harness in it.
    NeverRecorded,
    /// The marker is there and omh could not read it, and why. A truncated
    /// write, a directory where the file should be, bad permissions, invalid
    /// UTF-8.
    CouldNotTell(String),
}

/// The harness a session ran, as far as omh can tell.
pub fn harness_of(run_dir: &Path, session: &str) -> Ran {
    let path = harness_marker(run_dir, session);
    let said = match std::fs::read_to_string(&path) {
        Ok(said) => said,
        // The one error that means *nothing was recorded* rather than *omh
        // could not find out*. Every other kind is news.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ran::NeverRecorded,
        Err(e) => return Ran::CouldNotTell(format!("{}: {e}", path.display())),
    };
    let name = said.trim();
    if name.is_empty() {
        // Not `NeverRecorded`: something wrote this and left nothing in it,
        // which is damage rather than absence.
        return Ran::CouldNotTell(format!("{} is empty", path.display()));
    }
    // omh wrote this, but a file on disk is not omh's word. The value becomes
    // `argv[0]` of a launch and is joined into a path, and whatever it holds
    // reaches the terminal in an error message — a marker carrying an ANSI
    // escape printed one, before this check.
    if name.contains(['/', '\\', '\n']) || name.chars().any(char::is_control) {
        return Ran::CouldNotTell(format!("{} does not name a harness", path.display()));
    }
    Ran::Harness(name.to_string())
}

/// Which session a command is asking for.
///
/// **`Resume` was the third answer and is gone.** It meant *the one already
/// here, or a fresh one if there is none*, and that second half is what let
/// `attach` open an editor on a session nobody started. Rejoining is
/// `existing_session` now, which refuses by name instead of inventing — so
/// there is no caller left that wants "or make one up", and a variant kept
/// alive by its own unit tests is a shape the next caller would reach for.
///
/// Three answers, and they were two parameters — `explicit: Option<&str>` and
/// `new: bool` — which is four combinations for three meanings. The spare one,
/// *this exact session, and also a brand new one*, had to be resolved, and it
/// was: silently, by returning the named id and never looking at the flag. A
/// test pinned that resolution, for an input the CLI refuses in two places and
/// no caller could produce.
///
/// As one value there is no pair to reconcile, so the precedence rule is not
/// simplified but deleted, and the caller says which of the three it means
/// rather than encoding it in the third argument's position.
///
/// Not `Landing`, which is how a session's *work* reaches a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start<'a> {
    /// This one, by id.
    Named(&'a str),
    /// One that does not exist yet.
    Fresh,
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

/// `git` for the callers that build their arguments dynamically.
/// Every whitespace check `diff --check` runs alongside the marker check,
/// turned off.
///
/// Not tidiness — correctness. `--check` reports whitespace errors too, and a
/// whitespace error is reported by **echoing the offending line of the file**.
/// Measured: a file containing `\t…: leftover conflict marker` with a space
/// before that tab comes back as `+ \tsecrets.rs:1: leftover conflict marker`,
/// which is to say the agent can write a line that reads exactly like git
/// finding a conflict and omh would refuse a commit that has nothing wrong with
/// it. Filtering the echo back out means guessing which lines are git's, and a
/// path may begin with `+`.
///
/// With the checks off there is no echo to tell apart: every line `--check`
/// prints is git's own, and exit 2 means markers and nothing else. A repo that
/// turns them all on cannot undo this — measured, the last `-c` wins.
const NO_WHITESPACE_CHECKS: &str = "core.whitespace=-trailing-space,-space-before-tab,\
                                    -indent-with-non-tab,-tab-in-indent,-blank-at-eof,\
                                    -cr-at-eol";

impl Session {
    /// The lines git reports for conflict markers still in the session's files.
    ///
    /// Against `HEAD`, not the index: measured, a bare `diff --check` compares
    /// the worktree to the index and says nothing at all about a marker the
    /// agent has already `git add`ed — which is the state a resolve-then-stage
    /// half-finished leaves behind, and precisely when the guard is needed.
    ///
    /// Whole lines rather than parsed paths. Each is `path:line: leftover
    /// conflict marker`, so a path holding a colon has no unambiguous reading —
    /// and the line is the better thing to show anyway, since it names where.
    ///
    /// Through the review index, not the worktree, and that is the whole
    /// mechanism rather than a convenience. Measured: `--check` diffs what git
    /// **tracks**, so a file the agent created and never added is invisible to
    /// it — and `commit -m` is a `git add -A`, which is exactly how such a file
    /// reaches the branch. A guard blind to new files would have refused the
    /// conflicts git already knew about and waved through the ones it did not.
    ///
    /// Staging into the throwaway index answers for the tree a commit would
    /// actually make, new files included, without touching the index the user's
    /// own git shares. It also inherits the unstage list, so omh's mounted
    /// scaffolding is out of the answer for the same reason it is out of a
    /// review.
    pub fn unresolved(&self, base: &str) -> Result<Vec<String>> {
        let out = self.reviewing(base, |worktree, index, _| {
            Command::new("git")
                .current_dir(worktree)
                .env("GIT_INDEX_FILE", index)
                .args(["-c", NO_WHITESPACE_CHECKS])
                // The worktree holds agent-authored files and an
                // agent-authored `.gitattributes`; a diff is a thing git can be
                // told to run a program for.
                .args([
                    "diff",
                    "--no-textconv",
                    "--no-ext-diff",
                    "--check",
                    "--cached",
                    "HEAD",
                ])
                .output()
                .context("running git")
        })?;
        match out.status.code() {
            // 2 is `--check`'s answer, not its failure.
            Some(0) | Some(2) => Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(crate::out::untrusted)
                .collect()),
            _ => anyhow::bail!(
                "git diff --check: {}",
                crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
            ),
        }
    }
}

fn git_owned(cwd: &Path, args: &[String]) -> Result<String> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    git(cwd, &borrowed)
}

/// `git` against an index of our own, so a read-only command can stage the
/// worktree without disturbing the one the user's git shares.
/// Whether git ignores `path` in this worktree: `info/exclude`, where
/// `carry::apply` lists what it carried, and the project's own ignore files.
///
/// Asked of git rather than read from the files, because the patterns are
/// git's to evaluate. Tracked files are never reported — they are not
/// subject to exclude rules — so a file the project ships is never mistaken
/// for a carried one, whatever `.gitignore` says about its name.
fn ignored_here(worktree: &Path, path: &str) -> Result<bool> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["check-ignore", "-q", "--", path])
        .output()
        .context("running git check-ignore")?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "git check-ignore -- {}: {}",
            crate::out::untrusted(path),
            crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
        ),
    }
}

fn git_with_index(cwd: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether something is still on disk — with "could not tell" kept apart from
/// "not there".
///
/// Not `Path::exists`, which is `metadata().is_ok()` and so folds every stat
/// failure into `false`. An unreadable parent, a directory whose search bit is
/// gone, a mount that went away mid-command: all of them answered "absent",
/// and a caller checking whether it had finished deleting something read that
/// as done. Measured on macOS: with the parent at `0o600` the child still
/// exists and `exists()` returns `false`.
pub(crate) fn still_there(at: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(at) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // Not followed, so a dangling symlink is its own presence rather than
        // the absence of what it points at.
        Err(e) => Err(format!("{e}")),
    }
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
            crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Somewhere for `remove` to reap a shadow from. The tests that use it are
    /// not about shadows; they just may not silently skip the reaping.
    fn d_shadows() -> PathBuf {
        std::env::temp_dir().join("omh-test-shadows")
    }

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

    /// A conflict left half-resolved is found — staged, loose, or in a file
    /// git has never seen — and prose about conflicts is not mistaken for one.
    ///
    /// Four claims, because the guard is only worth having if all four hold,
    /// and two of them were wrong in the first draft. A marker the agent
    /// staged is invisible to a worktree diff; a marker in a file it never
    /// added is invisible to any diff of tracked files, and that is precisely
    /// the file `commit -m` would sweep up. The forged line is the fourth:
    /// with git's whitespace checks left on, `--check` echoes the offending
    /// line of the file, so the agent can write something that reads exactly
    /// like git finding a conflict and omh refuses a commit over a comment.
    #[test]
    fn markers_left_in_the_tree_are_found_and_agent_prose_is_not_mistaken_for_them() {
        let (dir, root) = repo();
        let s = Session::new(&dir.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        let write = |name: &str, body: &str| {
            std::fs::write(s.worktree.join(name), body).unwrap();
        };

        write("clean.rs", "fn ok() {}\n");
        write("staged.rs", "fn a() {}\n");
        write("loose.rs", "fn c() {}\n");
        write("prose.rs", "fn note() {}\n");
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-qm", "the agent's work"]).unwrap();
        assert!(
            s.unresolved("main").unwrap().is_empty(),
            "a tree with no markers is clean"
        );

        // Uncommitted, so it is in the diff `--check` reads: a committed file
        // nothing has touched is not, and a forgery git never looks at proves
        // nothing about whether git would have been fooled.
        //
        // Reads exactly like git's own report, and carries a whitespace error
        // (a space before that tab) so `--check` echoes the line back if the
        // whitespace checks are left on.
        write("prose.rs", " \tsecrets.rs:1: leftover conflict marker\n");
        write("staged.rs", "<<<<<<< main\na\n=======\nb\n>>>>>>> s01\n");
        write("loose.rs", "<<<<<<< main\nc\n=======\nd\n>>>>>>> s01\n");
        git(&s.worktree, &["add", "staged.rs"]).unwrap();

        let found = s.unresolved("main").unwrap();
        assert!(
            found.iter().any(|l| l.starts_with("staged.rs:")),
            "a marker the agent staged is still a marker: {found:?}"
        );
        assert!(
            found.iter().any(|l| l.starts_with("loose.rs:")),
            "and so is one it left in the worktree: {found:?}"
        );
        assert!(
            !found.iter().any(|l| l.contains("secrets.rs")),
            "but a line the agent wrote is not git speaking: {found:?}"
        );

        // A file git has never seen. `--check` read from the worktree cannot
        // see it either — and `commit -m` is a `git add -A`, so this is the
        // one that would have landed.
        write(
            "never-added.rs",
            "<<<<<<< main\ne\n=======\nf\n>>>>>>> s01\n",
        );
        assert!(
            s.unresolved("main")
                .unwrap()
                .iter()
                .any(|l| l.starts_with("never-added.rs:")),
            "a marker in a file the agent never added is still one about to land"
        );
    }

    /// Session ids are reused: `next_id` is the highest `sNN` among the
    /// worktrees plus one, so removing `s01` hands the next session that name
    /// back. A shadow left behind is then adopted by a session that has nothing
    /// to do with it — the new agent opens on someone else's scratch history,
    /// and the seed recorded for it names a tree this worktree never had, so
    /// everything measured from that seed is measured from a fiction.
    ///
    /// Deterministic rather than a race, and invisible: the shadow looks
    /// perfectly well-formed, it just belongs to a session that is gone.
    #[test]
    fn removing_a_session_takes_its_shadow_with_it() {
        let (dir, root) = repo();
        let shadows = dir.path().join("shadow");
        let s = Session::new(&dir.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        let shadow = crate::shadow::Shadow::new(&shadows, &s.id);
        shadow.ensure(&s.worktree, &[]).unwrap();
        let first_seed = std::fs::read_to_string(&shadow.seed_record).unwrap();

        s.remove(&root, "main", &shadows).unwrap();

        // the id comes back around
        assert_eq!(next_id(&dir.path().join("wt")), "s01");
        let reborn = Session::new(&dir.path().join("wt"), "s01".into());
        reborn.ensure(&root, "main").unwrap();
        std::fs::write(reborn.worktree.join("different.rs"), "fn new() {}").unwrap();
        let shadow = crate::shadow::Shadow::new(&shadows, &reborn.id);
        shadow.ensure(&reborn.worktree, &[]).unwrap();

        assert_ne!(
            std::fs::read_to_string(&shadow.seed_record).unwrap(),
            first_seed,
            "a new session must not inherit the shadow of the one that had its id"
        );
    }

    /// **Whether it is gone is asked of the disk, not of git's exit code.**
    ///
    /// The fallback used to run only when `git worktree remove` reported
    /// failure. A git that exits 0 while leaving the directory behind — files
    /// it cannot delete, an owner it is not — was taken at its word, and `rm`
    /// printed `removed session sNN` over a worktree still on disk, exiting 0.
    /// Nothing verified the thing this function is named for.
    ///
    /// Driven by making the directory undeletable, which is the shape of the
    /// real cause: something else is holding it.
    #[test]
    #[cfg(unix)]
    fn a_worktree_that_could_not_be_removed_is_reported_as_still_there() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        ] {
            std::process::Command::new("git")
                .args(["-C", &root.display().to_string()])
                .args(&args)
                .output()
                .expect("git must be installed to run this test");
        }

        let worktrees = d.path().join("worktrees");
        let s = Session::new(&worktrees, "s01".into());
        s.ensure(&root, "main").unwrap();
        assert!(s.worktree.exists(), "the fixture made a worktree");

        // A directory whose parent denies writes cannot have its entries
        // unlinked, so both git and `remove_dir_all` leave it behind.
        let parent = s.worktree.parent().unwrap().to_path_buf();
        let was = std::fs::metadata(&parent).unwrap().permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let asked = s.remove(&root, "main", &d.path().join("shadows"));
        std::fs::set_permissions(&parent, was).unwrap();

        let gone = asked.expect("`remove` itself still answers").worktree;
        match gone {
            // The reason is what a caller prints to somebody deciding what to
            // clean up by hand, so an empty one is a row nobody can act on.
            Gone::No(why) => assert!(!why.trim().is_empty(), "and it says why: {why}"),
            Gone::Yes => panic!(
                "the worktree is still at {} — reporting it gone is the bug",
                s.worktree.display()
            ),
        }
        assert!(s.worktree.exists(), "the fixture really did block removal");
    }

    /// The same lie, one syscall earlier: a parent that cannot be looked into.
    ///
    /// `Path::exists` is `metadata().is_ok()`, so it answers `false` for every
    /// stat failure and not only for "it is not there". With the parent's
    /// search bit gone, the old check skipped the removal entirely and the
    /// default `Gone::Yes` stood — `rm` reporting a removed session over a
    /// worktree still on disk, which is the bug this type exists to make
    /// unsayable.
    ///
    /// Measured, and the reason the test above cannot catch this: at `0o500`
    /// the parent is still traversable, so `exists` answers truthfully and
    /// `remove_dir_all` is what fails. Only `0o600` reaches the collapse.
    #[test]
    #[cfg(unix)]
    fn a_worktree_that_cannot_be_looked_at_is_not_reported_as_gone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        ] {
            std::process::Command::new("git")
                .args(["-C", &root.display().to_string()])
                .args(&args)
                .output()
                .expect("git must be installed to run this test");
        }

        let worktrees = d.path().join("worktrees");
        let s = Session::new(&worktrees, "s01".into());
        s.ensure(&root, "main").unwrap();
        assert!(s.worktree.exists(), "the fixture made a worktree");

        // No search bit: the directory can neither be traversed nor stat'd,
        // so every question about it fails rather than answering "absent".
        let parent = s.worktree.parent().unwrap().to_path_buf();
        let was = std::fs::metadata(&parent).unwrap().permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o600)).unwrap();
        let asked = s.remove(&root, "main", &d.path().join("shadows"));
        std::fs::set_permissions(&parent, was).unwrap();

        let gone = asked.expect("`remove` itself still answers").worktree;
        match gone {
            Gone::No(why) => assert!(!why.trim().is_empty(), "and it says why: {why}"),
            Gone::Yes => panic!(
                "the worktree is still at {} — omh could not tell, and \
                 reporting that as gone is the bug",
                s.worktree.display()
            ),
        }
        assert!(s.worktree.exists(), "the fixture really did block removal");
    }

    /// The distinction the row above rests on: absent, present, and unanswerable
    /// are three answers, and `Path::exists` collapses the third into the first.
    #[test]
    #[cfg(unix)]
    fn a_directory_that_cannot_be_stated_is_neither_there_nor_gone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let parent = d.path().join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();

        assert_eq!(still_there(&child), Ok(true), "it is there");
        assert_eq!(
            still_there(&parent.join("never")),
            Ok(false),
            "and that one is not"
        );

        let was = std::fs::metadata(&parent).unwrap().permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o600)).unwrap();
        let blind = still_there(&child);
        std::fs::set_permissions(&parent, was).unwrap();

        assert!(
            blind.is_err(),
            "an unreadable parent is not an answer of `false`: {blind:?}"
        );
    }

    /// The three answers, and the honest edge of the third.
    #[test]
    fn a_branch_question_git_cannot_answer_is_not_answered_as_no() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
            vec!["branch", "omh/s01"],
        ] {
            std::process::Command::new("git")
                .args(["-C", &root.display().to_string()])
                .args(&args)
                .output()
                .expect("git must be installed to run this test");
        }

        let there = Session::new(&d.path().join("worktrees"), "s01".into());
        assert_eq!(there.branch_exists(&root), Ok(true), "it is there");
        let not = Session::new(&d.path().join("worktrees"), "s09".into());
        assert_eq!(not.branch_exists(&root), Ok(false), "and that one is not");

        // Not a repository at all: the answer is neither yes nor no, and
        // answering "no" is what let a branch holding work disappear from the
        // report. `remove` turns this into `BranchKept`, which is the arm that
        // loses nothing.
        let elsewhere = d.path().join("not-a-repo");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let blind = there.branch_exists(&elsewhere);
        assert!(
            blind.is_err(),
            "a question git could not answer is not an answer of `false`: {blind:?}"
        );
    }

    /// `BranchDropped` is a claim about a deletion, so the deletion has to be
    /// the thing that produces it.
    ///
    /// It was computed before the worktree went and never revised: `git branch
    /// -D` ran as `let _`, and the variant decided minutes earlier was
    /// returned whether or not the ref survived. `rm` then printed "the branch
    /// omh/s01, which held no commits" over a branch still on disk, and the
    /// `omh/` namespace filled with refs omh had said were gone.
    ///
    /// Driven by an unwritable `refs/heads/omh`, which is what a read-only
    /// checkout, a stale lock or a concurrent `gc` looks like from here.
    #[test]
    #[cfg(unix)]
    fn a_branch_that_would_not_delete_is_not_reported_as_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        ] {
            std::process::Command::new("git")
                .args(["-C", &root.display().to_string()])
                .args(&args)
                .output()
                .expect("git must be installed to run this test");
        }

        let worktrees = d.path().join("worktrees");
        let s = Session::new(&worktrees, "s01".into());
        s.ensure(&root, "main").unwrap();
        // Commitless, so the decision is `BranchDropped` and the delete is
        // actually attempted.
        let refs = root.join(".git/refs/heads/omh");
        let was = std::fs::metadata(&refs).unwrap().permissions();
        std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(0o500)).unwrap();
        let done = s.remove(&root, "main", &d.path().join("shadows")).unwrap();
        std::fs::set_permissions(&refs, was).unwrap();

        let alive = std::process::Command::new("git")
            .args(["-C", &root.display().to_string()])
            .args(["rev-parse", "--verify", "--quiet", "omh/s01"])
            .output()
            .unwrap();
        assert!(
            alive.status.success(),
            "the fixture has to actually block the delete"
        );
        assert_ne!(
            done.branch,
            Removed::BranchDropped,
            "the branch is still there, so nothing may report it dropped"
        );
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

        let done = s.remove(&root, "main", &d_shadows()).unwrap();
        let (outcome, gone) = (done.branch, done.worktree);
        assert_eq!(gone, Gone::Yes, "the worktree is gone");
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

        let done = s.remove(&root, "main", &d_shadows()).unwrap();
        let (outcome, gone) = (done.branch, done.worktree);
        assert_eq!(gone, Gone::Yes, "the worktree is gone");
        assert_eq!(outcome, Removed::BranchKept(Some(1)));
        assert!(
            git(&root, &["rev-parse", "--verify", "omh/s02"]).is_ok(),
            "unreviewed work must be unloseable"
        );
    }

    /// The same half, when git cannot answer the question at all.
    ///
    /// `commits` asks `rev-list <base>..<branch>`, and a base that does not
    /// resolve locally makes that fail rather than return a number — a repo
    /// whose trunk was renamed, or a clone whose `main` exists only as
    /// `origin/main`. Read as zero, the failure means `rm` reports *no commits*
    /// and deletes a branch holding work nobody has reviewed.
    ///
    /// The base is passed explicitly rather than through `default_branch`.
    /// What that function returns is a moving target — it is where the
    /// unresolvable name comes from in the first place — and a test that
    /// sourced its precondition from the code under repair would stop
    /// reproducing anything the day that changed, while still passing.
    #[test]
    fn a_branch_is_kept_when_omh_cannot_count_what_is_on_it() {
        let (dir, root) = repo();
        let s = Session::new(&dir.path().join("wt"), "s03".into());
        s.ensure(&root, "main").unwrap();

        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "."]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();

        // The repo renames its trunk, which is the whole reason `main` stops
        // resolving. Asserted rather than assumed: without this the test can
        // quietly become a second copy of the one above.
        git(&root, &["branch", "-m", "main", "trunk"]).unwrap();
        assert!(
            git(&root, &["rev-list", "--count", "main..omh/s03"]).is_err(),
            "the precondition is that git cannot answer; it just did"
        );

        let done = s.remove(&root, "main", &d_shadows()).unwrap();
        let (outcome, gone) = (done.branch, done.worktree);
        assert_eq!(gone, Gone::Yes, "the worktree is gone");
        assert!(
            git(&root, &["rev-parse", "--verify", "omh/s03"]).is_ok(),
            "a count omh could not take is not a branch it may delete"
        );
        assert_eq!(
            outcome,
            Removed::BranchKept(None),
            "and it has to say *why* it was kept: a count omh never took is not a \
             count of zero, and `rm` renders the two differently"
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
        assert_eq!(
            s.remove(&root, "main", &d_shadows()).unwrap().branch,
            Removed::NoBranch
        );
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

        s.remove(&root, "main", &d_shadows()).unwrap();
        assert!(!s.worktree.exists(), "worktree gone");
        assert_eq!(s.branch_exists(&root), Ok(true), "branch must survive rm");

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
        assert!(s.diff("main", What::Summary).unwrap().contains("new.rs"));
    }

    /// The agent never commits — it cannot, and after the shadow repo lands its
    /// commits still are not the branch's. So the work `diff` has to report is
    /// the work sitting in the worktree, and a commit-to-commit diff reports
    /// none of it: `omh s diff` answered with silence for the whole span of a
    /// session, while the git rules section and `getting-started` both told the agent the
    /// user reviews with it *before* committing.
    #[test]
    fn diff_reports_work_the_user_has_not_committed_yet() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("agent.rs"), "fn main() {}").unwrap();

        let out = s.diff("main", What::Summary).unwrap();
        assert!(
            out.contains("agent.rs"),
            "uncommitted agent work must be reviewable: {out:?}"
        );
    }

    /// omh stages its rules by mounting them over a placeholder it writes into
    /// the worktree, and `info/exclude` cannot hide that placeholder when the
    /// repo *tracks* the filename — gitignore semantics say nothing about a
    /// file git already has. This repo tracks `AGENTS.md`, so the case is not
    /// exotic: reading the worktree turned omh's own scaffolding into a line
    /// saying the agent had emptied the project's rules.
    ///
    /// `commit` already unstages these for the same reason. Both now read the
    /// one list, so what a review shows and what a commit carries cannot
    /// disagree about omh's own files.
    #[test]
    fn diff_never_shows_omhs_own_staging_as_the_agents_work() {
        let (d, root) = repo();
        let rules = crate::carry::STAGED_RULES[1];
        std::fs::write(root.join(rules), "# the project's real rules\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "rules the repo tracks"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        // the placeholder a bind mount needs its destination to be
        std::fs::write(s.worktree.join(rules), "").unwrap();
        std::fs::write(s.worktree.join("agent.rs"), "fn main() {}").unwrap();

        let out = s.diff("main", What::Summary).unwrap();
        assert!(
            !out.contains(rules),
            "omh's own staging is not the agent's work: {out:?}"
        );
        assert!(
            out.contains("agent.rs"),
            "the agent's actual work still has to show: {out:?}"
        );
    }

    /// The base moving under a running session must not manufacture changes the
    /// session never made. Three-dot diff pinned this to the fork point; the
    /// working-tree form has to keep that property rather than inherit `HEAD`'s.
    #[test]
    fn diff_ignores_commits_the_base_gained_after_the_session_forked() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("agent.rs"), "fn main() {}").unwrap();

        // trunk moves on while the agent works
        std::fs::write(root.join("trunk.rs"), "fn trunk() {}").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "trunk work"]).unwrap();

        let out = s.diff("main", What::Summary).unwrap();
        assert!(out.contains("agent.rs"), "the session's own work: {out:?}");
        assert!(
            !out.contains("trunk.rs"),
            "a file the session never touched must not appear as its change: {out:?}"
        );
    }

    /// A worktree that wandered off its branch no longer holds the session's
    /// committed work on disk, and a diff read from the worktree reports what
    /// is there — so the commits vanish from the review and the command still
    /// succeeds. Silence would be bad enough; a confident partial answer is
    /// worse, because the user commits against it.
    #[test]
    fn diff_refuses_from_a_worktree_that_left_its_branch() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("agent.rs"), "fn main() {}").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "the session's work"]).unwrap();

        // the worktree is taken off its branch to look at something
        git(&s.worktree, &["checkout", "-q", "HEAD~1"]).unwrap();
        assert!(
            !s.worktree.join("agent.rs").exists(),
            "checkout takes the committed work off disk — the premise of this test"
        );

        let err = s.diff("main", What::Summary).unwrap_err().to_string();
        assert!(
            err.contains("omh/s01"),
            "the branch it should be on has to be named: {err}"
        );
    }

    /// `read-tree HEAD` is what makes the throwaway index start from the commit
    /// rather than from nothing, and only this case proves it: `add -A` skips a
    /// path `info/exclude` names, so against an empty index the path is simply
    /// absent and the diff calls it a **deletion** — a file reported as removed
    /// that is sitting right there on disk.
    ///
    /// Not hypothetical. `carry` writes that exclude file, and a `carry_in`
    /// entry naming an already-tracked path is a live misconfiguration `carry`
    /// warns about at launch rather than refuses.
    ///
    /// The mutation this is red against is `read-tree --empty`, not dropping
    /// the call: `NamedTempFile` leaves a zero-byte file that git rejects
    /// outright, so removing the line fails every diff test on `index file
    /// smaller than expected` and proves nothing about *this*.
    #[test]
    fn diff_calls_an_excluded_tracked_file_changed_rather_than_deleted() {
        let (d, root) = repo();
        std::fs::write(root.join("local.env"), "KEY=1\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(
            &root,
            &["commit", "-q", "-m", "a tracked file carry_in also names"],
        )
        .unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        let exclude = crate::carry::exclude_path(&s.worktree).unwrap();
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        std::fs::write(&exclude, "local.env\n").unwrap();
        std::fs::write(s.worktree.join("local.env"), "KEY=1\nKEY2=2\n").unwrap();

        let out = s.diff("main", What::Summary).unwrap();
        assert!(
            !out.contains("deletion"),
            "a file present on disk must never be reported as deleted: {out:?}"
        );
        assert!(
            out.contains("local.env"),
            "the change itself still has to show: {out:?}"
        );
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

        s.commit(Some("Add the work"), Carried::refusing(&[]))
            .unwrap();

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

        s.commit(Some("Add a new file"), Carried::refusing(&[]))
            .unwrap();

        assert!(committed_files(&s.worktree).contains("brand-new.rs"));
    }

    /// A no-op reporting success teaches people to trust a commit that never
    /// happened, and the next command they run is `push`.
    #[test]
    fn committing_a_clean_worktree_is_an_error() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        let err = s
            .commit(Some("nothing to say"), Carried::refusing(&[]))
            .unwrap_err();
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

        s.commit(Some("Fix the guard"), Carried::refusing(&[]))
            .unwrap();

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
        s.commit(Some("Add the work"), Carried::refusing(&[]))
            .unwrap();
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

        assert_eq!(s.published_as().unwrap().as_deref(), Some("fix/tap-guard"));
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
        s.commit(Some("Add more"), Carried::refusing(&[])).unwrap();

        assert_eq!(s.push(None).unwrap(), "fix/tap-guard");
    }

    /// Every local step can succeed while the remote a reviewer would open the
    /// PR from stays untouched — which is `e0a41b8` exactly: a release job that
    /// copied, staged, pushed and passed, against a local clone, while the tap
    /// it was publishing to stayed empty.
    ///
    /// Reproduced with a `pushurl`, because that is the configuration where the
    /// push genuinely succeeds and origin genuinely does not have it. Deleting
    /// the remote instead only proves that `git push` fails when there is
    /// nothing to push to, which needs no guard of ours — that version stays
    /// green with the read-back removed.
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

    // ── what `omh s` reports about work in flight ────────────────────────────

    /// A summary says which files; a patch says what changed in them.
    ///
    /// Both go through `reviewing`, so they are taken from one index against
    /// one merge base — the alternative is two answers about one session that
    /// can disagree, and the summary is what a user reads *before* deciding
    /// the patch is worth opening. The paged patch goes through it too, which
    /// is what `a_worktree_that_left_its_branch_is_refused_whichever_way_you_
    /// ask` checks: this test cannot see that route, because it returns
    /// nothing to assert on.
    #[test]
    fn a_patch_carries_the_lines_a_summary_only_counts() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn added() {}\n").unwrap();

        let summary = s.diff("main", What::Summary).unwrap();
        let patch = s.diff("main", What::Patch).unwrap();

        assert!(
            summary.contains("work.rs") && !summary.contains("fn added"),
            "a summary counts lines rather than quoting them: {summary}"
        );
        assert!(
            patch.contains("+fn added() {}"),
            "the patch is the lines themselves: {patch}"
        );
        assert!(
            patch.contains("work.rs"),
            "and still says which file: {patch}"
        );
    }

    /// A three-way merge on the host, which is where every merge in this
    /// design happens.
    ///
    /// The claim the whole mechanism rests on: `merge-tree --write-tree`
    /// produces a tree — with conflict markers already in it — and touches
    /// nothing. If it moved a ref or wrote the worktree, running it against a
    /// live checkout would be the thing this design refuses to do.
    #[test]
    fn a_merge_on_the_host_produces_a_tree_and_touches_nothing() {
        let (d, root) = repo();
        let git_in = |args: &[&str]| git(&root, args).unwrap();
        let write = |name: &str, body: &str| {
            std::fs::write(root.join(name), body).unwrap();
        };

        write("shared.rs", "one\ntwo\nthree\n");
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "base"]);
        let base = git_in(&["rev-parse", "HEAD"]).trim().to_string();

        // Trunk moves.
        write("shared.rs", "ONE\ntwo\nthree\n");
        write("from-trunk.rs", "fn trunk() {}\n");
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "trunk moved"]);
        let trunk = git_in(&["rev-parse", "HEAD"]).trim().to_string();

        // The session changed a different line, from the same base.
        git_in(&["checkout", "-q", "-b", "session", &base]);
        write("shared.rs", "one\ntwo\nTHREE\n");
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "agent work"]);
        let clean = git_in(&["rev-parse", "HEAD"]).trim().to_string();

        // Something to notice if the merge writes to the checkout.
        write("untracked.rs", "not committed\n");
        let before = (
            git_in(&["status", "--porcelain"]),
            git_in(&["rev-parse", "HEAD"]),
        );

        let merged = merge_three(&root, &base, &trunk, &clean).unwrap();
        assert!(
            merged.conflicted.is_empty(),
            "different lines merge cleanly: {merged:?}"
        );
        let shared = git_in(&["cat-file", "-p", &format!("{}:shared.rs", merged.tree)]);
        assert_eq!(
            shared, "ONE\ntwo\nTHREE\n",
            "both sides are in the merged tree"
        );
        assert!(
            !git_in(&["cat-file", "-p", &format!("{}:from-trunk.rs", merged.tree)]).is_empty(),
            "including what trunk added"
        );

        assert_eq!(
            before,
            (
                git_in(&["status", "--porcelain"]),
                git_in(&["rev-parse", "HEAD"])
            ),
            "the merge wrote a tree and nothing else — not the worktree, not HEAD"
        );
        let _ = d;
    }

    /// A conflict is an answer, and it arrives as text.
    ///
    /// The tree is written either way, with the markers already in it, which
    /// is what lets a conflict cross into the sandbox for the agent to resolve.
    /// The labels are the arguments: a branch name on one side reads as the
    /// branch, and naming the other side is worth a scratch ref.
    #[test]
    fn a_conflict_comes_back_as_a_tree_with_markers_and_the_paths_that_need_a_decision() {
        let (d, root) = repo();
        let git_in = |args: &[&str]| git(&root, args).unwrap();
        std::fs::write(root.join("shared.rs"), "one\ntwo\n").unwrap();
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "base"]);
        let base = git_in(&["rev-parse", "HEAD"]).trim().to_string();

        std::fs::write(root.join("shared.rs"), "TRUNK\ntwo\n").unwrap();
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "trunk"]);

        git_in(&["checkout", "-q", "-b", "session", &base]);
        std::fs::write(root.join("shared.rs"), "AGENT\ntwo\n").unwrap();
        git_in(&["add", "-A"]);
        git_in(&["commit", "-qm", "agent"]);
        // A ref so the marker names the session rather than an object id.
        git_in(&["update-ref", "refs/s01", "session^{tree}"]);

        let merged = merge_three(&root, &base, "main", "s01").unwrap();
        assert_eq!(
            merged.conflicted,
            vec!["shared.rs".to_string()],
            "named once, though git reports three stages for it"
        );

        let body = git_in(&["cat-file", "-p", &format!("{}:shared.rs", merged.tree)]);
        assert!(
            body.contains("<<<<<<< main") && body.contains(">>>>>>> s01"),
            "both sides are labelled with what they are: {body}"
        );
        assert!(
            body.contains("TRUNK") && body.contains("AGENT"),
            "and both versions are there to choose between: {body}"
        );
        let _ = d;
    }

    /// Every path `changed` reports is a name the worktree actually has.
    ///
    /// The contract, not the parser: a name omh cannot open is a name it must
    /// not print, and `omh s` prints these at a user as the files two sessions
    /// are both changing.
    ///
    /// The first version of this test used `a space.rs` — which git quotes,
    /// but whose quoting is pure delimiter, so it passed while every escape
    /// git actually writes went untested. These four names are the ones that
    /// were wrong: an accent (octal-escaped), a literal ` -> ` (split as if it
    /// were a rename), an embedded quote (over-trimmed), and a plain name to
    /// prove the ordinary case still works.
    #[test]
    #[cfg(unix)]
    fn changed_reports_the_names_the_worktree_actually_has() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        let names = ["plain.rs", "été.rs", "a -> b.rs", "say\"hi.rs"];
        for name in names {
            std::fs::write(s.worktree.join(name), "fn f() {}\n").unwrap();
        }

        let mut got = s.changed().unwrap();
        got.sort();
        let mut want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        want.sort();
        assert_eq!(
            got, want,
            "a name omh cannot open is a name it must not print"
        );
    }

    /// A rename reports the name that exists now, not the one it came from.
    ///
    /// The old name is not something another session can be changing — it is
    /// not there.
    #[test]
    fn a_renamed_file_is_reported_under_the_name_it_has_now() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("before.rs"), "fn work() {}\n").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-qm", "add it"]).unwrap();
        git(&s.worktree, &["mv", "before.rs", "after.rs"]).unwrap();

        assert_eq!(
            s.changed().unwrap(),
            vec!["after.rs".to_string()],
            "the name that is there, once"
        );
    }

    /// The state that strands work is the one `omh s` cannot otherwise see: a
    /// session holding a day of uncommitted changes reads exactly like an
    /// untouched one. It must not count what omh itself put there, for the same
    /// reason `commit` must not commit it.
    #[test]
    fn uncommitted_counts_the_agents_work_and_not_omhs_own() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        std::fs::write(s.worktree.join("CLAUDE.md"), "staged by omh").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        assert_eq!(s.changed().unwrap().len(), 1);
    }

    #[test]
    fn a_clean_session_reports_nothing_uncommitted() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        assert_eq!(s.changed().unwrap().len(), 0);
    }

    /// Before a push there is no upstream to measure against, which is a
    /// different answer from "nothing to push" — one means name it, the other
    /// means you are done.
    #[test]
    fn unpushed_distinguishes_never_pushed_from_up_to_date() {
        let (d, root) = repo_with_origin();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");
        assert_eq!(s.unpushed().unwrap(), None, "never pushed");

        s.push(Some("fix/tap-guard")).unwrap();
        assert_eq!(s.unpushed().unwrap(), Some(0), "everything is on origin");

        std::fs::write(s.worktree.join("more.rs"), "fn more() {}").unwrap();
        s.commit(Some("Add more"), Carried::refusing(&[])).unwrap();
        assert_eq!(s.unpushed().unwrap(), Some(1));
    }

    /// A repo that commits its own `CLAUDE.md` is the normal case for one whose
    /// users run agent harnesses, and it is the case `carry`'s exclusion cannot
    /// reach: `info/exclude` is gitignore semantics, silent about a file git
    /// already tracks. omh overwrites it at launch, so git sees a modification
    /// and `add -A` stages it — omh's generated rules landing on top of the
    /// project's own conventions, in the user's PR, on the commit omh made.
    #[test]
    fn omhs_rules_stay_out_of_the_commit_even_when_the_repo_tracks_them() {
        let (d, root) = repo();
        // The repo's own file, committed before any session exists.
        std::fs::write(root.join("CLAUDE.md"), "# House style\n\nTabs.\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "house style"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        // What omh does at launch: overwrite it with the merged rules.
        std::fs::write(s.worktree.join("CLAUDE.md"), "omh generated rules").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        s.commit(Some("Add the work"), Carried::refusing(&[]))
            .unwrap();

        let committed = git(&s.worktree, &["show", "--stat", "--name-only", "HEAD"]).unwrap();
        assert!(committed.contains("work.rs"), "got: {committed}");
        assert!(
            !committed.contains("CLAUDE.md"),
            "omh clobbered the project's own conventions file: {committed}"
        );
        // And the repo's version is what survives on the branch.
        let on_branch = git(&s.worktree, &["show", "HEAD:CLAUDE.md"]).unwrap();
        assert!(on_branch.contains("House style"), "got: {on_branch}");
    }

    /// The same overwrite, in a session where the agent did nothing. Left
    /// counted, omh's own clobbering is the entire diff — so `commit` reports
    /// success for work that does not exist, and `omh s` reads `1 uncommitted`
    /// for a session nobody has touched.
    #[test]
    fn a_session_holding_only_omhs_overwrite_has_nothing_to_commit() {
        let (d, root) = repo();
        std::fs::write(root.join("CLAUDE.md"), "# House style\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "house style"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::hide_staged_rules(&s.worktree).unwrap();
        std::fs::write(s.worktree.join("CLAUDE.md"), "omh generated rules").unwrap();

        assert_eq!(
            s.changed().ok().map(|c| c.len()),
            Some(0),
            "omh's own staging is not work"
        );
        let err = s
            .commit(Some("nothing the agent did"), Carried::refusing(&[]))
            .unwrap_err();
        assert!(err.to_string().contains("nothing to commit"), "got: {err}");
    }

    /// `git status --porcelain` collapses an untracked directory into one entry,
    /// so a session where the agent wrote a whole new module reads as a single
    /// stray file — and this is the number `omh s` is designed to be glanced at.
    #[test]
    fn a_new_directory_counts_once_per_file_not_once() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::create_dir_all(s.worktree.join("newmodule")).unwrap();
        for i in 0..12 {
            std::fs::write(s.worktree.join(format!("newmodule/f{i}.rs")), "fn f() {}").unwrap();
        }

        assert_eq!(s.changed().unwrap().len(), 12);
    }

    /// `carry_in` is documented as the only path by which a secret reaches the
    /// agent, and a carried file that the repo *tracks* arrives in the worktree
    /// as an ordinary modification — `info/exclude` says nothing about tracked
    /// files. Staged and committed, a local edit holding a credential is on the
    /// branch, and one `s push` from being published.
    #[test]
    fn a_carried_file_the_repo_tracks_is_refused_rather_than_committed() {
        let (d, root) = repo();
        std::fs::write(root.join("config.toml"), "PORT=3000\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "config"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        // The user's local edit, carried in as `carry::apply` copies it.
        std::fs::write(
            s.worktree.join("config.toml"),
            "PORT=3000\nSECRET=hunter2\n",
        )
        .unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        let carried = ["config.toml".to_string()];
        let err = s
            .commit(Some("Add the work"), Carried::refusing(&carried))
            .unwrap_err();

        assert!(err.to_string().contains("config.toml"), "got: {err}");
        assert_eq!(s.commits(&root, "main").unwrap(), 0, "nothing may land");
    }

    /// The escape hatch, because refusing forever would make a carried file that
    /// the repo tracks a session you can never commit from.
    #[test]
    fn skipping_carried_files_commits_the_rest_and_leaves_them_behind() {
        let (d, root) = repo();
        std::fs::write(root.join("config.toml"), "PORT=3000\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-q", "-m", "config"]).unwrap();

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(
            s.worktree.join("config.toml"),
            "PORT=3000\nSECRET=hunter2\n",
        )
        .unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        let carried = ["config.toml".to_string()];
        s.commit(Some("Add the work"), Carried::skipping(&carried))
            .unwrap();

        let committed = git(&s.worktree, &["show", "--stat", "--name-only", "HEAD"]).unwrap();
        assert!(committed.contains("work.rs"), "got: {committed}");
        assert!(
            !committed.contains("config.toml"),
            "the secret must not land: {committed}"
        );
    }

    /// A carried file the repo does not track is already invisible to git, so it
    /// must not turn every commit into a refusal.
    #[test]
    fn an_untracked_carried_file_is_not_something_to_refuse_over() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        crate::carry::apply(&root, &s.worktree, &[".env.local".to_string()]).ok();
        std::fs::write(s.worktree.join(".env.local"), "SECRET=hunter2\n").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        let carried = [".env.local".to_string()];
        s.commit(Some("Add the work"), Carried::refusing(&carried))
            .unwrap();

        let committed = git(&s.worktree, &["show", "--stat", "--name-only", "HEAD"]).unwrap();
        assert!(!committed.contains(".env.local"), "got: {committed}");
    }

    /// Without `-m`, git owns the message and can refuse it — an editor that
    /// writes nothing means an empty message, and git aborts. Accepting that as
    /// success reports `committed to omh/s01 (0 commits)` and exits zero, which
    /// is the same lie as an empty commit and reaches the user one command later,
    /// at `push`.
    #[test]
    fn a_commit_the_editor_abandoned_is_not_reported_as_made() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();
        // An "editor" that exits 0 having written nothing.
        git(&s.worktree, &["config", "core.editor", "true"]).unwrap();

        let err = s.commit(None, Carried::refusing(&[])).unwrap_err();

        assert!(err.to_string().contains("aborted"), "got: {err}");
        assert_eq!(
            s.commits(&root, "main").unwrap(),
            0,
            "nothing may land on the branch"
        );
    }

    /// `branch.autoSetupMerge = always` — a documented git setting — makes
    /// `worktree add -b omh/s01 <path> main` track the **local** `main`. Parsing
    /// that upstream as `remote/branch` yields `main` as a branch name on origin,
    /// it fast-forwards, the read-back passes because it checks the ref it just
    /// pushed, and unreviewed agent work is on trunk.
    #[test]
    fn an_upstream_that_is_not_on_origin_is_never_read_as_a_branch_name() {
        let (d, root) = repo_with_origin();
        git(&root, &["config", "branch.autoSetupMerge", "always"]).unwrap();
        git(&root, &["push", "-q", "-u", "origin", "main"]).unwrap();
        let s = session_with_a_commit(&root, &d.path().join("wt"), "work.rs");

        let err = s.push(None).unwrap_err();

        assert!(
            err.to_string().contains("not a branch name") || err.to_string().contains("not origin"),
            "got: {err}"
        );
        let on_origin = git(&root, &["ls-remote", "origin", "refs/heads/main"]).unwrap();
        let trunk = git(&root, &["rev-parse", "main"]).unwrap();
        assert!(
            on_origin.starts_with(trunk.trim()),
            "the session branch reached origin/main: {on_origin}"
        );
    }

    /// `worktree add -b` loses to git's DWIM when the base exists only as
    /// `origin/<base>`: the worktree lands on a local branch named after the
    /// base instead. Committing then puts the agent's work on trunk and reports
    /// the branch it never touched.
    #[test]
    fn a_worktree_that_drifted_off_its_branch_is_not_committed_to() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        git(&s.worktree, &["checkout", "-q", "-b", "somewhere-else"]).unwrap();
        std::fs::write(s.worktree.join("work.rs"), "fn main() {}").unwrap();

        let err = s
            .commit(Some("Add the work"), Carried::refusing(&[]))
            .unwrap_err();

        assert!(
            err.to_string().contains("rather than omh/s01"),
            "got: {err}"
        );
    }

    /// The read-back is a detector with no undo. Recording the upstream in the
    /// same breath as the push leaves that record behind when the push did not
    /// reach origin — and `omh s` then reports the branch as published while the
    /// remote holds nothing, which is the `e0a41b8` state the guard exists to
    /// prevent, reproduced by the guard itself.
    #[test]
    fn a_failed_push_leaves_no_claim_that_the_branch_is_published() {
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

        assert!(s.push(Some("fix/tap-guard")).is_err());

        assert_eq!(
            s.published_as().unwrap(),
            None,
            "a push that never reached origin must not claim it did"
        );
    }

    /// `omh auth` and `omh doctor` get a writable directory, not somewhere to
    /// keep work. `diff` already refuses them; so must this.
    ///
    /// Asserting the reason: a scratch directory is not a git repository, so
    /// `add -A` refuses on its own and a bare `is_err()` stays green with the
    /// branch guard deleted.
    #[test]
    fn a_scratch_session_cannot_be_committed() {
        let d = tempfile::tempdir().unwrap();
        let s = Session::scratch(d.path().join("scratch"), "doctor".into());
        std::fs::create_dir_all(&s.worktree).unwrap();
        let err = s
            .commit(Some("anything"), Carried::refusing(&[]))
            .unwrap_err();
        assert!(err.to_string().contains("no branch"), "got: {err}");
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

        let base = default_branch(&root);
        assert_eq!(
            base, "origin/trunk",
            "the remote's answer has to win over the local `main`"
        );
        // Spelled `origin/trunk`, because `trunk` is not a local branch in this
        // fixture and the answer is handed to `rev-list` and `merge-base` by
        // every caller. This asserted the bare name until the day a clone
        // without a local trunk proved that name resolves to nothing.
        assert!(
            git(&root, &["rev-parse", "--verify", "--quiet", &base]).is_ok(),
            "and it has to be a ref this checkout can answer about"
        );
    }

    /// Resuming asks git nothing about the base, so a base gone bad cannot stop
    /// it.
    ///
    /// `omh rm` keeps a branch that holds work, so a session id outlives its
    /// worktree and `ensure` reattaches. Taking the start point before choosing
    /// the arm made that reattachment depend on a name the command it runs
    /// never reads — so a trunk renamed between the two launches turned a
    /// resumable session into an error about `main`.
    #[test]
    fn resuming_does_not_need_the_base_to_resolve() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();
        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();
        s.remove(&root, "main", &d_shadows()).unwrap();

        // the repo renames its trunk while the session is put away
        git(&root, &["branch", "-m", "main", "trunk"]).unwrap();
        assert!(
            git(&root, &["rev-parse", "--verify", "--quiet", "main"]).is_err(),
            "the precondition is that the old base no longer resolves"
        );

        s.ensure(&root, "main")
            .expect("reattaching to a branch that exists asks nothing of the base");
        assert_eq!(
            git(&s.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "omh/s01"
        );
    }

    /// The local branch wins, because it is the one the user can move.
    ///
    /// `main` and `origin/main` are one branch to a person and two refs to git,
    /// and they diverge whenever the remote is ahead. The order is a decision,
    /// not an accident, so it is asserted rather than left to the reading of a
    /// doc comment.
    #[test]
    fn a_base_that_is_both_local_and_remote_means_the_local_one() {
        let (_d, root) = repo();
        let local = git(&root, &["rev-parse", "main"])
            .unwrap()
            .trim()
            .to_string();
        git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "the remote moved on"],
        )
        .unwrap();
        let ahead = git(&root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        git(&root, &["update-ref", "refs/remotes/origin/main", &ahead]).unwrap();
        git(&root, &["update-ref", "refs/heads/main", &local]).unwrap();

        assert_ne!(local, ahead, "the fixture has to actually diverge");
        assert_eq!(
            start_point(&root, "main").unwrap(),
            local,
            "the branch the user can move is the one they meant"
        );
    }

    /// A base that resolves nowhere is an error, and says both reasons.
    #[test]
    fn a_base_that_names_nothing_is_refused() {
        let (_d, root) = repo();
        let err = start_point(&root, "no-such-thing")
            .expect_err("a name git cannot resolve is not a start point");
        let said = format!("{err:#}");
        assert!(
            said.contains("no-such-thing"),
            "the message has to name what it could not resolve: {said}"
        );
    }

    /// Whatever it names has to be a ref this checkout can answer about.
    ///
    /// `origin/HEAD` is read for the *name* and the name is then handed to
    /// `rev-list` and `merge-base` by every caller. In a clone whose local
    /// trunk is absent that name resolves to nothing, so `s diff` cannot take a
    /// merge base and `commits` cannot count — and a count that cannot be taken
    /// was, until recently, a branch omh deleted.
    #[test]
    fn the_default_branch_is_a_ref_that_resolves() {
        let (_d, root) = repo_on("other");
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .unwrap();

        let base = default_branch(&root);
        assert!(
            git(&root, &["rev-parse", "--verify", "--quiet", &base]).is_ok(),
            "every range is measured from {base}, and git cannot resolve it here"
        );
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

        let out = s.diff(&default_branch(&root), What::Summary).unwrap();
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

    /// A checkout whose default branch exists only on the remote.
    ///
    /// An ordinary clone, worked on elsewhere, with the local trunk deleted —
    /// after a merge, say. `origin/HEAD` still names `main` because nothing
    /// updates it, and `main` is what `default_branch` therefore reads.
    ///
    /// This fixture told a different story first: that `git clone --branch
    /// <other>` gets you here. Measured, it does not — that clone points
    /// `origin/HEAD` at `origin/other`, so `default_branch` returns a name that
    /// does resolve and the bug never fires. The fixture only worked because it
    /// then forced `remote set-head`, which no user does. A fixture that has to
    /// stage the precondition by hand is describing something nobody meets.
    fn clone_without_local_trunk() -> (tempfile::TempDir, PathBuf) {
        let (d, src) = repo_on("main");

        let clone = d.path().join("clone");
        git(
            d.path(),
            &[
                "clone",
                "-q",
                src.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(&clone, &["config", "user.email", "t@example.com"]).unwrap();
        git(&clone, &["config", "user.name", "t"]).unwrap();
        git(&clone, &["checkout", "-q", "-b", "feature"]).unwrap();
        git(&clone, &["branch", "-D", "main"]).unwrap();
        (d, clone)
    }

    /// The session must be on the branch omh named, whatever git would rather.
    ///
    /// `worktree add -b <branch> <path> <base>` looks unambiguous and is not:
    /// when `base` has no local ref but exactly one remote has it, git's DWIM
    /// takes over, creates a branch named after the *base*, and ignores `-b`
    /// entirely. Measured against git 2.55.0 — the session lands on `main`,
    /// tracking `origin/main`, which is the one branch omh exists to keep an
    /// agent away from.
    ///
    /// Nothing then works: `diff`, `commit` and `--keep` all refuse a worktree
    /// that is not on its branch, and `omh s` reports a branch that was never
    /// created. `--no-guess-remote` does not help; a resolved commit does.
    #[test]
    fn a_session_lands_on_its_own_branch_when_the_base_is_only_on_the_remote() {
        let (d, root) = clone_without_local_trunk();

        assert!(
            git(&root, &["rev-parse", "--verify", "--quiet", "main"]).is_err(),
            "the precondition is that `main` is not a local branch here"
        );
        assert!(
            git(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "refs/remotes/origin/main"
                ]
            )
            .is_ok(),
            "…but is a branch the remote has, which is what makes git guess"
        );

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main").unwrap();

        assert_eq!(
            git(&s.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "omh/s01",
            "the session is on the branch omh opened, not on the base it forked from"
        );
    }

    /// The same base, in a repo with no remote configured, fails the other way.
    ///
    /// Only the remote-tracking *ref* is there — no `remote.origin`, so git has
    /// nothing to guess from. Handed the bare name it refused outright with
    /// `invalid reference: main` rather than quietly taking the branch over:
    /// one cause, two symptoms, and this was the loud one. Measured. A session
    /// starts either way now, which is what this asserts.
    #[test]
    fn a_base_that_is_only_a_remote_ref_still_opens_a_session() {
        let (d, root) = repo_on("other");
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .unwrap();
        assert!(
            git(&root, &["rev-parse", "--verify", "--quiet", "main"]).is_err(),
            "the precondition is that `main` is not a local branch here"
        );

        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root, "main")
            .expect("a base git can resolve at all is enough to start a session");

        assert_eq!(
            git(&s.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "omh/s01"
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
        s.remove(&root, "master", &d_shadows()).unwrap();
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
        assert_eq!(s.behind(&root, "master").unwrap(), 0);

        for m in ["one", "two"] {
            git(&root, &["commit", "-q", "--allow-empty", "-m", m]).unwrap();
        }
        assert_eq!(s.behind(&root, "master").unwrap(), 2);
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

    #[test]
    fn a_new_session_is_something_you_ask_for() {
        let (_d, wt) = worktrees(&["s01", "s02"]);
        assert_eq!(pick(&wt, Start::Fresh), "s03");
    }

    /// What was recorded reads back; what was not is not invented; and what
    /// omh cannot read is neither.
    ///
    /// The third case is the one worth having a type for. A marker omh wrote
    /// and cannot read back is not the same news as no marker, and reporting
    /// it as "nothing was recorded" is a claim about history rather than about
    /// a failed read.
    #[test]
    fn a_recorded_harness_reads_back_and_a_damaged_one_is_not_absence() {
        let d = tempfile::tempdir().unwrap();
        let runs = d.path();

        assert_eq!(harness_of(runs, "s01"), Ran::NeverRecorded, "nothing yet");
        remember_harness(runs, "s01", "opencode").unwrap();
        assert_eq!(harness_of(runs, "s01"), Ran::Harness("opencode".into()));

        // A relaunch records what it launched, not what it launched last time.
        // `fs::write` truncates, and that is load-bearing: an append would
        // resolve to `opencodeclaude` and resume as neither.
        remember_harness(runs, "s01", "claude").unwrap();
        assert_eq!(harness_of(runs, "s01"), Ran::Harness("claude".into()));

        // A different session is a different answer.
        assert_eq!(harness_of(runs, "s02"), Ran::NeverRecorded);

        // Emptied by a truncated write: damage, not absence.
        std::fs::write(harness_marker(runs, "s01"), "   \n").unwrap();
        assert!(
            matches!(harness_of(runs, "s01"), Ran::CouldNotTell(_)),
            "an emptied marker is not a session that never ran one"
        );

        // Unreadable for any other reason. A directory where the file goes is
        // the uid-independent way to say this — CI runs as root, where
        // permissions prove nothing.
        let d2 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(harness_marker(d2.path(), "s03")).unwrap();
        match harness_of(d2.path(), "s03") {
            Ran::CouldNotTell(why) => assert!(
                why.contains("s03"),
                "the reason names the marker it could not read: {why}"
            ),
            other => panic!("a marker omh cannot read is not absence: {other:?}"),
        }

        // Content that is not a harness name never becomes one. It would
        // otherwise be `argv[0]` of a launch, joined into a path, and printed
        // raw in the error.
        for junk in ["../claude", "cla\u{7}ude", "a/b"] {
            std::fs::write(harness_marker(runs, "s01"), junk).unwrap();
            assert!(
                matches!(harness_of(runs, "s01"), Ran::CouldNotTell(_)),
                "`{junk}` is not a harness name"
            );
        }
    }

    /// Naming one gets that one, whether or not it is the current session.
    ///
    /// This used to end by asserting `pick(&wt, Some("s09"), true) == "s09"` —
    /// *"explicit beats --new"* — pinning which half of a contradiction won.
    /// `Start` has no way to say both, so there is no winner to assert and the
    /// line is gone rather than rewritten.
    #[test]
    fn an_explicit_id_always_wins() {
        let (_d, wt) = worktrees(&["s01", "s02"]);
        assert_eq!(pick(&wt, Start::Named("s01")), "s01");
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

        s.remove(&root, "main", &d_shadows())
            .expect("must clean up what is actually there");
        assert!(!s.worktree.exists(), "the directory must be gone");
        assert_eq!(
            s.branch_exists(&root),
            Ok(true),
            "and the branch still kept"
        );
    }

    #[test]
    fn removing_a_session_that_was_never_created_is_not_an_error() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s09".into());
        s.remove(&root, "main", &d_shadows())
            .expect("nothing to do is not a failure");
    }

    /// The drift count fails where the commit count does, and says so.
    ///
    /// Nothing destructive reads `behind` — it fills a column. It is a
    /// `Result` anyway because it asks git the same question in the same
    /// checkouts, and because the answer reaches JSON, where `0` for *cannot
    /// tell* is a number omh did not have. Written as the twin of
    /// `a_branch_is_kept_when_omh_cannot_count_what_is_on_it`, over the same
    /// fixture, so the pair cannot drift apart.
    #[test]
    fn a_drift_count_omh_cannot_take_is_not_zero_either() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s04".into());
        s.ensure(&root, "main").unwrap();

        assert_eq!(
            s.behind(&root, "main").unwrap(),
            0,
            "a session made from the tip is behind nothing"
        );

        git(&root, &["branch", "-m", "main", "trunk"]).unwrap();
        assert!(
            s.behind(&root, "main").is_err(),
            "a base that does not resolve is a question with no answer, not zero"
        );
    }

    /// …and it has no branch to report either way.
    ///
    /// `omh s rm` builds a session rather than looking one up, so an id nothing
    /// ever created reaches `remove` with a branch name that resolves to
    /// nothing. `rev-list <base>..<branch>` fails for a missing branch exactly
    /// as it does for a missing base, so the rule *a count omh could not take
    /// keeps the branch* answered a question nobody had asked: it reported a
    /// branch kept, over a branch that was never there, and offered
    /// `git log omh/s09` — which fails the same way.
    ///
    /// Kept and dropped are both claims about work. Neither is available here.
    #[test]
    fn a_session_id_nothing_created_has_no_branch_to_keep() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s09".into());

        assert!(
            git(&root, &["rev-parse", "--verify", "omh/s09"]).is_err(),
            "the precondition is that the branch is not there"
        );

        assert_eq!(
            s.remove(&root, "main", &d_shadows()).unwrap().branch,
            Removed::NoBranch,
            "a branch that does not exist was neither kept nor dropped"
        );
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
        s.remove(&root, "main", &d_shadows()).unwrap();

        assert!(!s.worktree.exists());
        assert_eq!(
            git(&root, &["branch", "--list", "omh/*"]).unwrap().trim(),
            ""
        );
    }

    /// A scratch directory must not live among the worktrees, or `omh s`
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
        assert_eq!(s.branch_exists(&root), Ok(true));
    }
}
