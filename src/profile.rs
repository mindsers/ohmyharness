//! One catalogue, and it is personal.
//!
//! ```text
//! ~/.omh/rules/ skills/ commands/ subagents/ hooks/ mcp.json
//! ```
//!
//! Content used to live in three layers of identical shape — `~/.omh/profile`,
//! `<repo>/.omh/profile`, `<repo>/.omh/local` — which meant "where is this
//! skill" had three answers, and `sources` was a union: a later layer could
//! shadow a same-named entry, but nothing could turn one off. The only lever
//! was not installing it globally, which is the opposite of a catalogue.
//!
//! **Hooks are the exception, and the reason is in the capability itself.** A
//! skill is a way *you* work and travels with you across repos. A hook binds to
//! a repo's own commands — `cargo test` here, `pnpm test` next door, one name
//! and two bodies — so a capability that is project-specific by nature has to be
//! declarable where the project is, or the catalogue fills with entries that are
//! only ever right in one place. So the rule is not "no content in the repo", it
//! is **content lives where its scope is**.
//!
//! What that costs, and it is real: a repo can no longer ship a skill, an MCP
//! server or a command to your teammates. What it still shares is its rules
//! file — which for the first time actually reaches the agent — its hooks, and
//! its settings. Recorded, not built: a catalogue entry could carry a `source`
//! and `omh sync` could fetch missing ones, which restores team sharing without
//! putting content back in the repo.
//!
//! Nothing here is ever copied into your home directory. A capability resolves
//! to a list of paths and the launcher bind-mounts them, which is why there is
//! no drift to fight and no daemon to run.

use crate::adapter::Capability;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Which checkout a `repo_id` belongs to — and whether it is still there.
///
/// `repo_id` is `<basename>-<digest of the canonical path>`, one-way on
/// purpose: it must be stable, short and safe as a filename. The cost is that
/// omh cannot look at `omh-cache-repo-5e54b748` and say whose it is, so every
/// artifact keyed that way is disk omh can describe but never attribute.
///
/// Three answers, and the third is the one that matters. **No record is
/// `Unknown`, never `Gone`.** Every checkout on a machine upgrading to this
/// version has no record yet, so reading "no record" as "deleted" would make
/// the first prune delete everything on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// Recorded, and the checkout is still on disk.
    Live(PathBuf),
    /// Recorded, and the path is provably not there any more.
    Gone(PathBuf),
    /// No record, or omh could not tell. Carries what to say about it, and is
    /// never something an automatic removal may act on.
    Unknown(String),
}

/// Read back which checkout an id was recorded against.
///
/// `Ok(None)` is "there is no record", and is a different fact from `Err`,
/// which is "omh could not look". Both end as `Unknown`, but only because
/// `attribution_from` decides that — the shapes stay apart on the way there.
pub fn recorded_checkout(root: &Path, repo_id: &str) -> Result<Option<PathBuf>, String> {
    let at = root.join("checkouts").join(repo_id);
    match std::fs::read_to_string(&at) {
        // Absent is genuinely "no record". Every other read failure is omh
        // unable to look, which must not spell the same thing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} could not be read ({e})", at.display())),
        Ok(body) => {
            let line = body.trim();
            if line.is_empty() {
                // A record with nothing in it is a record omh cannot use. It
                // falls to `Err` rather than `None` so it reads as "could not
                // tell" — a truncated write must never license a removal.
                Err(format!("{} is empty", at.display()))
            } else {
                Ok(Some(PathBuf::from(line)))
            }
        }
    }
}

/// The id a checkout at `repo` is keyed by.
///
/// A free function so the registry, the backfill and `Paths` all compute it
/// one way. Two spellings of this would put a record under an id nothing else
/// looks for, which is a registry that silently records nothing.
pub fn id_for(repo: &Path) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    let full = settled(repo);
    format!("{name}-{:08x}", stable_digest(&full.to_string_lossy()))
}

