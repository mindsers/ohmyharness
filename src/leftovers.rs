//! What omh finds lying around, and where it could not look.
//!
//! Session state nothing points at any more — a sandbox repository, a run
//! directory, a container — swept out of `~/.omh` and the runtime for `omh s`
//! and `omh doctor`. `.claude/skills/could-not-look` is why every read here
//! that fails is said and recorded, never read as "none".

use crate::paths::Paths;
use crate::{idle, out, runtime};

/// What omh found lying around, and where it could not look.
///
/// Two `Vec<String>` returned side by side are transposable in silence: a
/// swapped destructure here, or swapped arguments at `leftovers_from`, compiles
/// and reads as a clean machine reporting reasons as orphans. Naming the halves
/// closes that, and moves the *found* versus *could not look* distinction out of
/// a doc comment and into the type.
#[derive(Default)]
pub(crate) struct Leftovers {
    /// Sessions nothing points at: sandbox repositories, run directories and
    /// containers, minus the ones a live worktree still claims.
    pub(crate) found: Vec<String>,
    /// One line per read omh could not make, in the words the warning used —
    /// plus the consequence, when a failed predicate costs the whole list.
    ///
    /// Empty is "omh looked everywhere", which is the only thing that makes
    /// `found` being empty mean *nothing is orphaned* — and it was a claim
    /// rather than a fact while two reads could come back empty-handed without
    /// recording anything. The container half was skipped entirely when there
    /// was no runtime to ask, and `session::list` answered `Vec::new()` for a
    /// `worktrees/` it could not open. Five reads report here now: the three
    /// sources, the predicate, and the runtime omh never chose.
    pub(crate) unchecked: Vec<String>,
}

/// Every read that failed, said once and recorded once.
///
/// Seven copies of `ctx.warn(&why); unchecked.push(why);` stood here, and a
/// warn whose push went missing *is* the defect this function was rewritten
/// for: the reason reaches `omh s`'s stderr and never reaches `omh doctor`'s
/// row, where a `--json` reader has no stderr to read it from. There is now no
/// syntax for the one without the other.
///
/// A type rather than a local closure because the sweeps below take it by
/// `&mut` while `found` is being built, which a closure capturing the `Vec`
/// cannot allow — and because a closure cannot carry `ctx` into them.
///
/// **Not [`out::Asides`].** Asides reach `Ctx::say`, which drops warnings under
/// `--json` — the exact stream `doctor` reports on, and the exact collapse this
/// exists to stop.
pub(crate) struct Unchecked<'a> {
    ctx: &'a out::Ctx,
    why: Vec<String>,
}

impl<'a> Unchecked<'a> {
    pub(crate) fn new(ctx: &'a out::Ctx) -> Self {
        Self {
            ctx,
            why: Vec::new(),
        }
    }

    /// Say it, and record it. Never one without the other.
    pub(crate) fn note(&mut self, why: String) {
        self.ctx.warn(&why);
        self.why.push(why);
    }

    /// Record it without saying it again.
    ///
    /// One caller, deliberately: both commands that reach this function already
    /// print "there is no container runtime" in their own words — `omh s` as
    /// `omh cannot say which sandboxes are up`, `omh doctor` in its own
    /// `container runtime` row. What neither carries is the fact *in the
    /// report*, which is the half this row exists to fix.
    pub(crate) fn record(&mut self, why: String) {
        self.why.push(why);
    }

    pub(crate) fn recorded(&self) -> usize {
        self.why.len()
    }

    pub(crate) fn reasons(self) -> Vec<String> {
        self.why
    }
}

/// One entry in a directory listing, as much of it as a sweep reads.
///
/// `std::fs::DirEntry` has no public constructor, so while the sweeps read one
/// directly no test could hand them an entry that fails: every branch below was
/// reachable only through a fixture that made `read_dir` *itself* fail, and
/// reverting any of them to `.flatten()` left the whole suite green. This is
/// the smallest surface that makes them driveable.
pub(crate) trait Entry {
    fn name(&self) -> String;
    /// Whether this is a directory — a read of its own, which can fail.
    ///
    /// **Follows symlinks, deliberately.** `session::list` asks
    /// `e.path().is_dir()`, and `idle::recorded_use` stats through a link too.
    /// A sweep that disagreed with `session::list` about what a directory is
    /// would report a session `omh s` lists as an orphan — with `omh <id> rm`
    /// beside it, which takes the sandbox repository as well.
    fn is_dir(&self) -> std::io::Result<bool>;
}