/// Whether an id was made by the current scheme.
///
/// Ids from before 0.8.0 are a bare basename with no digest (risk 8d), so a
/// digest comparison is meaningless for them — and a basename comparison is
/// exactly the ambiguity 8d existed to end. Knowing which kind an id is, is
/// what keeps the two apart.
fn is_digest_shaped(repo_id: &str) -> bool {
    match repo_id.rsplit_once('-') {
        Some((name, digest)) => {
            !name.is_empty() && digest.len() == 8 && digest.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// Whether a derived attribution may be written down.
///
/// The backfill records what it works out, so the harvest is paid once. That
/// is a **write**, and a command promising "nothing is written" must not do it
/// — `omh --dry-run prune` was permanently recording attributions that a later
/// real run then acted on, which makes the rehearsal the first half of the
/// performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backfill {
    Record,
    DoNot,
}

/// Who owns `repo_id`, using the record if there is one and the evidence omh
/// already wrote if there is not.
pub fn attribution_of(root: &Path, repo_id: &str, backfill: Backfill) -> Attribution {
    let recorded = match recorded_checkout(root, repo_id) {
        Ok(Some(path)) => Ok(Some(path)),
        // No record: fall back to what omh already wrote. A worktree's `.git`
        // pointer names its checkout directly, which is the only evidence that
        // works for ids made before 0.8.0 — they carry no digest to compare.
        Ok(None) => match from_the_worktrees(root, repo_id) {
            Ok(found) => {
                if let (Some(path), Backfill::Record) = (&found, backfill) {
                    // Worked out once. A checkout that later disappears is
                    // still attributable afterwards, which is exactly when it
                    // matters — the pointer goes when the worktree does.
                    let _ = remember(root, repo_id, path);
                }
                Ok(found)
            }
            Err(why) => Err(why),
        },
        Err(why) => Err(why),
    };
    let on_disk = match &recorded {
        Ok(Some(path)) => absent_or_unreachable(path),
        _ => Ok(false),
    };
    attribution_from(recorded, on_disk)
}

/// Whether the checkout is there — with *the whole tree went* kept apart from
/// *this directory went*.
///
/// `still_there` closed the `EACCES` collapse. It cannot close this one: when a
/// volume is ejected the mountpoint's contents cease to exist, so `stat`
/// returns a plain `ENOENT`, exactly as for a checkout somebody deleted. Every
/// artifact belonging to every checkout on that disk would then be reclaimable
/// — caches, containers, networks, and the ssh keys omh minted — on a bare
/// `prune` with no flag and no prompt.
///
/// The tell is the parent. Deleting a checkout leaves the directory it sat in;
/// unmounting takes the whole tree. So an absent path whose parent is *also*
/// absent is a question rather than an answer, and `Err` is how this says so.
///
/// It costs some cleanup: deleting a whole project tree at once now reads as
/// unattributable rather than gone. That is the direction to be wrong in.
fn absent_or_unreachable(path: &Path) -> Result<bool, String> {
    match crate::session::still_there(path) {
        Ok(true) => Ok(true),
        Err(why) => Err(why),
        Ok(false) => match path.parent() {
            // No parent to ask about — the filesystem root. Take the answer.
            None => Ok(false),
            Some(parent) => match crate::session::still_there(parent) {
                Ok(true) => Ok(false),
                Ok(false) => Err(format!(
                    "{} is not there either, so this may be an unmounted disk rather than a \
                     checkout somebody removed",
                    parent.display()
                )),
                Err(why) => Err(why),
            },
        },
    }
}

/// The checkout named by the `.git` pointers under this id's worktrees.
fn from_the_worktrees(root: &Path, repo_id: &str) -> Result<Option<PathBuf>, String> {
    let at = root.join("worktrees").join(repo_id);
    match owning_checkout(&at) {
        // Nothing there claims an owner. Absent evidence, not evidence of
        // absence — the caller turns this into `Unknown`.
        Ok(Ownership::Unclaimed) => Ok(None),
        // More than one checkout, or a pointer omh could not read. `Disputed`
        // is the migration's word for "omh does not get to decide", and it
        // means the same here.
        Ok(Ownership::Disputed(reasons)) => Err(reasons.join("; ")),
        Ok(Ownership::All(owner)) => {
            // A digest-shaped id that disagrees with the checkout its own
            // worktree names is a checkout that has moved: the id describes
            // where it *was*. Saying either one is guessing, so neither is
            // said.
            if is_digest_shaped(repo_id) && id_for(&owner) != repo_id {
                return Err(format!(
                    "its worktrees name {}, which is keyed as {} — the checkout has moved since",
                    owner.display(),
                    id_for(&owner)
                ));
            }
            Ok(Some(owner))
        }
        Err(e) => Err(format!("{e:#}")),
    }
}

/// Record a mapping worked out from evidence, under an id that is not
/// necessarily this process's own checkout.
pub fn remember(root: &Path, repo_id: &str, path: &Path) -> std::io::Result<()> {
    let dir = root.join("checkouts");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{repo_id}.tmp"));
    std::fs::write(&tmp, format!("{}\n", path.display()))?;
    std::fs::rename(&tmp, dir.join(repo_id))
}

/// The decision, over answers someone else went and got.
///
/// Split from the reading for the reason `git_checks_from` is: the part that
/// can be wrong silently is this one, and it is a table.
pub fn attribution_from(
    recorded: Result<Option<PathBuf>, String>,
    on_disk: Result<bool, String>,
) -> Attribution {
    match recorded {
        Err(why) => Attribution::Unknown(format!(
            "omh could not read its record of this checkout: {why}"
        )),
        Ok(None) => Attribution::Unknown(
            "omh has no record of which checkout this belongs to — it predates the \
             record, or was never set up by this omh"
                .to_string(),
        ),
        Ok(Some(path)) => match on_disk {
            Ok(true) => Attribution::Live(path),
            Ok(false) => Attribution::Gone(path),
            // Asked of the disk properly, never `Path::exists`: that is
            // `metadata().is_ok()`, so a checkout on a mount that has gone
            // away answers "not there" and everything it owns reads as
            // reclaimable.
            Err(why) => Attribution::Unknown(format!(
                "recorded as {}, but omh could not tell whether it is still there: {why}",
                path.display()
            )),
        },
    }
}

pub struct Paths {
    pub root: PathBuf,
    pub repo: PathBuf,
}

impl Paths {
    /// `~/.omh`, without needing a repo.
    ///
    /// `discover` refuses outside a git repository, correctly — a session is a
    /// worktree. Two things are true outside one: the catalogue exists, and so
    /// does the template a new repo is seeded from. Those callers need this.
    pub fn home() -> Result<PathBuf> {
        Ok(dirs::home_dir().context("no home directory")?.join(".omh"))
    }

    /// A `Paths` that works outside a repository.
    ///
    /// `repo` is the cwd when there is no repo, which is honest: the caller
    /// that needs this — `omh settings` — touches only `root`, and a command
    /// that reaches for `repo` here would be asking a question with no answer.
    pub fn anywhere(cwd: &Path) -> Result<Self> {
        Ok(Self {
            root: Self::home()?,
            repo: repo_root(cwd).unwrap_or_else(|_| cwd.to_path_buf()),
        })
    }

    pub fn discover(cwd: &Path) -> Result<Self> {
        let home = dirs::home_dir().context("no home directory")?;
        Ok(Self {
            root: home.join(".omh"),
            repo: repo_root(cwd)?,
        })
    }

    pub fn adapters(&self) -> PathBuf {
        self.root.join("adapters")
    }

    pub fn editors(&self) -> PathBuf {
        self.root.join("editors")
    }

    /// The stacks as shipped: what a project needs installed, and how the image
    /// gets it. Managed files, refreshed on every `init` like the adapters — a
    /// local edit that fixes one ecosystem leaves omh broken for everybody else
    /// using it, so the fix belongs upstream.
    pub fn stacks(&self) -> PathBuf {
        self.root.join("stacks")
    }

    /// Ecosystems **this repo** taught omh, for the one case a release never
    /// will: a proprietary internal toolchain. Written by `init` from an answer
    /// somebody typed, read beside the shipped ones, and unable to answer to a
    /// name omh ships.
    pub fn repo_stacks(&self) -> PathBuf {
        self.repo.join(".omh").join("stacks")
    }

    /// Files omh recognises as naming an ecosystem it cannot yet set up. A
    /// question, not an answer — see `stack::Marker`.
    pub fn markers(&self) -> PathBuf {
        self.root.join("markers")
    }

    /// The conventional hooks as shipped — `cargo test`, `gofmt -w .` — living
    /// in the catalogue beside the ones you write, because that is where their
    /// scope is: `cargo test` is what a rust project runs, not what *this* rust
    /// project runs.
    ///
    /// Managed like the stacks and the adapters, so a fix omh ships reaches
    /// somebody who ran `init` a year ago. A repo that needs its own spelling
    /// writes `<repo>/.omh/hooks/<name>.json`, which shadows this by the rule
    /// `merge_hooks` already applies.
    ///
    /// Deliberately the same directory `Capability::Hooks` sources, not a
    /// parallel one: a second place hooks can live is a second precedence rule
    /// to explain, and the shadowing already says what to do about a clash.
    pub fn hooks(&self) -> PathBuf {
        self.root.join(Capability::Hooks.source())
    }

    /// The base set as shipped: what `init` seeds and what `omh why` explains.
    /// Versioned files, oldest kept, so an upgrade can eventually diff two.
    pub fn base(&self) -> PathBuf {
        self.root.join("base")
    }

    pub fn creds(&self, harness: &str) -> PathBuf {
        self.root.join("creds").join(harness)
    }

    /// Outside the repo on purpose: nested worktrees make your IDE index every
    /// session's full copy of the codebase.
    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees").join(self.repo_id())
    }

    /// Per-launch staging. Keyed by repo as well as session and harness: two
    /// checkouts both on `s01` must not share a rendered profile.
    pub fn staging(&self, session: &str, harness: &str) -> PathBuf {
        self.runs().join(session).join(harness)
    }

    /// Per-repo run state: staged profiles, and the marker recording when each
    /// session was last used.
    pub fn runs(&self) -> PathBuf {
        self.root.join("run").join(self.repo_id())
    }

    /// A throwaway working directory, deliberately outside `worktrees/` so a
    /// login never appears in `omh s` as a session you could resume.
    pub fn scratch(&self, name: &str) -> PathBuf {
        self.root.join("scratch").join(self.repo_id()).join(name)
    }

    pub fn keys(&self) -> PathBuf {
        self.root.join("keys").join(self.repo_id())
    }

    /// Where the sandbox's own repositories live — one gitdir per session,
    /// plus the seed each was created from.
    ///
    /// A tree of its own rather than a sibling of the worktree it serves:
    /// `session::list` reports every directory under `worktrees()` as a
    /// session, so an `s01.git` beside `s01` would show up in `omh s` as a
    /// session you could resume.
    ///
    /// `next_id` would *not* be fooled — it parses the name after `s` as a
    /// number and `01.git` does not parse — but that is one enumerator getting
    /// lucky, not a reason to put them together.
    pub fn shadows(&self) -> PathBuf {
        self.root.join("shadow").join(self.repo_id())
    }

    /// The local note store — keyed by repo, and outside the checkout so it
    /// outlives the worktree that produced it. A session is a git worktree
    /// holding tracked files only, and `omh s rm` removes it with `--force`,
    /// so a gitignored store inside the repo would be both invisible to the
    /// sandbox and destroyed by session removal.
    ///
    /// The committed half of the store is not here: it is tracked, so it
    /// belongs in the repo, and it arrives in every worktree by itself.
    pub fn notes(&self) -> PathBuf {
        self.root.join("notes").join(self.repo_id())
    }

    /// Cache volume — keyed by repo, deliberately not by harness. This is what
    /// lets memory survive a harness switch.
    pub fn cache_volume(&self) -> String {
        format!("omh-cache-{}", self.repo_id())
    }

    /// The directory of checkout records: `<repo_id>` → the path it came from.
    pub fn checkouts(&self) -> PathBuf {
        self.root.join("checkouts")
    }

    /// Record where this checkout is, so what it leaves behind can be traced
    /// back to it.
    ///
    /// `repo_id` is one-way, so without this every artifact omh keys by it is
    /// disk omh can describe and never attribute. Written on the canonical
    /// path, because that is what the digest was taken over.
    pub fn remember_checkout(&self) -> std::io::Result<()> {
        // Written whole or not at all: a half-written record is a path that
        // resolves to somewhere else, and this value decides what gets
        // removed. `recorded_checkout` refuses an empty one for the same
        // reason.
        remember(&self.root, &self.repo_id(), &settled(&self.repo))
    }

    /// The network a scratch verb shares: the per-repo `omh-<repo_id>`.
    ///
    /// `omh auth` and `omh doctor` run a one-shot `--rm` container and need a
    /// network, but not isolation from each other — they hold no services. A
    /// per-session network for each would be a leftover nothing removes (they
    /// are not in the session list) and one `id_in_network` would misattribute
    /// to a phantom `<repo>-auth`. One shared, reused network avoids both, and
    /// is what these verbs used before per-session networks existed.
    pub fn scratch_network(&self) -> String {
        format!("omh-{}", self.repo_id())
    }

    /// The session's own network, named like its container. One per session
    /// rather than one per checkout: two sessions of the same repo have no
    /// business reaching each other's services, and on a shared network they
    /// could. The per-repo `omh-<repo_id>` older versions made is left for
    /// `omh prune` to find; `id_in_network` still reads that form.
    pub fn session_network(&self, session: &str) -> String {
        format!("omh-{}-{session}", self.repo_id())
    }

    pub fn container(&self, session: &str) -> String {
        format!("omh-{}-{session}", self.repo_id())
    }

    pub fn repo_name(&self) -> String {
        self.repo_id()
    }

    /// What every piece of per-repo state is keyed by.
    ///
    /// The checkout's basename, and a digest of where it actually is. It was
    /// the basename alone until 2026.08, which made `~/work/api` and
    /// `~/oss/api` one repo — sharing worktrees, sandbox repositories, the
    /// note store, the cache volume, the network and the container name, so
    /// the second checkout's `omh new` resumed into the first one's session.
    /// That is risk 8d, and this function is the whole of it: nine accessors
    /// route through here and nothing else composes a repo key.
    ///
    /// **The name stays in front** because these are read by people. `omh s`
    /// prints them and `docker ps` lists them, and `omh-3f9a2c1b-s01` tells
    /// nobody which checkout it belongs to. The digest disambiguates; the name
    /// is what makes the answer legible.
    ///
    /// Canonicalised, so a checkout reached through a symlink is the same repo
    /// as the checkout itself rather than a second one with its own sessions.
    fn repo_id(&self) -> String {
        id_for(&self.repo)
    }
}

/// The canonical form of a path, whether or not all of it exists yet.
///
/// A plain `canonicalize()` here was wrong in a way worth recording, because
/// it looked right and the guard written for it passed. It fails for a path
/// that does not exist, so the fallback was the path as given — which meant
/// **the answer changed the moment the directory came into being**. A repo id
/// computed before `mkdir` and again after was two different ids, and since
/// that id names the note store, the worktrees and the sandbox repository, the
/// state written under the first one simply stopped being found.
///
/// The suite caught it and the new test did not: seven memory tests failed
/// because seeding a team note creates `<repo>/.omh/notes`, which creates
/// `<repo>` — so notes seeded before and after that line landed in two
/// different stores. The guard that was supposed to cover this asserted an id
/// twice over a directory that existed both times, which is the easy half.
///
/// So: canonicalise the longest prefix that does exist and re-attach the rest.
/// `/tmp/x/repo` and `/private/tmp/x/repo` agree before `repo` is created and
/// after, and a symlinked checkout still resolves to the thing it points at.
pub(crate) fn settled(path: &Path) -> PathBuf {
    let mut suffix = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = at.canonicalize() {
            let mut out = real;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (at.file_name(), at.parent()) {
            (Some(name), Some(parent)) => {
                suffix.push(name.to_os_string());
                at = parent;
            }
            // Nothing on this path resolves — a relative path with no existing
            // ancestor, or the root itself refusing. The path as given is the
            // only deterministic answer left, and it is still a stable one.
            _ => return path.to_path_buf(),
        }
    }
}

/// Every kind of per-repo state, as `<root>/<kind>/<repo id>`.
///
/// The list the migration walks, and the reason `repo_id` is worth getting
/// right: these are the six directories a checkout's identity names. Kept
/// beside the accessors that build them so adding a seventh is a change in one
/// place — `worktrees`, `runs`, `keys`, `shadows`, `notes` and `scratch` each
/// join one of these.
pub(crate) const KEYED: [&str; 6] = ["worktrees", "run", "keys", "shadow", "notes", "scratch"];

/// What the one-time move from basename keying to digest keying did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Migration {
    /// No directory under the old key, or it has already run.
    NothingToDo,
    /// These kinds moved from the old key to the new one.
    ///
    /// `stranded` is what could **not** move in the same run, because the new
    /// key already holds it. Carried here rather than reported on some later
    /// run because it was: `blocked` used to be read only when nothing could
    /// move, so a mixed run said "moved this checkout's notes" and never
    /// mentioned the worktrees that had just become unreachable.
    Moved {
        from: String,
        kinds: Vec<String>,
        stranded: Vec<String>,
    },
    /// Something is under the old key that nothing will ever look at, because
    /// the new key is already in use and omh will not merge two directories
    /// of sessions together.
    ///
    /// Reported rather than ignored, and rather than tidied away. Ignoring it
    /// is how state becomes invisible instead of absent — the shape of every
    /// other defect this release closed — and merging it is a guess about
    /// which of two directories a session belongs to, made silently, in the
    /// one place where being wrong costs an agent's unharvested commits.
    Stranded { from: String, kinds: Vec<String> },
    /// Something is there and omh will not touch it. Says why, in a sentence
    /// meant for the person who has to decide.
    Refused(String),
}

/// Move a checkout's state from basename keying onto its own id.
///
/// omh keyed everything by the checkout's directory name until 2026.08, so an
/// install upgrading into this has `~/.omh/worktrees/api` where it now looks
/// for `~/.omh/worktrees/api-3f9a2c1b`. Without this the sessions, notes and
/// sandbox repositories under the old name become unreachable — not lost, but
/// invisible, which for a directory holding an agent's commits is close
/// enough.
///
/// **Ownership is read, never assumed.** A worktree's `.git` is a file saying
/// `gitdir: <checkout>/.git/worktrees/<id>`, so the old directory names the
/// checkout it belongs to and omh does not have to guess. The whole point of
/// risk 8d is that two checkouts can answer to one old key; adopting on
/// proximity would hand one of them the other's sessions, which is the bug
/// rather than the fix.
///
/// Three answers, and the middle one is the one worth stating:
///
/// - a pointer naming **this** checkout — move everything.
/// - a pointer naming **another** checkout — refuse and say so. The other
///   checkout will claim it when it next runs, and taking it here would be
///   the collision, performed deliberately.
/// - **no worktrees at all** — move the rest. There is no session to collide
///   over, and the realistic case is a repo that ran `init` and never `new`:
///   stranding its notes silently is worse than adopting a directory nothing
///   else is asking for.
///
/// `is_running` is injected rather than read here so this stays a pure
/// filesystem function with no runtime behind it — the same reason
/// `Runtime::running_args` returns arguments instead of running them.
pub fn migrate(paths: &Paths, is_running: &dyn Fn(&str) -> bool) -> Result<Migration> {
    let new = paths.repo_id();
    let old = paths
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    if old == new {
        return Ok(Migration::NothingToDo);
    }

    let here = |kind: &str, id: &str| paths.root.join(kind).join(id);
    let under_old: Vec<&str> = KEYED
        .into_iter()
        .filter(|kind| here(kind, &old).is_dir())
        .collect();
    let (pending, blocked): (Vec<&str>, Vec<&str>) = under_old
        .into_iter()
        .partition(|kind| !here(kind, &new).exists());

    if pending.is_empty() {
        // Nothing can move. If something is still sitting under the old key,
        // say so — it is invisible to every command from here on, and silence
        // is what made the collision this whole change is about survive three
        // weeks of use.
        return Ok(match blocked.is_empty() {
            true => Migration::NothingToDo,
            false => Migration::Stranded {
                from: old,
                kinds: blocked.into_iter().map(str::to_string).collect(),
            },
        });
    }

    // Before anything moves. A running container's mounts point at the
    // worktree path about to change underneath it, and docker would keep
    // serving the old inode while omh reported a successful move — a session
    // live on a directory neither of them can name. Refusing is a sentence;
    // the alternative is a class of bug nobody could reproduce.
    if is_running(&old) {
        return Ok(Migration::Refused(format!(
            "a sandbox from before this version is still running under `{old}`. \
             omh will not move a session's worktree out from under a live container — \
             `omh s down` first, then run this again"
        )));
    }

    let worktrees = paths.root.join("worktrees").join(&old);
    match owning_checkout(&worktrees)? {
        // Nothing there claims an owner, so there is no session to collide
        // over — the `init`-only repo, whose notes would otherwise strand.
        Ownership::Unclaimed => {}
        Ownership::All(owner) if owner == settled(&paths.repo) => {}
        Ownership::All(owner) => {
            return Ok(Migration::Refused(format!(
                "`{}` holds sessions belonging to {}, not this checkout. Two checkouts \
                 named `{old}` shared one directory before this version, and omh will not \
                 decide which of them gets it — that other checkout claims it the next \
                 time it runs omh",
                worktrees.display(),
                owner.display()
            )));
        }
        // Two checkouts' sessions in one directory is the *ordinary* shape of
        // risk 8d, not the exotic one: `next_id` scanned the shared directory,
        // so the second checkout took `s02` rather than its own `s01`. Moving
        // the directory would hand one of them the other's work.
        Ownership::Disputed(why) => {
            return Ok(Migration::Refused(format!(
                "omh will not move `{}` — it cannot establish that it is this \
                 checkout's:\n    {}\n  Two checkouts named `{old}` shared one \
                 directory before this version, and sessions from both can be in \
                 there. Move the ones you want by hand, or remove what you do not.",
                worktrees.display(),
                why.join("\n    ")
            )));
        }
    }

    // Renamed one at a time, and a failure has to say what already moved.
    // Without `moved` in the message the caller reports "omh could not move
    // this checkout's state, so anything under it is invisible" — which is
    // false for every kind that did move, and sends the user to look under
    // the old key for state that is no longer there.
    let mut moved: Vec<&str> = Vec::new();
    for kind in &pending {
        let from = paths.root.join(kind).join(&old);
        let to = paths.root.join(kind).join(&new);
        std::fs::create_dir_all(to.parent().context("a keyed root has a parent")?)?;
        std::fs::rename(&from, &to).with_context(|| {
            format!(
                "moving {} to {}{}",
                from.display(),
                to.display(),
                match moved.is_empty() {
                    true => String::new(),
                    false => format!(
                        " — this checkout's {} did move, and are on the new key;                          its state is split across both until this is resolved",
                        moved.join(", ")
                    ),
                }
            )
        })?;
        moved.push(kind);
    }
    Ok(Migration::Moved {
        from: old,
        kinds: pending.into_iter().map(str::to_string).collect(),
        // Said in the same breath as what did move. Reported on a later run
        // is not good enough: the run that says "moved your notes" is the one
        // the user reads.
        stranded: blocked.into_iter().map(str::to_string).collect(),
    })
}

/// Who owns the sessions under an old-key worktrees directory.
///
/// Three answers, and keeping them apart is the whole of what stops this
/// migration doing the thing it exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ownership {
    /// Nothing there claims an owner — an empty directory, or one a hand
    /// `git worktree remove` already emptied. Nothing to collide over.
    Unclaimed,
    /// Every session that said, said this checkout.
    All(PathBuf),
    /// More than one checkout, or a pointer omh could not read. Either way
    /// omh does not get to decide; the reasons are for the person who does.
    Disputed(Vec<String>),
}

/// Read every worktree's `.git` pointer and say who owns them.
///
/// `git worktree add` writes a `.git` **file** holding
/// `gitdir: <checkout>/.git/worktrees/<name>`, so the answer is on disk and
/// does not have to be inferred.
///
/// **Every pointer, not the first.** The first version returned on the first
/// one it could read, which is sampling: pre-2026.08 `next_id` scanned the
/// shared directory, so two checkouts named `api` took `s01` and `s02` *in
/// one directory*, and `read_dir` order decided which of them adopted the
/// other's sessions. Measured against the release binary — `oss/api` took
/// `work/api`'s `s01`, `work/api` was left reporting `no sessions`.
///
/// **A pointer omh cannot read is `Disputed`, never `Unclaimed`.** Read
/// failures used to collapse onto "nobody owns this", which `migrate` reads
/// as permission to proceed — so an unreadable directory belonging to another
/// checkout was adopted. "Cannot look" must not spell the same as "nobody is
/// there" in the one place where being wrong moves somebody else's commits.
fn owning_checkout(worktrees: &Path) -> Result<Ownership> {
    let entries = match std::fs::read_dir(worktrees) {
        Ok(entries) => entries,
        // Absent is genuinely nobody. Anything else is omh unable to look,
        // and that is a refusal.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Ownership::Unclaimed),
        Err(e) => {
            return Ok(Ownership::Disputed(vec![format!(
                "{} could not be read ({e})",
                worktrees.display()
            )]))
        }
    };

    let mut owners: Vec<PathBuf> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                unreadable.push(format!("an entry could not be read ({e})"));
                continue;
            }
        };
        // **Stat failures are evidence, not silence.** The first version of
        // this filtered with `if !entry.is_dir()`, and `Path::is_dir()`
        // answers `false` for every error — a dangling symlink, EACCES, a
        // racing removal. So an entry omh could not look at contributed to
        // neither list and vanished before the accounting, and a directory
        // holding another checkout's session was adopted with nothing said.
        // Measured, and it is how this function reintroduced the bug it was
        // written to fix.
        match std::fs::metadata(&entry) {
            // Not a directory, so not a worktree, so not evidence either way.
            Ok(meta) if !meta.is_dir() => continue,
            Ok(_) => {}
            Err(e) => {
                unreadable.push(format!("{} could not be examined ({e})", entry.display()));
                continue;
            }
        }

        // Absent is not unreadable, and the difference decides the answer.
        // A directory with no `.git` claims nothing — an emptied worktree, an
        // editor's scratch folder — and blocking on those would strand the
        // notes of every repo with clutter in its worktrees directory. A
        // `.git` that exists and cannot be read is omh unable to look, which
        // is the one thing it must never spell as "nobody is there".
        let pointer = entry.join(".git");
        match std::fs::read_to_string(&pointer) {
            Ok(body) => match owner_of(body.trim(), &entry) {
                Some(owner) if !owners.contains(&owner) => owners.push(owner),
                Some(_) => {}
                None => unreadable.push(format!(
                    "{} is not a `gitdir:` pointer, so it does not name a checkout",
                    pointer.display()
                )),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => unreadable.push(format!("{} could not be read ({e})", pointer.display())),
        }
    }

    Ok(match (owners.len(), unreadable.is_empty()) {
        (0, true) => Ownership::Unclaimed,
        (1, true) => Ownership::All(owners.remove(0)),
        _ => Ownership::Disputed(
            owners
                .into_iter()
                .map(|o| format!("{} owns one", o.display()))
                .chain(unreadable)
                .collect(),
        ),
    })
}