impl Entry for std::fs::DirEntry {
    fn name(&self) -> String {
        self.file_name().to_string_lossy().into_owned()
    }

    fn is_dir(&self) -> std::io::Result<bool> {
        // `std::fs::metadata`, not `DirEntry::metadata`: the second does not
        // follow the link, and neither does `file_type()`.
        Ok(std::fs::metadata(self.path())?.is_dir())
    }
}

/// The entries omh read, and one line per entry it could not.
///
/// `read_dir` fails in two places — opening the directory, and reading an entry
/// out of the open stream. The second is what `.flatten()` drops, and no
/// fixture on the platforms omh is tested on can produce one.
///
/// **The errors, not a count.** Counting them is why `unreadable_reason` used
/// to be the one line in this file that could not end in `: {e}` like every
/// other reason omh gives.
pub(crate) fn listed<E, X: std::fmt::Display>(
    entries: impl IntoIterator<Item = Result<E, X>>,
) -> (Vec<E>, Vec<String>) {
    let mut read = Vec::new();
    let mut failed = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => read.push(e),
            Err(e) => failed.push(e.to_string()),
        }
    }
    (read, failed)
}

/// The reason line for entries omh could not read under `dir`, or `None` when
/// it read them all.
pub(crate) fn unreadable_reason(
    failed: &[String],
    what: &str,
    dir: &std::path::Path,
) -> Option<String> {
    (!failed.is_empty()).then(|| {
        let mut seen: Vec<&str> = failed.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        format!(
            "omh could not read {} entr{} under {}, so those {what} went unchecked: {}",
            failed.len(),
            if failed.len() == 1 { "y" } else { "ies" },
            dir.display(),
            seen.join("; ")
        )
    })
}

/// What a directory read yielded.
///
/// **`Absent` is its own answer.** It is the ordinary "nothing has ever been
/// made here"; `Refused` is omh being unable to look, and is already recorded
/// by the time it is returned. Collapsing the two into an empty listing is the
/// whole of what this row exists to stop.
enum Opened {
    Listing(std::fs::ReadDir),
    Absent,
    Refused,
}

/// `refused` completes "omh could not read <dir>, so …" — the consequence,
/// which differs per caller in a way the plural noun does not.
fn opened(dir: &std::path::Path, refused: &str, unchecked: &mut Unchecked) -> Opened {
    match std::fs::read_dir(dir) {
        Ok(entries) => Opened::Listing(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Opened::Absent,
        Err(e) => {
            unchecked.note(format!(
                "omh could not read {}, so {refused}: {e}",
                dir.display()
            ));
            Opened::Refused
        }
    }
}

/// One directory of session state, swept for ids nothing points at.
///
/// Generic over the entry so a test can drive this exact body — see [`Entry`].
/// `id_of` maps a filename to an id (`s01.git` → `s01`, or identity); `counts`
/// is the per-id read that decides whether the id belongs in the found list, and
/// phrases its own failure. Named for its answer: it was `keep`, which reads as
/// the opposite of what `Ok(true)` means here.
pub(crate) fn sweep<E: Entry, X: std::fmt::Display>(
    entries: impl IntoIterator<Item = Result<E, X>>,
    dir: &std::path::Path,
    what: &str,
    id_of: impl Fn(&str) -> Option<String>,
    counts: impl Fn(&str) -> Result<bool, String>,
    live: &[String],
    unchecked: &mut Unchecked,
) -> Vec<String> {
    let (entries, failed) = listed(entries);
    // Before the loop, not after. This reason is about the directory and the
    // ones below are about things inside it, and in the runs read it used to be
    // pushed last — so the row read that half of its reasons inside-out.
    if let Some(why) = unreadable_reason(&failed, what, dir) {
        unchecked.note(why);
    }
    let mut found = Vec::new();
    for e in entries {
        let name = e.name();
        // **A session's state is a directory.** `notice::record_path` writes
        // `hooks.json` directly under `runs()`, and every entry was taken for an
        // id: `recorded_use` stat'd `runs/hooks.json/last-used`, which for a
        // regular file is `NotADirectory` rather than `NotFound`, so `keep`
        // reported a run omh could not read — on every `omh s`, about a snapshot
        // omh had written itself.
        //
        // Filtered here rather than widened in `recorded_use`: a regular file is
        // not a run omh failed to read, it is not a run at all.
        // `id_of` first, so a name that was never a candidate is dropped without
        // being stat'd: `shadow/` holds `s01.seed` beside `s01.git`, and a seed
        // omh could not stat would otherwise become a reason about a file that
        // is not a sandbox repository and never was.
        let Some(id) = id_of(&name) else { continue };
        // **A live worktree settles it, before any further read.** `is_dir` is a
        // further read, and it used to run first: a `shadow/` that could be
        // listed but not searched made omh say it could not tell whether `s01`
        // was a sandbox repository — about a session with a worktree, on every
        // `omh s`. A reason for a read whose answer could not have changed
        // anything is the shape this branch opened by removing.
        //
        // `found.retain` below filters the found list alone and never reaches
        // the reasons, which is why the skip has to be here.
        if live.contains(&id) {
            continue;
        }
        match e.is_dir() {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                unchecked.note(format!(
                    "omh could not tell whether {} is one of this repo's {what}, so it \
                     went unchecked: {err}",
                    dir.join(&name).display()
                ));
                continue;
            }
        }
        match counts(&id) {
            Ok(true) => found.push(id),
            Ok(false) => {}
            Err(why) => unchecked.note(why),
        }
    }
    found
}