/// The checkout a `gitdir:` pointer names.
///
/// `<checkout>/.git/worktrees/<name>` — everything before `/.git/` is the
/// checkout. Matched on the component rather than by counting parents,
/// because a bare or relocated gitdir has a different depth.
///
/// **`beside` is the directory holding the pointer, and a relative body is
/// resolved against it.** git ≥ 2.48 writes relative pointers under
/// `worktree.useRelativePaths` and `git worktree add --relative-paths`, and
/// the first version handed those straight to `settled` — which canonicalises
/// a relative path against the *omh process's working directory*. Ownership
/// was therefore computed from wherever the user ran the command, and the
/// answer moved with the shell. It refused rather than adopting, but only
/// because a cwd-derived path happens not to match; the point of this
/// function is that ownership is read rather than inferred, and being right
/// by coincidence is not reading.
fn owner_of(body: &str, beside: &Path) -> Option<PathBuf> {
    let gitdir = Path::new(body.strip_prefix("gitdir:")?.trim());
    let absolute = match gitdir.is_absolute() {
        true => gitdir.to_path_buf(),
        false => beside.join(gitdir),
    };
    let mut at = absolute.as_path();
    while let Some(parent) = at.parent() {
        if at.file_name().is_some_and(|n| n == ".git") {
            return Some(settled(parent));
        }
        at = parent;
    }
    None
}

/// A digest that will still be the same digest in five years.
///
/// FNV-1a, written out, for a reason worth stating: this value **names
/// directories on disk** — the worktrees a session lives in, the sandbox
/// repository holding every commit an agent made. A digest that changed would
/// not corrupt anything, it would do something worse and quieter: strand the
/// lot, and open a fresh empty session where the user's work used to be.
///
/// So the two obvious choices are both wrong here.
///
/// `DefaultHasher` is what `ssh::port` and `base::ui_port` use, and it is
/// right for them — those derive a *port*, recomputed every run, where drift
/// costs a moved bookmark. std explicitly does not guarantee its output across
/// releases, which `container::labels` already refuses it for on exactly this
/// reasoning: it would restart every running session on the day somebody
/// upgrades Rust. Here it would strand them instead.
///
/// `image::recipe_digest` is stable — `git hash-object` is a fixed SHA-1 for
/// ever — but it spawns a process, and `repo_id` is called from nine path
/// accessors many times per command. Correct and unaffordable.
///
/// Written out, the algorithm is ours and cannot move under us. The test
/// pinning a known vector is not ceremony: it is the only thing standing
/// between a tidy-up of this function and every existing session becoming
/// unreachable.
fn stable_digest(s: &str) -> u32 {
    // FNV-1a, 32-bit. The constants are the specification's.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in s.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub struct Profile {
    /// The catalogue — yours, every project.
    root: PathBuf,
    /// This checkout, which declares hooks and nothing else.
    repo: PathBuf,
}

impl Profile {
    pub fn resolve(paths: &Paths) -> Self {
        Self {
            root: paths.root.clone(),
            repo: paths.repo.clone(),
        }
    }