/// Session ids with a container, a run directory or a sandbox repository but
/// no worktree.
///
/// Invisible until now, and not merely untidy: an orphan container holds a
/// session id, and the next session to take that id used to exec straight into
/// it. That was the mount-namespace failure. `s rm` cleans up after itself now,
/// so this reports what older versions left — and anything a hand
/// `git worktree remove` strands from here on.
///
/// A run directory counts only when it carries the marker `idle::touch` writes.
/// `omh doctor` and `omh account login` stage into the same tree under their own names,
/// and neither is a session anybody could resume or would want reported.
pub(crate) fn leftovers(
    paths: &Paths,
    backend: Result<&runtime::Backend, String>,
    mut unchecked: Unchecked<'_>,
) -> Leftovers {
    let itself = |n: &str| Some(n.to_string());
    let always = |_: &str| Ok(true);

    // **The predicate, not a fourth source.** `session::list` answers
    // `Vec::new()` both for "no session has ever been made here" and for "omh
    // could not look", and here those are opposite answers: an unreadable
    // `worktrees/` left `live` empty, so every run, sandbox repository and
    // container below was unclaimed. omh named live sessions as orphans with
    // `omh <id> rm` beside each — the command that takes the worktree *and* the
    // sandbox repository holding every commit the agent made — while `unchecked`
    // stayed empty to say it had looked everywhere.
    //
    // Read here rather than through `session::list` because this one needs the
    // *consequence* named — "it cannot tell which sessions are live" is this
    // caller's sentence, not a fact about the directory.
    //
    // This used to say `session::list` was left alone because "its other
    // callers reap and render, and emptiness is the right answer for them".
    // The reaping half was right; the rendering half was the same collapse one
    // command over. A rendered emptiness *is* a claim — `omh s` printed `no
    // sessions` on stdout over a `worktrees/` it could not open, and `omh info`
    // printed an empty section. `list` returns a `Result` now; the two callers
    // that still take the lossy answer — `reap_idle` and `sessions_ls` — each
    // say why at the call site.
    // Nothing else may record between here and `live_is_certain` below: the
    // comparison is what makes a reason from *this* read mean the predicate
    // failed, and a reason from anywhere else would make every read one.
    let mark = unchecked.recorded();
    let live = match opened(
        &paths.worktrees(),
        "it cannot tell which sessions are live",
        &mut unchecked,
    ) {
        Opened::Listing(entries) => sweep(
            entries,
            &paths.worktrees(),
            "sessions",
            itself,
            always,
            &[],
            &mut unchecked,
        ),
        Opened::Absent | Opened::Refused => Vec::new(),
    };
    // One boolean, not a special case: a refused directory, a per-entry stream
    // error and an `is_dir` that would not answer all land in `unchecked`, and
    // any of them means `live` is short. `Absent` records nothing and so keeps
    // this true, which is right — a checkout that has never made a session
    // genuinely has none.
    let live_is_certain = unchecked.recorded() == mark;

    // A sandbox repository with no worktree — risk 8c on the risks page.
    // The most valuable orphan of the three: a container is re-creatable and a
    // run directory holds a timestamp, while this holds every commit the agent
    // made and nothing points at it. `omh <id> rm` clears it, and since #58
    // says what it would take with it first.
    let mut found = match opened(
        &paths.shadows(),
        "orphaned sandbox repositories went unchecked",
        &mut unchecked,
    ) {
        Opened::Listing(entries) => sweep(
            entries,
            &paths.shadows(),
            "sandbox repositories",
            |n| n.strip_suffix(".git").map(str::to_string),
            always,
            &live,
            &mut unchecked,
        ),
        Opened::Absent | Opened::Refused => Vec::new(),
    };

    // A run directory counts only when it carries the marker `idle::touch`
    // writes: `omh doctor` and `omh account login` stage into this same tree under their
    // own names, and neither is a session anybody could resume.
    found.extend(
        match opened(
            &paths.runs(),
            "orphaned run directories went unchecked",
            &mut unchecked,
        ) {
            Opened::Listing(entries) => sweep(
                entries,
                &paths.runs(),
                "run directories",
                itself,
                |id| match idle::recorded_use(&paths.runs(), id) {
                    // **A marker omh could not read is not an absent marker.**
                    // `last_used` collapsed both into `None`, so a run omh could
                    // list but not examine was recorded neither as a leftover nor
                    // as a read that failed.
                    Ok(seen) => Ok(seen.is_some()),
                    Err(e) => Err(format!(
                        "omh could not read when run {id} was last used, so it went \
                     unchecked: {e}"
                    )),
                },
                &live,
                &mut unchecked,
            ),
            Opened::Absent | Opened::Refused => Vec::new(),
        },
    );

    match backend {
        Ok(backend) => {
            let prefix = paths.container("");
            // **Could not look is not "none".** This swallowed its failure, so a
            // daemon that was down reported *fewer* leftovers rather than saying
            // it had not looked — the same collapse `Running` exists to prevent,
            // in the function whose whole job is to notice what is left behind.
            match backend.output(&["ps", "-a", "--format", "{{.Names}}"]) {
                Ok(out) if out.status.success() => found.extend(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter_map(|n| n.trim().strip_prefix(&prefix))
                        .map(str::to_string),
                ),
                Ok(out) => unchecked.note(format!(
                    "omh could not list containers, so orphaned sandboxes went unchecked: {}",
                    crate::image::unreadable(&String::from_utf8_lossy(&out.stderr), &out.status)
                )),
                Err(e) => unchecked.note(format!(
                    "omh could not list containers, so orphaned sandboxes went unchecked: {e}"
                )),
            }
        }
        // **A runtime omh never chose is a read it never made.** This half sat
        // inside `if let Some(backend)`, so on a machine with no container
        // runtime omh listed no container at all, `unchecked` came back empty,
        // and the row read "none — nothing orphaned on this machine" having
        // never looked for a sandbox. The reason travels as a `Result` rather
        // than being dropped at the call site, so this ends in `: {e}` like
        // every other line. `record` rather than `note`: both callers already
        // say this on stderr in their own words.
        Err(e) => unchecked.record(format!(
            "omh could not choose a container runtime, so orphaned sandbox \
             containers went unchecked: {e}"
        )),
    }

    found.retain(|id| !live.contains(id));
    found.sort();
    found.dedup();
    // **A failed predicate invalidates every candidate.** Everything above is a
    // *source*, and one failing loses candidates — which is what its reason
    // says, and what the other arms exist to report. `worktrees()` is the filter
    // applied to all of them, so a reason from it does not shorten this list, it
    // makes every name in it untrustworthy.
    //
    // Counted rather than named, for the reason the volumes half of this row is
    // (`doctor.rs`): an earlier version named caches it could not attribute and
    // handed over `docker volume rm` for them, which was `omh doctor` in one
    // checkout calling another's live cache garbage. Naming is what makes a
    // leftover actionable, and omh must not make something actionable that it
    // cannot stand behind — here the command on offer takes the sandbox
    // repository with it.
    if !live_is_certain && !found.is_empty() {
        unchecked.note(format!(
            "{} candidate{} went unnamed rather than be reported as orphans — \
             `omh <id> rm` on a live session takes the worktree and the sandbox \
             repository with it",
            found.len(),
            if found.len() == 1 { "" } else { "s" }
        ));
        found.clear();
    }
    Leftovers {
        found,
        unchecked: unchecked.reasons(),
    }
}