    /// Where `cap` is declared, in application order — so a later path wins.
    ///
    /// One entry for five of the six. Hooks get a second because they are the
    /// one capability with a repo tier, and the repo's come **last**: a project
    /// overrides your personal `format` hook with the one it actually needs,
    /// without either being renamed.
    ///
    /// Absent paths are skipped, so an empty result means "nothing declared" —
    /// and it stays a `Vec` rather than an `Option` because every caller
    /// downstream merges a list, and hooks would have needed the list anyway.
    ///
    /// **Absent is not unreadable**, which `Path::exists()` cannot express: it
    /// answers `false` for a dangling symlink, an `EACCES` parent and an
    /// unmounted share alike. Read as "nothing declared" the launcher skips the
    /// capability, mounts nothing, reports nothing dropped and exits 0 — and
    /// `doctor` agrees, because it branches on the same empty list. With one
    /// catalogue that is every skill, rule, command, subagent and server at
    /// once, so `try_exists` and a `Result` rather than a silent `false`.
    pub fn sources(&self, cap: Capability) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for path in self.candidates(cap) {
            if path
                .try_exists()
                .with_context(|| format!("reading {}", path.display()))?
            {
                out.push(path);
            }
        }
        Ok(out)
    }

    fn candidates(&self, cap: Capability) -> Vec<PathBuf> {
        let mut out = vec![self.root.join(cap.source())];
        if cap == Capability::Hooks {
            out.push(self.repo.join(".omh").join(cap.source()));
        }
        out
    }

    /// What this capability actually holds, by name.
    ///
    /// The names `[use]` selects from, `init` writes expanded, and the launcher
    /// reports as unselected — one function, because a name that is spelled one
    /// way by the report and another by the file that fixes it is worse than no
    /// report at all.
    ///
    /// `mcp.json` is the lone irregular case: a server is a record inside a file
    /// rather than a file in a directory, and it is read through the same parser
    /// the renderer uses rather than a second one that could disagree about what
    /// counts as a server.
    pub fn entries(&self, cap: Capability) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        for source in self.sources(cap)? {
            if cap == Capability::Mcp {
                out.extend(crate::render::parse_layers(&[source])?.into_keys());
                continue;
            }
            let entries = std::fs::read_dir(&source)
                .with_context(|| format!("reading {}", source.display()))?;
            for entry in entries {
                // Not `.flatten()`, the rule this codebase follows for every
                // directory it lists: a `readdir` failing part-way through would
                // silently shorten the catalogue, and the report built from it
                // would say an entry is not selected because it was never seen.
                let entry = entry.with_context(|| format!("reading {}", source.display()))?;
                let name = entry_name(&entry.file_name());
                // A name omh mints has to be a name a `[use]` list can hold, or
                // `init` writes something every later read refuses. `.DS_Store`
                // is the one that actually happens: Finder creates it in any
                // directory somebody opens, and it bricked the repo.
                //
                // Skipped rather than reported, because it is not an entry that
                // went missing — it is a file that was never a catalogue entry,
                // and a launch that named it would be telling the user about
                // their operating system.
                if crate::selection::validate_entry_name(&name, cap, &source).is_err() {
                    continue;
                }
                out.push(name);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Capabilities the profile actually carries.
    ///
    /// Not currently called outside tests: the launcher reports dropped
    /// capabilities from the adapter side instead. Kept because it is the
    /// profile-side half of that answer and `omh eject` will need it.
    #[allow(dead_code)]
    pub fn declared(&self) -> Result<Vec<Capability>> {
        let mut out = Vec::new();
        for cap in Capability::ALL {
            if !self.sources(cap)?.is_empty() {
                out.push(cap);
            }
        }
        Ok(out)
    }
}

/// The catalogue name of one directory entry.
///
/// A skill is a directory and a rule is a file, so the extension comes off and
/// nothing else does. One function rather than a `file_stem()` at each site,
/// because the name a `[use]` list matches, the name a launch reports as
/// unselected and the name `omh use` writes have to be the same string — three
/// spellings of "drop the extension" is three chances for a skill called
/// `review.diff` to be selectable under one name and reported under another.
pub fn entry_name(file_name: &std::ffi::OsStr) -> String {
    let path = Path::new(file_name);
    path.file_stem()
        .unwrap_or(file_name)
        .to_string_lossy()
        .into_owned()
}

/// Walk up looking for `.git`. The worktree model needs a real repo, so a
/// missing one is a hard error rather than a silent fallback to `cwd`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let mut cur = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        if !cur.pop() {
            anyhow::bail!(
                "{} is not inside a git repository\n\
                 omh isolates the agent on a worktree branch, which needs one.\n\
                 run `git init` first.",
                start.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {

    /// A worktree's own `.git` pointer names its checkout, so an id with no
    /// record can still be attributed from what omh already wrote.
    ///
    /// This is the whole of the backfill: without it the registry only helps
    /// checkouts set up *after* it landed, and every artifact already on a
    /// machine stays unattributable for ever.
    ///
    /// Measured while designing this, and worth recording because it removed a
    /// planned source: a container's `omh.mounts` stamp does **not** help. Its
    /// `/work` mount is the worktree, `~/.omh/worktrees/<repo_id>/sNN`, so it
    /// yields the id omh already had rather than the checkout — circular.
    #[test]
    fn a_checkout_named_by_a_worktree_pointer_is_attributed_without_a_record() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("home/.omh");
        let checkout = d.path().join("work/api");
        std::fs::create_dir_all(checkout.join(".git/worktrees/s01")).unwrap();

        let id = super::id_for(&checkout);
        let wt = root.join("worktrees").join(&id).join("s01");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/s01\n", checkout.display()),
        )
        .unwrap();

        // No record at all — only the pointer omh wrote when it made the
        // worktree.
        assert_eq!(super::recorded_checkout(&root, &id), Ok(None));
        assert_eq!(
            super::attribution_of(&root, &id, super::Backfill::Record),
            super::Attribution::Live(super::settled(&checkout)),
            "the pointer names the checkout, and it is still there"
        );

        // And the answer is written back, so the next run does not re-derive
        // it and a checkout that later disappears is still attributable.
        assert_eq!(
            super::recorded_checkout(&root, &id),
            Ok(Some(super::settled(&checkout))),
            "what was worked out once is recorded"
        );

        // With the checkout gone, the same id is `Gone` — the one answer a
        // removal may act on. Only the checkout: removing `work/` too would be
        // a whole tree vanishing, which is deliberately a different answer.
        std::fs::remove_dir_all(&checkout).unwrap();
        assert_eq!(
            super::attribution_of(&root, &id, super::Backfill::Record),
            super::Attribution::Gone(super::settled(&checkout))
        );
    }

    /// An unplugged drive is not a deleted checkout.
    ///
    /// `still_there` fixed the `EACCES` collapse, but an ejected volume is
    /// plainer than that: the mountpoint's contents simply cease to exist, so
    /// `stat` says `ENOENT` — identical to a checkout somebody deleted. Unplug
    /// the disk your work lives on, run a bare `omh prune`, and every checkout
    /// on it reads as `Gone`: caches, containers, networks and the ssh keys omh
    /// minted for them, with no flag and no prompt.
    ///
    /// The tell is the parent. Deleting a checkout leaves the directory it sat
    /// in; unmounting takes the whole tree. So an absent path whose parent is
    /// *also* absent is a question, not an answer.
    #[test]
    fn a_checkout_whose_whole_tree_vanished_is_not_reported_gone() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join(".omh");

        // Deleted the ordinary way: `work/` survives it.
        let deleted = d.path().join("work/api");
        std::fs::create_dir_all(&deleted).unwrap();
        let id = super::id_for(&deleted);
        super::remember(&root, &id, &deleted).unwrap();
        std::fs::remove_dir_all(&deleted).unwrap();
        assert!(
            matches!(
                super::attribution_of(&root, &id, super::Backfill::Record),
                super::Attribution::Gone(_)
            ),
            "a checkout removed from a directory that is still there is gone"
        );

        // The drive went: the mountpoint and everything above it with it.
        let ejected = d.path().join("Volumes/Backup/api");
        std::fs::create_dir_all(&ejected).unwrap();
        let eid = super::id_for(&ejected);
        super::remember(&root, &eid, &ejected).unwrap();
        std::fs::remove_dir_all(d.path().join("Volumes/Backup")).unwrap();
        match super::attribution_of(&root, &eid, super::Backfill::Record) {
            super::Attribution::Unknown(why) => assert!(
                why.contains("Backup") || why.contains("not there either"),
                "and it says what it could not find: {why}"
            ),
            other => panic!("an unplugged drive is not a deleted checkout: {other:?}"),
        }
    }

    /// Evidence omh cannot read one way is not evidence of nobody.
    ///
    /// Both of these end as `Unknown`, so neither is a removal hazard — what
    /// they must not lose is *why*, because that reason is the whole of what
    /// the report gives somebody deciding by hand.
    #[test]
    fn evidence_that_disagrees_is_a_reason_not_an_absence() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("home/.omh");

        // Two checkouts claiming worktrees under one id: the pre-0.8.0 state
        // that risk 8d is about. omh does not get to pick one.
        let disputed = "shared-1a2b3c4d";
        for (n, owner) in [("s01", "one"), ("s02", "two")] {
            let wt = root.join("worktrees").join(disputed).join(n);
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(
                wt.join(".git"),
                format!(
                    "gitdir: {}/{owner}/.git/worktrees/{n}\n",
                    d.path().display()
                ),
            )
            .unwrap();
        }
        match super::attribution_of(&root, disputed, super::Backfill::Record) {
            super::Attribution::Unknown(why) => assert!(
                why.contains("one") && why.contains("two"),
                "it names both claims rather than saying nothing is there: {why}"
            ),
            other => panic!("two owners is not an answer: {other:?}"),
        }

        // A digest-shaped id whose worktree names a checkout that hashes to a
        // *different* id: the checkout moved, so the id describes where it
        // was. Saying either is guessing.
        let moved = d.path().join("moved/api");
        std::fs::create_dir_all(moved.join(".git")).unwrap();
        let stale = "api-00000000";
        let wt = root.join("worktrees").join(stale).join("s01");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/s01\n", moved.display()),
        )
        .unwrap();
        match super::attribution_of(&root, stale, super::Backfill::Record) {
            super::Attribution::Unknown(why) => {
                assert!(why.contains("moved") || why.contains("api-"), "{why}")
            }
            other => panic!("a moved checkout is not this id's owner: {other:?}"),
        }
        assert_eq!(
            super::recorded_checkout(&root, stale),
            Ok(None),
            "and nothing is written back from evidence omh refused"
        );
    }

    /// A checkout that recorded itself can be found again — and one that never
    /// did is absent, not deleted.
    #[test]
    fn a_checkout_that_recorded_itself_can_be_found_again() {
        let f = fixture(&[]);
        std::fs::create_dir_all(&f.paths.repo).unwrap();
        let id = f.paths.repo_id();

        // Nothing recorded yet: absent, and not an error.
        assert_eq!(
            super::recorded_checkout(&f.paths.root, &id),
            Ok(None),
            "an id nothing recorded is absent"
        );

        f.paths.remember_checkout().expect("recording must work");
        assert_eq!(
            super::recorded_checkout(&f.paths.root, &id),
            Ok(Some(super::settled(&f.paths.repo))),
            "and afterwards it names the checkout it came from"
        );

        // Recording twice is not an error and does not duplicate: every
        // command may do it.
        f.paths.remember_checkout().expect("again");
        assert_eq!(
            super::recorded_checkout(&f.paths.root, &id),
            Ok(Some(super::settled(&f.paths.repo)))
        );

        // Nothing is left beside the record. The write goes via a temp name so
        // a torn write cannot leave a path that resolves somewhere else, and a
        // stray temp file would be a second entry in a directory whose entries
        // are about to mean "a checkout omh knows about".
        let beside: Vec<String> = std::fs::read_dir(f.paths.checkouts())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != &id)
            .collect();
        assert!(beside.is_empty(), "only the record itself: {beside:?}");

        // A record omh cannot make sense of is "could not tell", never "no
        // such checkout" — the reasons differ, and the report prints them.
        std::fs::write(f.paths.checkouts().join(&id), "   \n").unwrap();
        match super::recorded_checkout(&f.paths.root, &id) {
            Err(why) => assert!(why.contains("empty"), "and says so: {why}"),
            other => panic!("an empty record is not the absence of one: {other:?}"),
        }
    }

    /// The three answers a `repo_id` can have, and the one that must never be
    /// guessed.
    ///
    /// `Unknown` is not a tidier spelling of `Gone`. On the first run after
    /// this lands, *no* checkout has a record — so if absence of a record read
    /// as "the checkout was deleted", prune would take every artifact on the
    /// machine. That arm is the reason this function exists as a table rather
    /// than an `if let`.
    #[test]
    fn a_repo_id_with_no_record_is_unknown_and_never_gone() {
        use super::{attribution_from, Attribution};
        let at = |p: &str| std::path::PathBuf::from(p);

        // Recorded and present.
        assert_eq!(
            attribution_from(Ok(Some(at("/work/api"))), Ok(true)),
            Attribution::Live(at("/work/api"))
        );
        // Recorded and provably not there: the only arm a removal may act on.
        assert_eq!(
            attribution_from(Ok(Some(at("/work/api"))), Ok(false)),
            Attribution::Gone(at("/work/api"))
        );

        // No record at all.
        match attribution_from(Ok(None), Ok(false)) {
            Attribution::Unknown(why) => assert!(
                !why.trim().is_empty(),
                "and it says why, because the row prints it"
            ),
            other => panic!("no record is not a deleted checkout: {other:?}"),
        }

        // A record omh could not read.
        match attribution_from(Err("permission denied".into()), Ok(false)) {
            Attribution::Unknown(why) => assert!(why.contains("permission denied"), "{why}"),
            other => panic!("an unreadable record is not a deleted checkout: {other:?}"),
        }

        // Recorded, but omh could not tell whether the path is there — a
        // checkout on a mount that has gone away. `Path::exists` answers
        // `false` here, which is how everything it owns would read as
        // reclaimable.
        match attribution_from(Ok(Some(at("/mnt/api"))), Err("host is down".into())) {
            Attribution::Unknown(why) => {
                assert!(
                    why.contains("/mnt/api") && why.contains("host is down"),
                    "{why}"
                )
            }
            other => panic!("could not tell is not gone: {other:?}"),
        }
    }
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: Paths,
    }

    fn fixture(layers: &[(&str, &str, &str)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        for (layer, name, body) in layers {
            let base = match *layer {
                "catalogue" => paths.root.clone(),
                "project" => paths.repo.join(".omh"),
                // The three that are going away, so a test can say what no
                // longer reaches a session.
                "personal" => paths.root.join("profile"),
                "shared" => paths.repo.join(".omh/profile"),
                "local" => paths.repo.join(".omh/local"),
                other => panic!("unknown layer {other}"),
            };
            let p = base.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        Fixture { _dir: dir, paths }
    }

    /// A catalogue omh cannot read is not a catalogue that declares nothing.
    ///
    /// `Path::exists()` answers `false` for *every* error — a dangling symlink
    /// into an unmounted volume, a parent directory created under `sudo`, a
    /// network share not up yet — so an unreadable catalogue resolved to "you
    /// declared none of this". The launcher then skips the capability, mounts
    /// nothing, adds nothing to `dropped`, and exits 0; `omh doctor` takes the
    /// same empty-sources branch and reports healthy. That is the closed loop
    /// `config::read_layer` was written about, and one catalogue makes it total
    /// rather than partial — before this there were three layers and one bad
    /// path degraded a third of the way.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_catalogue_is_an_error_not_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture(&[("catalogue", "skills/mine/SKILL.md", "s")]);
        // The parent unreadable, so `stat` on the child fails with EACCES
        // rather than ENOENT — a broken symlink or an absent mount reaches
        // `exists()` exactly the same way.
        std::fs::set_permissions(&f.paths.root, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = Profile::resolve(&f.paths).sources(Capability::Skills);
        std::fs::set_permissions(&f.paths.root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.expect_err("unreadable must not read as undeclared");
        assert!(
            format!("{err:#}").contains("skills"),
            "must name the path: {err:#}"
        );
    }

    /// Absent is normal: a fresh catalogue declares most capabilities not at
    /// all, and an empty result has to mean that rather than a path invented on
    /// its behalf — the launcher mounts what this returns.
    #[test]
    fn absent_capabilities_are_skipped_not_faked() {
        let f = fixture(&[("catalogue", "rules/tdd.md", "only")]);
        let profile = Profile::resolve(&f.paths);
        assert_eq!(profile.sources(Capability::Rules).unwrap().len(), 1);
        assert!(profile.sources(Capability::Skills).unwrap().is_empty());
    }

    /// A name omh mints has to be a name `[use]` can hold.
    ///
    /// `entries` is where `init` and `omh use --all` get the names they write,
    /// and `Selection::read_list` refuses one that begins with a dot — so a
    /// `.DS_Store` in `~/.omh/skills`, which Finder creates in any directory
    /// somebody opens, was written into `[use]` and then refused by every
    /// command that read the file afterwards. `omh info --repo`, `omh use`, and the
    /// launch itself, all dead until the file was hand-edited.
    ///
    /// The rule this restores is `validate_entry_name`'s own: checked where a
    /// name is **minted**. This is the fourth mint point and the only one that
    /// was not on the list.
    #[test]
    fn a_name_omh_cannot_write_is_not_an_entry() {
        let f = fixture(&[
            ("catalogue", "skills/review-diff/SKILL.md", "s"),
            ("catalogue", "skills/.DS_Store", "junk"),
        ]);
        assert_eq!(
            Profile::resolve(&f.paths)
                .entries(Capability::Skills)
                .unwrap(),
            vec!["review-diff"],
            "a dotfile is not a skill, and naming it would poison the settings file"
        );
    }

    #[test]
    fn declared_reports_only_present_capabilities() {
        let f = fixture(&[
            ("catalogue", "rules/tdd.md", "r"),
            ("catalogue", "mcp.json", "{}"),
            ("catalogue", "skills/x/SKILL.md", "s"),
        ]);
        let declared = Profile::resolve(&f.paths).declared().unwrap();
        assert_eq!(
            declared,
            vec![Capability::Rules, Capability::Skills, Capability::Mcp]
        );
    }

    /// Content lives in one place.
    ///
    /// Three layers with identical shapes meant "where is this skill" had three
    /// answers, and `sources` was a union — a later layer could shadow a
    /// same-named entry but nothing could turn one off, so "these are my twelve
    /// MCP servers, this project uses three" was unsayable.
    #[test]
    fn a_capability_resolves_to_one_catalogue_path() {
        let f = fixture(&[("catalogue", "skills/mine/SKILL.md", "yours")]);
        assert_eq!(
            Profile::resolve(&f.paths)
                .sources(Capability::Skills)
                .unwrap(),
            vec![f.paths.root.join("skills")]
        );
    }

    /// A project names entries from your catalogue; it cannot declare one.
    ///
    /// The committed layer is what made a repo able to hand you a skill, an MCP
    /// server or a command — content that arrives by `git clone` and runs
    /// against your work. What a repo still shares is its rules file, its hooks,
    /// its selection and its policy.
    #[test]
    fn a_repo_cannot_declare_content_of_its_own() {
        let f = fixture(&[
            ("shared", "skills/theirs/SKILL.md", "the repo's"),
            ("local", "skills/secret/SKILL.md", "yours, here"),
            ("shared", "mcp.json", "{}"),
        ]);
        let profile = Profile::resolve(&f.paths);
        assert!(profile.sources(Capability::Skills).unwrap().is_empty());
        assert!(profile.sources(Capability::Mcp).unwrap().is_empty());
    }

    /// Hooks are the one capability with a repo tier, because they are the one
    /// whose scope is genuinely the repo: `cargo test` here, `pnpm test` next
    /// door, one name and two bodies.
    ///
    /// The repo's come last, so a project overrides your personal `format` hook
    /// with the one this project actually needs, without renaming anything.
    #[test]
    fn hooks_resolve_to_the_catalogue_then_the_repo() {
        let f = fixture(&[
            ("catalogue", "hooks/format.json", "yours"),
            ("project", "hooks/format.json", "this repo's"),
        ]);
        assert_eq!(
            Profile::resolve(&f.paths)
                .sources(Capability::Hooks)
                .unwrap(),
            vec![f.paths.root.join("hooks"), f.paths.repo.join(".omh/hooks")],
            "project last, so project wins"
        );
    }

    /// Worktrees live outside the repo so an IDE opened on the repo root does not
    /// index every session's full copy of the codebase.
    #[test]
    fn worktrees_live_outside_the_repo() {
        let f = fixture(&[]);
        assert!(!f.paths.worktrees().starts_with(&f.paths.repo));
        assert!(f.paths.worktrees().starts_with(&f.paths.root));
    }

    /// Two checkouts with the same directory name are two repos.
    ///
    /// Risk 8d. `repo_id` was the checkout's basename, and **every** piece of
    /// per-repo state hangs off it — worktrees, run directories, ssh keys,
    /// sandbox repositories, the note store, the cache volume, the network and
    /// the container name. So `~/work/api` and `~/oss/api` were one repo as
    /// far as omh was concerned, and the second one's `omh new` resumed into
    /// the first one's session: a live container holding another project's
    /// code, reached by typing an ordinary command in an ordinary checkout.
    ///
    /// Asserted over every accessor by name rather than over a chosen few.
    /// The failure mode is that somebody adds a tenth piece of per-repo state
    /// and keys it the old way, and a test naming three of nine would not see
    /// it — this at least fails the moment an existing one regresses.
    #[test]
    fn two_checkouts_with_the_same_name_are_not_one_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        std::fs::create_dir_all(dir.path().join("work/api")).unwrap();
        std::fs::create_dir_all(dir.path().join("oss/api")).unwrap();

        let work = Paths {
            root: root.clone(),
            repo: dir.path().join("work/api"),
        };
        let oss = Paths {
            root,
            repo: dir.path().join("oss/api"),
        };

        // Named, because clippy reads the inline form as a complex type and
        // it is genuinely easier to read this way: nine ways of asking one
        // `Paths` what it calls something.
        type Accessor = (&'static str, fn(&Paths) -> String);
        let both_ways: [Accessor; 9] = [
            ("container", |p| p.container("s01")),
            ("cache_volume", |p| p.cache_volume()),
            ("network", |p| p.session_network("s01")),
            ("worktrees", |p| p.worktrees().display().to_string()),
            ("runs", |p| p.runs().display().to_string()),
            ("keys", |p| p.keys().display().to_string()),
            ("shadows", |p| p.shadows().display().to_string()),
            ("notes", |p| p.notes().display().to_string()),
            ("scratch", |p| p.scratch("login").display().to_string()),
        ];
        for (name, of) in both_ways {
            assert_ne!(
                of(&work),
                of(&oss),
                "{name}: two checkouts named `api` must not share it"
            );
        }
    }

    /// The digest reads the **whole** path, not the tail of it.
    ///
    /// Every other fixture in this file distinguishes its two checkouts by
    /// the *parent directory's name* — `work/api` against `oss/api` — so a
    /// digest that hashed only that name satisfied all of them. Measured: the
    /// entire `profile` suite, 32 tests, green against an implementation that
    /// re-collides `~/a/work/api` with `~/b/work/api`. Risk 8d, reopened,
    /// with nothing to say so.
    ///
    /// These two differ only *above* the parent, which is the one shape that
    /// forces the digest to read past the tail.
    #[test]
    fn two_checkouts_differing_only_above_the_parent_are_not_one_repo() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one/work/api");
        let two = dir.path().join("two/work/api");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();

        let of = |repo: std::path::PathBuf| Paths {
            root: dir.path().join("home"),
            repo,
        };
        assert_ne!(
            of(one).container("s01"),
            of(two).container("s01"),
            "same basename and same parent name is still two checkouts"
        );
    }

    /// The same checkout is the same repo, every time.
    ///
    /// The other half, and the more dangerous one to get wrong: an id that
    /// varied between two constructions — or between two runs — would strand
    /// every session the previous id created, which is worse than the
    /// collision it replaced.
    ///
    /// **The directory is created halfway through on purpose.** Without that
    /// this test passes against a `repo_id` built on a bare `canonicalize()`,
    /// which fails for a path that does not exist and so returns a different
    /// answer before and after `mkdir`. That is not hypothetical: it is what
    /// the first version of this did, and seven memory tests found it because
    /// seeding a team note creates `<repo>/.omh/notes` — and therefore
    /// `<repo>` — between two writes to a store keyed by this id. Asserting an
    /// id twice over a directory that exists both times is the easy half of
    /// the property and misses the whole defect.
    #[test]
    fn the_same_checkout_answers_to_the_same_id_before_and_after_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let of = || Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("work/api"),
        };

        let before = of().container("s01");
        let before_worktrees = of().worktrees();
        assert_eq!(before, of().container("s01"), "stable while absent");

        std::fs::create_dir_all(dir.path().join("work/api")).unwrap();

        assert_eq!(
            before,
            of().container("s01"),
            "a checkout that comes into existence is the same checkout"
        );
        assert_eq!(before_worktrees, of().worktrees());
    }

    /// A symlinked checkout is not a second repo.
    ///
    /// The reason the id resolves the path at all rather than hashing it as
    /// typed. Someone reaching the same checkout through a symlink — a
    /// `~/code` that points elsewhere, a `/tmp` that is really `/private/tmp`
    /// — must land in the session they already have, not open a parallel one
    /// over the same files.
    #[test]
    #[cfg(unix)]
    fn a_checkout_reached_through_a_symlink_is_the_same_repo() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real/api");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let direct = Paths {
            root: dir.path().join("home"),
            repo: real,
        };
        let through = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("link/api"),
        };
        assert_eq!(
            direct.container("s01"),
            through.container("s01"),
            "one checkout, however it was reached"
        );
    }

    /// A worktree left by an older omh, at the old key.
    fn legacy_session(root: &Path, old: &str, id: &str, owner: &Path) {
        let wt = root.join("worktrees").join(old).join(id);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/{id}\n", owner.display()),
        )
        .unwrap();
    }

    const NOT_RUNNING: &dyn Fn(&str) -> bool = &|_: &str| false;
    const RUNNING: &dyn Fn(&str) -> bool = &|_: &str| true;

    /// An upgrade finds the state the old key left behind.
    ///
    /// Without this, everything an existing install has — sessions, notes,
    /// sandbox repositories holding commits an agent made — is still on disk
    /// and no longer anywhere omh looks. Not lost, but invisible, which for
    /// unharvested work is the same afternoon.
    #[test]
    fn state_under_the_old_key_moves_to_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };

        legacy_session(&paths.root, "api", "s01", &repo);
        std::fs::create_dir_all(paths.root.join("notes/api/local")).unwrap();
        std::fs::write(paths.root.join("notes/api/local/a.md"), "a note").unwrap();
        std::fs::create_dir_all(paths.root.join("shadow/api")).unwrap();

        let moved = migrate(&paths, NOT_RUNNING).unwrap();
        assert!(
            matches!(&moved, Migration::Moved { kinds, .. }
                if kinds.contains(&"worktrees".to_string())
                    && kinds.contains(&"notes".to_string())
                    && kinds.contains(&"shadow".to_string())),
            "got: {moved:?}"
        );

        assert!(
            paths.worktrees().join("s01").is_dir(),
            "the session arrived"
        );
        assert_eq!(
            std::fs::read_to_string(paths.notes().join("local/a.md")).unwrap(),
            "a note",
            "and so did the notes, contents intact"
        );
        assert!(
            !paths.root.join("worktrees/api").exists(),
            "and the old key is gone, so this does not run again"
        );
    }

    /// A directory holding sessions from **both** checkouts is refused.
    ///
    /// The realistic shape of risk 8d, and the one the first version of this
    /// migration got wrong. Pre-2026.08 `next_id` scanned the shared
    /// directory, so two checkouts named `api` did not each get an `s01` —
    /// the first took `s01` and the second took `s02`, **in one directory**.
    /// A migration that reads one pointer and stops is therefore sampling,
    /// and `read_dir` order decides which checkout wins.
    ///
    /// Measured against the release binary before this was fixed: `oss/api`
    /// migrated the shared directory onto its own key and took `work/api`'s
    /// `s01` with it. `work/api` then reported `no sessions`, and `oss/api`
    /// listed `s01` with `?  (how far behind main?)` — because that branch
    /// lives in the other checkout's repository. Risk 8d performed
    /// deliberately, by the code written to end it, with the owner's
    /// unharvested commits inside.
    ///
    /// The guard that missed it planted a **single** legacy session, which is
    /// why it passed. `AGENTS.md`: the original defect is the best mutation
    /// you will ever get.
    #[test]
    fn a_directory_holding_two_checkouts_sessions_is_refused_to_both() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("oss/api");
        let theirs = dir.path().join("work/api");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();

        let home = dir.path().join("home");
        legacy_session(&home, "api", "s01", &theirs);
        legacy_session(&home, "api", "s02", &mine);

        // Asked from both sides, because whichever one `read_dir` happens to
        // sample first must not be the one that decides.
        for (name, repo) in [("oss", &mine), ("work", &theirs)] {
            let paths = Paths {
                root: home.clone(),
                repo: repo.clone(),
            };
            let out = migrate(&paths, NOT_RUNNING).unwrap();
            assert!(
                matches!(out, Migration::Refused(_)),
                "{name}: a directory holding both checkouts' sessions must go to \
                 neither, got: {out:?}"
            );
        }
        assert!(
            home.join("worktrees/api/s01").is_dir() && home.join("worktrees/api/s02").is_dir(),
            "and both sessions stay where they are"
        );
    }

    /// A relative `gitdir:` pointer resolves against the worktree, not the cwd.
    ///
    /// git ≥ 2.48 writes relative pointers under `worktree.useRelativePaths`
    /// and `git worktree add --relative-paths` — `gitdir: ../../../.git/…`.
    /// `owner_of` walked those to `.git` and handed the parent to `settled`,
    /// which canonicalises a relative path against **the omh process's
    /// working directory**. So ownership was computed from wherever the user
    /// happened to run `omh`, and the answer changed with the shell.
    ///
    /// It failed closed by accident rather than by design — a cwd-derived
    /// owner does not match this checkout, so it refused — and the whole
    /// argument for this function is that ownership is *read* rather than
    /// inferred. A guard that is right by coincidence is the shape this
    /// release exists to remove.
    #[test]
    fn a_relative_gitdir_pointer_names_the_checkout_it_actually_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(repo.join(".git/worktrees/s01")).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };

        // Exactly what git writes with relative paths on: the pointer is
        // relative to the directory holding it.
        let wt = paths.root.join("worktrees/api/s01");
        std::fs::create_dir_all(&wt).unwrap();
        let up = "../".repeat(wt.components().count() - dir.path().components().count());
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {up}work/api/.git/worktrees/s01\n"),
        )
        .unwrap();

        assert!(
            matches!(
                migrate(&paths, NOT_RUNNING).unwrap(),
                Migration::Moved { .. }
            ),
            "the pointer names this checkout, however it spells the path"
        );
    }

    /// An entry omh cannot even stat is evidence, not silence.
    ///
    /// The first fix for the mixed-directory bug introduced this one. It
    /// filtered entries with `if !entry.is_dir() { continue }`, and
    /// `Path::is_dir()` returns `false` for **every** error — a dangling
    /// symlink, EACCES, a racing removal. So the entry contributed to neither
    /// `owners` nor `unreadable` and vanished before the accounting.
    ///
    /// Measured against the release binary: with `s01` a symlink whose target
    /// had gone and `s02` genuinely owned by the other checkout, `omh s`
    /// reported `moved this checkout's worktrees off `api`` and took the
    /// directory. Risk 8d, performed, by the commit that fixed risk 8d being
    /// performed. The lesson is the one this whole release is about, and it
    /// arrived through a one-line filter added while closing it.
    #[test]
    fn an_entry_omh_cannot_stat_stops_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s02", &repo);
        // A worktree that was moved or deleted out from under omh.
        std::os::unix::fs::symlink(
            dir.path().join("gone"),
            paths.root.join("worktrees/api/s01"),
        )
        .unwrap();

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        assert!(
            matches!(out, Migration::Refused(_)),
            "an entry omh cannot look at is not an entry it may ignore: {out:?}"
        );
    }

    /// A directory that is not a worktree does not block the move.
    ///
    /// `Unclaimed`'s doc says it covers "an empty directory, or one a hand
    /// `git worktree remove` already emptied" — and the first version
    /// refused those, because a directory with no `.git` fell into the same
    /// bucket as one whose `.git` could not be read. An editor's `.idea`, a
    /// prune leftover, or an emptied worktree would strand a repo's notes
    /// for ever with a message about two checkouts that do not exist.
    ///
    /// **Absent is not unreadable.** That distinction is the whole discipline
    /// of this release, applied here: nothing claiming ownership is evidence
    /// of nothing; a claim omh cannot read is evidence it must not decide.
    #[test]
    fn a_directory_that_is_not_a_worktree_does_not_block_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);
        std::fs::create_dir_all(paths.root.join("worktrees/api/.idea")).unwrap();
        std::fs::create_dir_all(paths.root.join("worktrees/api/s09")).unwrap();

        assert!(
            matches!(
                migrate(&paths, NOT_RUNNING).unwrap(),
                Migration::Moved { .. }
            ),
            "one owner and some clutter is still one owner"
        );
    }

    /// A pointer omh cannot read says so, rather than blaming the file.
    ///
    /// `read_to_string(..).ok()` threw the `io::Error` away and every failure
    /// landed on "does not say which checkout it belongs to" — a claim about
    /// the file's *contents*, printed when omh had been denied permission to
    /// look at them. The two neighbouring failure sites both append `({e})`;
    /// only this one, the one that fires most often, did not. The user was
    /// sent to inspect a file when the fix was `chmod`.
    #[test]
    #[cfg(unix)]
    fn a_pointer_omh_may_not_read_says_that_rather_than_blaming_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);
        let pointer = paths.root.join("worktrees/api/s01/.git");
        std::fs::set_permissions(&pointer, std::fs::Permissions::from_mode(0o000)).unwrap();

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        std::fs::set_permissions(&pointer, std::fs::Permissions::from_mode(0o644)).unwrap();

        let Migration::Refused(why) = &out else {
            panic!("must refuse, got: {out:?}");
        };
        assert!(
            why.contains("could not be read"),
            "the message has to name the read failure, not the file's contents: {why}"
        );
    }

    /// A pointer omh cannot read is a refusal, not an adoption.
    ///
    /// `owning_checkout` collapsed every read failure onto "nobody owns
    /// this", and `migrate` reads that as permission to proceed. So an
    /// unreadable directory belonging to *another* checkout named `api` was
    /// adopted — worktrees, notes, keys, shadow and scratch renamed under this
    /// checkout's id. "Cannot look" spelled exactly like "nobody is there", in
    /// the one place where being wrong moves somebody else's commits.
    #[test]
    fn a_session_pointer_omh_cannot_read_stops_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        // A worktree whose `.git` is a directory rather than a pointer file —
        // a hand-made clone parked there. `read_to_string` fails on it.
        let wt = paths.root.join("worktrees/api/s01");
        std::fs::create_dir_all(wt.join(".git")).unwrap();

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        assert!(
            matches!(out, Migration::Refused(_)),
            "omh cannot tell whose this is and must not decide: {out:?}"
        );
        assert!(wt.is_dir(), "and it stays put");
    }

    /// What could not move is reported even when something else did.
    ///
    /// `blocked` was computed and then only ever read inside the
    /// `pending.is_empty()` branch, so a mixed run printed a cheerful "moved
    /// this checkout's notes" and said nothing about the worktrees that had
    /// just become unreachable — the exact silence `Stranded`'s own doc
    /// comment says it exists to break.
    #[test]
    fn what_could_not_move_is_reported_alongside_what_did() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);
        std::fs::create_dir_all(paths.root.join("notes/api/local")).unwrap();
        // The new key already holds worktrees, so that kind cannot move…
        std::fs::create_dir_all(paths.worktrees().join("s02")).unwrap();

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        let Migration::Moved {
            kinds, stranded, ..
        } = &out
        else {
            panic!("notes can still move, so this is a Moved: {out:?}");
        };
        assert_eq!(kinds, &vec!["notes".to_string()], "…and notes did");
        assert_eq!(
            stranded,
            &vec!["worktrees".to_string()],
            "and the sessions nothing will read again are named in the same breath"
        );
    }

    /// Sessions belonging to another checkout are left where they are.
    ///
    /// The whole of risk 8d in one test. Two checkouts named `api` shared one
    /// directory, and exactly one of them owns it — a migration that adopted
    /// on proximity would perform the collision it exists to end, handing this
    /// checkout the other one's sessions. The worktree pointer says whose they
    /// are, so omh does not have to guess.
    #[test]
    fn sessions_belonging_to_another_checkout_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("oss/api");
        let theirs = dir.path().join("work/api");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: mine,
        };

        legacy_session(&paths.root, "api", "s01", &theirs);

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        let Migration::Refused(why) = &out else {
            panic!("must refuse, got: {out:?}");
        };
        assert!(
            why.contains(&theirs.display().to_string()),
            "and name whose they are: {why}"
        );
        assert!(
            paths.root.join("worktrees/api/s01").is_dir(),
            "and leave them for that checkout to claim"
        );
    }

    /// A live sandbox stops the move rather than having it done underneath.
    ///
    /// A running container's mounts point at the worktree directory being
    /// renamed. Docker goes on serving the old inode while omh reports
    /// success, which leaves a session running on a path neither of them can
    /// name — a state with no error message and no way back except finding
    /// the container by hand.
    #[test]
    fn a_running_sandbox_refuses_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);

        let out = migrate(&paths, RUNNING).unwrap();
        let Migration::Refused(why) = &out else {
            panic!("must refuse, got: {out:?}");
        };
        assert!(why.contains("omh s down"), "and say the way out: {why}");
        assert!(
            paths.root.join("worktrees/api/s01").is_dir(),
            "and move nothing"
        );
    }

    /// A repo that only ever ran `init` keeps its notes.
    ///
    /// No worktrees means no session to collide over, and the pointer that
    /// decides ownership everywhere else does not exist. Refusing here would
    /// strand the notes of every user who set a repo up and had not yet
    /// started a session — the common case, penalised for a collision that
    /// cannot occur.
    #[test]
    fn state_with_no_sessions_still_moves() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo,
        };
        std::fs::create_dir_all(paths.root.join("notes/api/local")).unwrap();

        assert!(matches!(
            migrate(&paths, NOT_RUNNING).unwrap(),
            Migration::Moved { .. }
        ));
        assert!(paths.notes().join("local").is_dir());
    }

    /// Running it twice is not running it twice.
    ///
    /// It fires on every command, so the second call has to be free and
    /// harmless — and must never merge a fresh directory into an old one it
    /// half-recognises.
    #[test]
    fn a_second_migration_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);

        assert!(matches!(
            migrate(&paths, NOT_RUNNING).unwrap(),
            Migration::Moved { .. }
        ));
        assert_eq!(
            migrate(&paths, NOT_RUNNING).unwrap(),
            Migration::NothingToDo,
            "the second run has nothing left to find"
        );
    }

    /// A new key already in use is never merged into.
    ///
    /// If both keys hold a directory, something already migrated or the user
    /// has been running two versions. Renaming onto it would either fail or —
    /// worse, on some platforms — merge two repos' state silently.
    #[test]
    fn an_existing_new_key_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("work/api");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        legacy_session(&paths.root, "api", "s01", &repo);
        std::fs::create_dir_all(paths.worktrees().join("s02")).unwrap();

        let out = migrate(&paths, NOT_RUNNING).unwrap();
        assert_eq!(
            out,
            Migration::Stranded {
                from: "api".into(),
                kinds: vec!["worktrees".into()],
            },
            "not merged — and not passed over in silence either"
        );
        assert!(paths.worktrees().join("s02").is_dir(), "the new one stands");
        assert!(
            paths.root.join("worktrees/api/s01").is_dir(),
            "and the old one is still there to be dealt with"
        );
    }

    /// The digest is pinned to published vectors, not to itself.
    ///
    /// A test asserting `stable_digest(x) == stable_digest(x)` would pass
    /// against any implementation, including a rewritten one — and a rewrite
    /// is precisely the event this has to survive, because the value names
    /// directories holding an agent's commits. If it moves, those sessions do
    /// not break loudly; they become unreachable while omh opens a fresh empty
    /// one where the user's work used to be.
    ///
    /// So the numbers below are FNV-1a/32's own published test vectors rather
    /// than output read off a run of this code. Reading them off a run would
    /// pin whatever this function does today, bug included, which is the
    /// mistake `image::recipe_digest`'s doc warns about in the other
    /// direction.
    #[test]
    fn the_digest_matches_the_published_fnv_vectors() {
        // From the FNV reference test vectors for the 32-bit 1a variant.
        assert_eq!(stable_digest(""), 0x811c_9dc5, "the offset basis");
        assert_eq!(stable_digest("a"), 0xe40c_292c);
        assert_eq!(stable_digest("foobar"), 0xbf9c_f968);
    }

    /// A repo id still reads as the directory it belongs to.
    ///
    /// The point of not simply hashing the whole path: `omh s` prints these,
    /// `docker ps` lists them, and a user has to be able to tell which of
    /// their checkouts a container belongs to at a glance. The digest
    /// disambiguates; the name is what makes the result legible.
    #[test]
    fn a_repo_id_still_names_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("work/api")).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("work/api"),
        };
        let container = paths.container("s01");
        assert!(
            container.starts_with("omh-api-"),
            "a person has to recognise this: {container}"
        );
    }

    /// Keyed by repo, not harness — this is what lets memory survive a switch.
    ///
    /// Asserted as the property rather than as the string. It read
    /// `assert_eq!(cache_volume(), "omh-cache-repo")`, which names no harness
    /// and so could not have failed if the volume had become harness-specific
    /// — the sentence above was carried entirely by the literal happening not
    /// to contain one. It also broke on the repo id gaining a digest, which is
    /// the tell: a guard that a keying change breaks, while the thing it
    /// claims to protect is untouched, was asserting shape and not invariant.
    ///
    /// What actually holds it up is that `cache_volume` takes no harness
    /// argument, and no test can say that. What a test can say is that the
    /// volume follows the repo: same checkout, same volume across any number
    /// of resolutions; different checkout, different volume.
    #[test]
    fn cache_volume_is_harness_independent() {
        let f = fixture(&[]);
        assert_eq!(
            f.paths.cache_volume(),
            f.paths.cache_volume(),
            "one checkout keeps one cache, however often it is asked"
        );
        assert!(
            f.paths.cache_volume().starts_with("omh-cache-repo"),
            "and it names the repo: {}",
            f.paths.cache_volume()
        );

        let other = Paths {
            root: f.paths.root.clone(),
            repo: f.paths.repo.parent().unwrap().join("elsewhere/repo"),
        };
        assert_ne!(
            f.paths.cache_volume(),
            other.cache_volume(),
            "and a different checkout of the same name does not share it"
        );
    }

    #[test]
    fn missing_git_repo_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = repo_root(dir.path()).unwrap_err();
        assert!(err.to_string().contains("git init"), "got: {err}");
    }

    /// Regression: staging was keyed by session and harness only, so two repos
    /// both using session `s01` shared one rendered profile — repo A's MCP
    /// config could be mounted into repo B's sandbox.
    #[test]
    fn staging_is_keyed_by_repo() {
        let dir = tempfile::tempdir().unwrap();
        let a = Paths {
            root: dir.path().into(),
            repo: dir.path().join("alpha"),
        };
        let b = Paths {
            root: dir.path().into(),
            repo: dir.path().join("beta"),
        };
        assert_ne!(a.staging("s01", "claude"), b.staging("s01", "claude"));
    }

    /// The hazard `shadows()` was moved out of `worktrees()` to avoid:
    /// `session::list` reports every directory under `worktrees()` as a
    /// session, so a gitdir living there shows up in `omh s` as one you
    /// could resume.
    #[test]
    fn a_sandbox_repository_never_lives_where_sessions_are_counted() {
        let f = fixture(&[]);
        let p = &f.paths;
        assert!(
            !p.shadows().starts_with(p.worktrees()),
            "shadows must not sit under worktrees: {} inside {}",
            p.shadows().display(),
            p.worktrees().display()
        );
    }

    /// The same reason staging is keyed by repo, with a sharper edge: two
    /// checkouts each running `s01` would share one gitdir, so the agent in
    /// repo B would open on repo A's scratch history — the isolation this
    /// whole module exists for, lost to a path collision.
    #[test]
    fn sandbox_repositories_are_keyed_by_repo() {
        let dir = tempfile::tempdir().unwrap();
        let a = Paths {
            root: dir.path().into(),
            repo: dir.path().join("alpha"),
        };
        let b = Paths {
            root: dir.path().into(),
            repo: dir.path().join("beta"),
        };
        assert_ne!(a.shadows(), b.shadows());
    }

    #[test]
    fn staging_still_separates_sessions_and_harnesses() {
        let f = fixture(&[]);
        let p = &f.paths;
        assert_ne!(p.staging("s01", "claude"), p.staging("s02", "claude"));
        assert_ne!(p.staging("s01", "claude"), p.staging("s01", "opencode"));
    }
}
