//! Getting a session’s work back onto your branch.
//!
//! The loop the rest of omh exists for: read what the agent did, curate it,
//! land it. `crate::shadow` owns the sandbox’s own repository and the replay;
//! this is the half that runs on the host, decides what the user is allowed
//! to do next, and says why when the answer is no.

use crate::out;
use crate::profile::Paths;
use crate::session::{self, Session};
use crate::{config, doctor, image, report, runtime, shadow};
use anyhow::{Context, Result};
use std::process::Command;

/// Bring a session up to date with its base, without letting a commit of
/// yours into the sandbox.
///
/// The order is the design, and every step of it is recoverable from the one
/// before:
///
/// 1. **Checkpoint** in the sandbox, so `omh sNN log` shows the point this can
///    be undone from and `git checkout` inside reaches it.
/// 2. **Take the session's tree** from the same throwaway index a review uses.
/// 3. **Merge on the host**, which writes a tree and touches nothing.
/// 4. **Materialise** it into the worktree.
/// 5. **Move the baseline**, replaying the branch's own commits first.
/// 6. **Record it in the sandbox** as a commit the agent can read.
///
/// Nothing is written anywhere until the merge has succeeded, so a failure in
/// steps 1-3 leaves the session exactly as it was.
pub(crate) fn sync(
    cwd: &std::path::Path,
    id: Option<&str>,
    base: Option<&str>,
    down: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| session::default_branch(&paths.repo));

    stop_before_syncing(&paths, &session, down, ctx)?;
    ctx.say(&sync_session(&paths, &session, &base)?);
    Ok(())
}

/// The sync itself, once it is known to be safe to run.
///
/// Split from `sync` so it can be asserted: the command around it needs a
/// container runtime to decide whether the sandbox is up, and nothing that
/// needs one is reachable from a test here. What is left outside is the
/// refusal and the printing.
pub(crate) fn sync_session(paths: &Paths, session: &Session, base: &str) -> Result<report::Synced> {
    let branch = session
        .branch
        .as_deref()
        .context("a scratch session has no base to move")?;
    let was = session::head_of(&paths.repo, branch)?;
    let onto = session::head_of(&paths.repo, base)?;
    anyhow::ensure!(
        was != onto,
        "{} is already on {base}. Nothing to bring over",
        session.id
    );

    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    let checkpoint = shadow.checkpoint(&session.worktree, "Before omh brought the base forward")?;

    // Named, so the conflict markers the agent opens say which side is which.
    // Measured: `merge-tree` labels each side with the string it was handed,
    // so a bare tree renders as forty hex characters in the middle of somebody's
    // source file.
    let ours = format!("refs/{}", session.id);
    let tree = session.tree(base)?;
    session::name_tree(&paths.repo, &ours, &tree)?;
    let merged = session::merge_three(&paths.repo, &was, base, &session.id);
    let _ = session::unname_tree(&paths.repo, &ours);
    let merged = merged?;

    session.materialise(&tree, &merged.tree)?;
    session.move_baseline(&paths.repo, &onto, &was)?;
    shadow.record_base_moved(&session.worktree, &onto, &merged.conflicted)?;
    let moved = session.commits_between(&paths.repo, &was, &onto)?;
    // Deliberately not `?`. The sync is done — the tree is merged, the baseline
    // has moved and the shadow has its commit — and a note that could not be
    // written is a worse outcome to report than to carry: the user would read a
    // failed command about work that landed.
    //
    // Carried out on the report rather than printed here, which is three things
    // at once. The failure becomes assertable, since nothing captures a write
    // to stderr from inside a function; it reaches `--json`, which a bare
    // `eprint` structurally never does; and this function goes back to being
    // the part with no side effects, which is the only reason it is split out
    // at all. The first draft printed it here and quietly broke that.
    let note = shadow
        .leave_note(&shadow::note_for(base, moved, merged.conflicted.len()))
        // `{e:#}` and not `{e}`: `Display` on an `anyhow::Error` prints the
        // outermost context only, so the reason — `Permission denied`, a full
        // disk — is exactly the part that would be dropped.
        .err()
        .map(|why| format!("{why:#}"));

    Ok(report::Synced {
        id: session.id.clone(),
        moved,
        base: base.to_string(),
        onto,
        conflicted: merged.conflicted,
        checkpoint: checkpoint.is_some(),
        note,
    })
}

/// A sync needs the sandbox stopped, and says why rather than assuming.
///
/// Not about the files — the checkpoint makes an overwrite recoverable. It is
/// about the agent's **context**: what it believes the tree contains lives in
/// its conversation, not on disk, and no checkpoint reaches that. It would then
/// edit a version that no longer exists, or write a whole file back from stale
/// content, and trunk's changes vanish inside a plausible-looking patch nobody
/// notices until review.
///
/// Stopping is the fix rather than the price: the harness restarts and reads
/// the tree as it now is. Whether an agent mid-turn may be interrupted is a
/// judgement only the user can make, which is why `--down` exists and why the
/// default is to refuse.
pub(crate) fn stop_before_syncing(
    paths: &Paths,
    session: &Session,
    down: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    // Not a silent `return Ok(())`, which is what this was — and it sat one
    // line above the fix below, reaching the same outcome by a shorter path.
    // `runtime::installed` is itself `.unwrap_or(false)` over a `sh -c command
    // -v`, so a machine under fork pressure answers *no runtime installed*
    // when it means *could not ask*, and the sync then went ahead over a live
    // agent with nothing said at all.
    //
    // Refusing over a genuinely runtime-less machine is the cost, and it is
    // small: no runtime means no sandbox, so the session is not running, but a
    // sync is also not something that machine was going to do usefully. The
    // message says which it is and `--down` is not the way past — the way past
    // is a runtime.
    let backend = runtime::select(&crate::runtime_preference(paths), &|p| {
        runtime::installed(p)
    })
    .with_context(|| {
        format!(
            "omh cannot tell whether {}'s sandbox is running, so it will not sync over it",
            session.id
        )
    })?;
    let name = paths.container(&session.id);
    // The reason this whole class was worth fixing. `.unwrap_or(false)` here
    // meant an unreachable runtime read as *nothing is running*, and a sync
    // then wrote over the files of a live agent — the outcome the paragraphs
    // above call the worst available, reached by the one path they do not
    // discuss.
    if !must_know(
        image::container_running(backend.as_ref(), &name),
        &session.id,
        "sync over it",
    )? {
        return Ok(());
    }
    anyhow::ensure!(
        down,
        "{id} is running, and a sync moves files underneath it. What the agent believes \
         the tree holds is in its conversation rather than on disk, so it would keep \
         editing a version that no longer exists:\n  \
         omh {id} down          stop it, then sync\n  \
         omh {id} sync --down   both, if the turn is safe to interrupt",
        id = session.id
    );
    ctx.progress(&format!("stopping {} first", session.id));
    image::container_remove(backend.program(), &name)?;
    Ok(())
}

/// The sandbox's own history, read from the host.
///
/// `log_cmd` rather than `log`, because `log` is what a reader of this file
/// expects to be a logging helper.
pub(crate) fn log_cmd(
    cwd: &std::path::Path,
    id: Option<&str>,
    turns: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    ctx.say(&log_report(&paths, &session, turns, ctx)?);
    Ok(())
}

/// The decision, separated from the printing so it can be asserted.
///
/// Split out because nothing reached the wiring otherwise: with the whole
/// command ending in `ctx.say`, replacing the read below with an empty list
/// left the entire suite green — `omh sNN log` would have reported *no
/// checkpoints* for every session on earth, with the shadow tests passing
/// (they call `checkpoints` directly) and the report tests passing (they build
/// the value by hand). `the_resolution_is_read_and_written_in_the_committed_
/// layer` in this file already argues the general case: a correct reader called
/// with the wrong argument is the same bug with a passing guard in front of it.
pub(crate) fn log_report(
    paths: &Paths,
    session: &Session,
    turns: bool,
    ctx: &out::Ctx,
) -> Result<report::Log> {
    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);

    // Three states, and only one of them is *nothing to show*.
    //
    // `Path::exists` was the first version of this and it answers `false` for
    // every failure, not only for absence — a root-owned `~/.omh`, an
    // unmounted volume, a stale handle — and each of those would have printed
    // *the agent has not committed anything* over an exit code of 0, about a
    // session whose repository omh could not open. `landed` in `shadow.rs`
    // states the rule this now follows and was written for this exact reason:
    // cannot tell must not spell the same as clean.
    //
    // The pairing with the gitdir is the part worth reading twice. `ensure`
    // writes the seed record and *then* renames the gitdir into place, so a
    // launch killed in between leaves a seed with no repository — the harmless
    // way round, rebuilt by the next launch. The reverse, a repository with no
    // seed, is what `reap` leaves when `remove_dir_all` fails on a live mount
    // and the seed file goes anyway. That one holds every checkpoint the agent
    // made and no way to say where they started, and it must not be reported
    // as an empty session: the user's next move would be `rm`.
    let read = match std::fs::metadata(&shadow.seed_record) {
        Ok(_) => shadow.checkpoints(&session.worktree)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !shadow.gitdir.exists() => {
            shadow::Checkpoints::default()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "{} has a sandbox repository at {} and no record of where it started. omh \
             cannot tell you what the agent committed, and `omh {} rm` would remove the \
             repository. Read it directly first:\n  git --git-dir={} log",
            session.id,
            shadow.gitdir.display(),
            session.id,
            shadow.gitdir.display()
        ),
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "reading where {} started, at {}",
                    session.id,
                    shadow.seed_record.display()
                )
            })
        }
    };

    let base = session::default_branch(&paths.repo);
    // Three answers, and the report renders each differently. A count omh could
    // not take must not print as zero — `0 behind main` is reassurance — and
    // the reason it could not be taken is worth saying out loud rather than
    // becoming an absence.
    let behind = match session.behind(&paths.repo, &base) {
        Ok(behind) => Some(behind),
        Err(e) => {
            ctx.warn(&format!(
                "could not tell how far behind {base} this is: {e}"
            ));
            None
        }
    };

    // Read only when asked. It is a second `git log` over a ref most sessions
    // do not have, and `omh sNN log` is a command people run often.
    let turns = match turns {
        false => None,
        true => Some(shadow.turn_log(&session.worktree)?),
    };

    Ok(report::Log {
        id: session.id.clone(),
        read,
        behind,
        base,
        turns,
    })
}

pub(crate) fn diff(
    cwd: &std::path::Path,
    id: Option<&str>,
    checkpoint: Option<usize>,
    base: Option<&str>,
    patch: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let report = diff_report(&paths, &session, checkpoint, base, patch, ctx)?;

    // Paging is for a person, and only when there is something to page. Under
    // `--json` the patch is a field in the answer — a pager between a script
    // and the object it asked for is a hang with no error — and an empty diff
    // goes to `say` so the reader gets the sentence `Diff::human` exists to
    // give them. Three states used to render as one blank screen: nothing
    // changed, the worktree had left its branch, and the pager was broken.
    let paged = patch && ctx.format == out::Format::Human && report.changed();
    if !paged {
        ctx.say(&report);
        return Ok(());
    }
    // What the palette resolved, not what git guesses. `NO_COLOR=1` and
    // `--color never` were both walked past by handing the terminal over —
    // measured: git honours neither, and prints colour into a file under
    // `--color always` because its own `auto` sees one.
    let colour = match ctx.palette.is_plain() {
        true => "never",
        false => "always",
    };
    match checkpoint {
        Some(number) => shadow::Shadow::new(&paths.shadows(), &session.id).stream_show(
            &paths.repo,
            &session.worktree,
            number,
            colour,
        ),
        None => session.stream_diff(&report.base, colour),
    }
}

/// The answer, separated from the printing so it can be asserted.
///
/// The same split `log_report` makes, for the reason it records: with the
/// wiring inline, hardcoding `What::Summary` at the call site left the whole
/// suite green while `omh sNN diff` dumped a full patch to stdout.
pub(crate) fn diff_report(
    paths: &Paths,
    session: &Session,
    checkpoint: Option<usize>,
    base: Option<&str>,
    patch: bool,
    ctx: &out::Ctx,
) -> Result<report::Diff> {
    let what = match patch {
        true => session::What::Patch,
        false => session::What::Summary,
    };
    let Some(number) = checkpoint else {
        let base = base
            .map(str::to_string)
            .unwrap_or_else(|| session::default_branch(&paths.repo));
        let body = session.diff(&base, what)?;
        return Ok(report::Diff {
            label: session.label().to_string(),
            session: session.id.clone(),
            checkpoint: None,
            base,
            what,
            body,
        });
    };

    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    // The same triage `log_report` does, so two commands one word apart answer
    // a never-launched session the same way. Without it, `omh sNN log` said
    // *no checkpoints* and exited 0 while `omh sNN diff 1` quoted the path of
    // a seed record at a user who has never heard of one.
    anyhow::ensure!(
        shadow.seed_record.exists() || shadow.gitdir.exists(),
        "there is no checkpoint {number} in this session. The agent has not committed \
         anything here yet — its sandbox has never run"
    );
    let _ = ctx;
    Ok(report::Diff {
        label: format!("{} checkpoint {number}", session.id),
        session: session.id.clone(),
        checkpoint: Some(number),
        // A checkpoint is measured against its own parent, not against a
        // branch — naming the base branch here would be naming something this
        // diff was never taken against.
        base: "its parent".to_string(),
        what,
        body: shadow.show(&session.worktree, number, what)?,
    })
}

/// Turn what the user typed into what the harvest takes.
///
/// Everything refusable *about the selection* is refused here, before
/// `harvest` makes a worktree, fetches, or touches the branch. That is not
/// everything `--keep` can refuse — `harvest` still turns away carried
/// secrets, a worktree that left its branch and a replay point it cannot
/// place, all after the fetch — but those need the sandbox read to answer.
/// A number, a merge and a missing terminal do not.
///
/// `terminal` is passed rather than probed, the rule `Cli::output` states in
/// this file: *resolved here and passed down rather than consulted where it is
/// used*. Probing inline made the decision untestable — no test process has a
/// terminal, so every test took the same arm and a mutation reopening the
/// editor for every real user left the suite green.
pub(crate) fn what_to_keep(
    shadow: &shadow::Shadow,
    session: &Session,
    selection: &str,
    edit: bool,
    terminal: bool,
    keeps_a_selection: &dyn Fn() -> Result<bool>,
) -> Result<shadow::Keep> {
    // Before the terminal check, so a line that names the same thing twice is
    // told so rather than being told about a terminal it also lacks. The
    // opposite order made this unreachable: every test, and every script, hit
    // the tty message first and this could be deleted without a test noticing.
    anyhow::ensure!(
        !edit || selection.is_empty(),
        "`--edit` opens the whole list for editing, so `--keep {selection} --edit` names \
         what to take twice. Use one: `--keep {selection}` takes those, `--keep --edit` \
         opens all of them"
    );
    if edit {
        // The measured hole this closes: with stdin not a terminal, `rebase -i`
        // runs the *unedited* todo, exits 0, and omh reports a curation that
        // never happened. `--edit` is the only path that needs a person, so it
        // is the only one that has to ask.
        anyhow::ensure!(
            terminal,
            "`--edit` opens the list in your editor and there is no terminal here. \
             git would run the list unedited and report success. Drop `--edit` to keep \
             everything, or name what you want: `omh {} commit --keep 1,3-4`",
            session.id
        );
        return Ok(shadow::Keep::Edit);
    }
    if selection.is_empty() {
        return Ok(shadow::Keep::All);
    }

    // The one thing a selection needs that `--keep` alone does not, asked
    // before anything is read. `cherry-pick --empty=` is newer than everything
    // else omh asks of git — #56 made it a dependency without being able to
    // name the release — and on a git without it the flagship command answers
    // with a usage dump the user has no way to read as *your git is too old*.
    // Asked here rather than at the call site, so `--keep` and `--keep --edit`
    // — which need nothing new of git — do not fork one to throw the answer
    // away. Both return above this line.
    //
    // Three answers. *Could not ask* is git's own failure and is reported as
    // itself: the first version collapsed it into "no", so a user with no git
    // on PATH was told their git was too old to name checkpoints, and the
    // fallback omh recommended failed differently a second later.
    match keeps_a_selection().with_context(|| {
        format!(
            "omh cannot tell whether this git can name checkpoints, so it will not guess. \
             `omh {} commit --keep` takes them all and asks nothing new of git",
            session.id
        )
    })? {
        true => {}
        false => anyhow::bail!(
            "naming checkpoints needs a newer git than this one: `git cherry-pick` here has \
             no `--empty`, which omh uses to drop a commit whose changes are already on the \
             branch. `omh {} commit --keep` takes them all and works on any git omh supports",
            session.id
        ),
    }

    // Resolved against the session's own list, so a number means the commit it
    // meant on screen.
    let read = shadow.checkpoints(&session.worktree)?;
    let mut ids = Vec::new();
    for number in shadow::chosen(selection, read.commits.len())? {
        // `chosen` bounds the number by the length of this list and
        // `checkpoints` numbers it contiguously from 1, so this cannot miss.
        let checkpoint = &read.commits[number - 1];
        debug_assert_eq!(checkpoint.number, number);
        // Already on the branch. Replaying it would apply it twice, and the
        // divider in `omh sNN log` is where the user read that it was theirs.
        anyhow::ensure!(
            !checkpoint.landed,
            "checkpoint {number} is already on {}. `omh {} log` draws the line: everything \
             below it has been handed over, and handing it over again applies it twice",
            session.branch.as_deref().unwrap_or("the branch"),
            session.id
        );
        // A merge, which a selection replays one commit at a time and git will
        // not do without being told which side to take. Refused here rather
        // than by git after the fetch, where the message is *"is a merge but no
        // -m option was given"* — advice about a flag `--keep` forbids and that
        // means something else entirely in omh.
        anyhow::ensure!(
            checkpoint.touched.is_some(),
            "checkpoint {number} is a merge, and omh will not choose which side of one to \
             take. `omh {} commit --keep` replays the whole range instead — note that git \
             flattens a merge when it does",
            session.id
        );
        ids.push(checkpoint.id.clone());
    }
    Ok(shadow::Keep::These(ids))
}

/// What the sandbox answered, and what the host answered, in one list.
///
/// **The emptiness check runs before the concatenation, which is what lets the
/// host rows come first.** They always produce something, so folded into the
/// same list unchecked they would answer *yes, something ran* on behalf of a
/// container that did nothing, and `doctor` would pass on a probe that never
/// executed. That was the original reason host rows were appended last and
/// gathered here rather than passed in. The check is now made against
/// `from_the_sandbox` alone, before anything is joined, so the ordering is free
/// — and the host reads first because it is usually what explains a failing
/// sandbox.
///
/// **The host's side is a parameter now**, because `doctor_cmd` gathers it
/// before the container work: on a machine with no runtime the probe never
/// runs, and the host rows are the whole of what the reader needs. This doc
/// used to argue the opposite — that not having a parameter *was* the guard,
/// since as two bare `Vec<Outcome>` arguments the order was a convention and
/// swapping them or passing an empty list silenced the emptiness check or
/// dropped the host from the report, neither reachable by a test because
/// `doctor_cmd` needs a container.
///
/// That reasoning was right, so the guard moved into the types rather than
/// being given up: `HostRows` cannot be passed where the sandbox's rows go, and
/// an empty one is refused below.
pub(crate) fn every_check(
    from_the_sandbox: Vec<doctor::Outcome>,
    host: doctor::HostRows,
) -> Result<Vec<doctor::Outcome>> {
    anyhow::ensure!(
        !from_the_sandbox.is_empty(),
        "the probe produced no output — the sandbox did not run it"
    );
    // The other half of the guard the old shape got from not having a
    // parameter: an empty host side is a caller that gathered nothing, not a
    // host with nothing to say. `host_checks` always answers two rows and
    // `git_checks` at least one.
    anyhow::ensure!(
        !host.0.is_empty(),
        "no host checks were gathered — the report would say the sandbox is \
         fine and nothing about the machine it ran on"
    );
    // Host first. It used to be last, when `doctor::git_checks()` was called on
    // this line — which is why every host-side answer was produced *after* the
    // probe and therefore only on a machine where the probe could run.
    Ok(host.0.into_iter().chain(from_the_sandbox).collect())
}

/// Whether a commit may go ahead over what `git diff --check` found.
///
/// Both ways of committing refuse, because both would land the markers: `-m`
/// stages the files as they are, and `--keep` replants commits the agent made
/// on top of them. A conflict half-resolved compiles in neither case, and the
/// person who finds out is whoever reviews the branch.
///
/// The lines are a parameter rather than something read here, so every branch
/// of this is a table test — and so that the caller cannot ask git twice.
///
/// `--force` exists because a conflict marker at the start of a line is not
/// always a conflict: a test fixture holds them on purpose, and this very
/// repository has files that would trip it. Refusing something a user can
/// mean is a nuisance; refusing it with no way past is a bug.
pub(crate) fn may_commit(id: &str, unresolved: &[String], force: bool) -> Result<()> {
    if unresolved.is_empty() || force {
        return Ok(());
    }
    // Enough to act on without becoming the output itself. A whole-file
    // conflict is one marker per hunk and there can be hundreds; the count is
    // the scale and the first lines are where to start.
    let shown: Vec<&str> = unresolved.iter().take(5).map(String::as_str).collect();
    let rest = unresolved.len().saturating_sub(shown.len());
    anyhow::bail!(
        "{id} still has {n} conflict marker{s} in its files:\n  {lines}{more}\n\
         Resolve them first, or:\n  \
         omh {id} commit --keep --allow-conflicts   commit them anyway",
        n = unresolved.len(),
        s = if unresolved.len() == 1 { "" } else { "s" },
        lines = shown.join("\n  "),
        more = match rest {
            0 => String::new(),
            n => format!("\n  …and {n} more"),
        }
    );
}

/// Whether the reaper may consider stopping this session.
///
/// The one place the safe direction is inverted, and a named function rather
/// than a `matches!` in a filter so that it can be asserted at all — the
/// mutation that matters (`Unknown` becoming reapable) left the whole suite
/// green while this was inline.
///
/// Not `must_know`, because this runs on a timer with nobody to refuse to.
/// *Could not tell* keeps a session **out** of the list: leaving a sandbox up
/// costs a container, and stopping a live one on a guess costs somebody's
/// turn.
pub(crate) fn reapable(running: &image::Running) -> bool {
    matches!(running, image::Running::Yes)
}

/// *Could not tell* at a point where omh is about to act on the answer.
///
/// Not a `no`, and at a decision point not a `yes` either — so it is a
/// refusal, carrying the runtime's own words. Every caller of this is about to
/// create, enter, stop or overwrite a container on the strength of the answer,
/// and each of those is worse done blind than not done.
///
/// The alternative it replaces was `.unwrap_or(false)`, which is how a Docker
/// daemon that is down made every sandbox look stopped. An earlier draft of
/// this sentence said it "shipped for a year"; the repository is nineteen days
/// old. Nobody would have checked that, which is the reason to say the
/// checkable thing instead: it was there from the first commit, 2026-08-05.
pub(crate) fn must_know(running: image::Running, what: &str, doing: &str) -> Result<bool> {
    match running {
        image::Running::Yes => Ok(true),
        image::Running::No => Ok(false),
        image::Running::Unknown(why) => anyhow::bail!(
            "omh could not tell whether {what} is running, so it will not {doing}: {why}"
        ),
    }
}

/// Whether omh may delete work that exists nowhere else.
///
/// **One value, not two booleans.** A `force` flag and a "there is somebody at
/// a keyboard" flag are adjacent, the same type, and mean opposite things.
/// Swapping them is type-correct, and the two mistakes it makes are the worst
/// available here: prompting inside a script, or refusing a person standing
/// right there.
///
/// Three states because there are three behaviours, so a call site reads as
/// the decision rather than as two flags to combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consent {
    /// `--force`. Said deliberately, and the only way past without a person.
    Given,
    /// Nobody said anything, and there is somebody to ask.
    MayAsk,
    /// Nobody said anything, and there is nobody to ask — a script, a CI
    /// runner, a closed pipe. The refusal stands.
    CannotAsk,
}

/// `--force` was said.
///
/// A type rather than a `bool` because it travels beside `Interactive`, and
/// two `bool`s next to each other is the hazard `Consent` exists to remove.
///
/// Naming the two positions is only half of it. Wrapping them at
/// `may_remove`'s boundary moved the swap up to `rm`, which still took
/// `force: bool, terminal: bool`; moving it to `rm` moved it up to the
/// dispatch, where `Forced(x)` and `Interactive(y)` are both built from bools
/// and so are still swappable by hand. What closes it is having nothing to
/// swap: see `Interactive::of_stdin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Forced(pub bool);

/// There is somebody at a keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interactive(pub bool);

impl Interactive {
    /// The spelling every command uses. It takes no argument, so the flag
    /// cannot arrive here and the terminal cannot arrive as `Forced` — the
    /// transposition stops being expressible rather than being caught.
    ///
    /// The field stays constructible for tests, which need both answers
    /// without a terminal to read them from.
    pub(crate) fn of_stdin() -> Self {
        Self(std::io::IsTerminal::is_terminal(&std::io::stdin()))
    }
}

impl Consent {
    /// Combined here rather than at each call site, so "forced beats
    /// everything" has one spelling instead of one per caller.
    pub(crate) fn read(forced: Forced, interactive: Interactive) -> Self {
        match (forced.0, interactive.0) {
            (true, _) => Self::Given,
            (false, true) => Self::MayAsk,
            (false, false) => Self::CannotAsk,
        }
    }
}

/// What omh knows about a session's turn snapshots when it is asked to remove
/// it.
///
/// Three answers rather than a count, for the reason `AtStake` next door has
/// three: *could not tell* is not *none*, and this is the last thing in front
/// of an irreversible delete. The first version of this was a `usize` reached
/// through `.unwrap_or(0)`, so an unreadable sandbox removed itself in
/// silence — no count, no warning, nothing to act on.
#[derive(Debug)]
pub(crate) enum Snapshots {
    /// No `refs/omh/turn` here — this session has never finished a turn with
    /// anything changed.
    None,
    Kept(usize),
    /// omh asked and could not tell, and why.
    Unreadable(String),
}

/// Whether this session may be removed, or what stands in the way.
///
/// Separate from `rm` so the decision is assertable: `rm` takes a container
/// down, and nothing that needs one can be reached by a test here. What is
/// left in `rm` is the single call — its absence is a line missing from a
/// diff rather than a behaviour hiding behind a runtime.
pub(crate) fn may_remove(
    paths: &Paths,
    session: &Session,
    snapshots: Snapshots,
    consent: Consent,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> Result<Option<String>> {
    let branch = format!("omh/{}", session.id);
    // Named, never the reason. A snapshot is a tree omh photographed at the
    // end of a turn, not work the agent chose to keep — and there is one for
    // nearly every session that ever ran, so refusing over them would make
    // this guard fire almost always. A guard that fires almost always is one
    // people learn to answer with `--force` without reading, which is exactly
    // how it would stop protecting the commits it was built for.
    //
    // They are still worth a sentence, because they go too, and because the
    // one time they matter is the one time the agent threw the tree away.
    // Part of the sentence, not a line of its own: every line below the first
    // is a command the user can paste, and there is a test that says so.
    let also = match &snapshots {
        Snapshots::None => String::new(),
        Snapshots::Kept(n) => format!(", and {n} turn snapshot{} omh took", plural(*n)),
        // Said as its own clause rather than folded into a number, because a
        // count omh could not take is the one answer a user might want to go
        // and look into before deleting.
        Snapshots::Unreadable(why) => {
            format!(", and omh could not tell how many turn snapshots go with it ({why})")
        }
    };
    let snapshots = match snapshots {
        Snapshots::Kept(n) => n,
        // An unreadable count is *something* at stake, so the line that reads
        // them is still offered — it is the command that would say what.
        Snapshots::Unreadable(_) => usize::MAX,
        Snapshots::None => 0,
    };
    // …and the way to read them is a command, so it goes where commands go.
    // Padded to the column the other lines use — the test that reads these
    // only checks each line is pasteable, so a misaligned one stays green.
    let reading = match snapshots {
        0 => String::new(),
        _ => format!(
            "\n  omh {} log --turns         read the snapshots",
            session.id
        ),
    };
    let (what, whether) = match at_stake(paths, session) {
        // Nothing the branch lacks. The snapshots still go, so they are still
        // said — returned rather than printed, because this function's whole
        // value is being a decision a table test can reach.
        // Nothing of the agent's own at stake. The snapshots still go, so they
        // are still said — returned rather than printed, because this
        // function's whole value is being a decision a table test can reach.
        AtStake::Nothing => {
            return Ok(non_empty(match snapshots {
                0 => String::new(),
                usize::MAX => format!(
                    "omh could not tell how many turn snapshots go with {id}{also_why}. \
                     `omh {id} log --turns` would say",
                    id = session.id,
                    also_why = also
                        .split_once('(')
                        .map(|(_, w)| format!(" — {}", w.trim_end_matches(')')))
                        .unwrap_or_default()
                ),
                n => format!(
                    "{n} turn snapshot{} omh took go with {id}. `omh {id} log --turns` reads them",
                    plural(n),
                    id = session.id
                ),
            }))
        }
        AtStake::Work(what) => (what, "that no branch has"),
        // Could not tell, which is not the same as nothing to lose. A user is
        // asked once and `--force` is right there; the alternative is deleting
        // the only copy of work omh could not count, in silence.
        AtStake::Unknown(why) => (why, "and omh cannot say what that removes"),
    };
    // **Asked, when there is somebody to ask.** This only ever refused, and
    // the refusal's last line told you to retype the command with `--force` —
    // so the way to answer the safety question was to type the dangerous thing
    // from memory, with the reasons scrolled off. `omh s down` has asked for
    // its destructive case all along; this is the same shape.
    //
    // `--force` keeps its meaning for everything that is not a terminal: a
    // script, a CI runner, a closed pipe. `ask::confirm` treats silence and
    // anything-but-yes as no, which is what makes that safe.
    let at_stake = format!("{id} has {what} {whether}{also}", id = session.id);
    if consent != Consent::Given {
        let agreed = consent == Consent::MayAsk
            && crate::ask::confirm(
                &format!(
                    "{at_stake}. Removing it deletes the only copy.\nremove {id} anyway?",
                    id = session.id
                ),
                input,
                out,
            )?;
        anyhow::ensure!(
            agreed,
            "{at_stake}. Removing it deletes the only copy:\n  \
             omh {id} log                 read what is there\n  \
             omh {id} commit --keep       put it on {branch}\n  \
             omh {id} commit -m \"…\"       or take the files as they stand{reading}\n  \
             omh {id} rm --force          remove it anyway",
            id = session.id
        );
    }
    Ok(None)
}

/// `None` for the empty string, so "nothing to add" is a state rather than a
/// blank line somebody has to remember not to print.
pub(crate) fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// What a removal would destroy that exists nowhere else.
///
/// Three answers, because two would make *omh could not tell* spell the same
/// as *there is nothing here* — in the one command where that spelling is
/// unrecoverable. `Shadow::landed` states the rule one layer down and the
/// first version of this discarded it: `.exists()` and `.ok()` turned a
/// permissions error, a truncated replay record and a repository with no seed
/// into "go ahead and delete".
#[derive(Debug)]
pub(crate) enum AtStake {
    /// No sandbox was ever built here.
    Nothing,
    /// This much, counted.
    Work(String),
    /// A repository is there and omh could not read it. Why, in git's words.
    Unknown(String),
}

/// What the seed record alone settles, if anything.
///
/// The same triage `log_cmd` does, and for the same reason — `Path::exists`
/// answers `false` for every failure, not only for absence, so an unreadable
/// `~/.omh` read as *nothing to lose*. The one difference is what the third
/// arm does: `log` cannot show you the repository and says so; `rm` would
/// delete it, so it asks first.
///
/// Over the metadata result rather than the path, so all four answers are a
/// table. The permission arm cannot be produced by a test that might run as
/// root — `chmod 000` does not stop uid 0 — which is exactly the arm that
/// silently deleted a sandbox before this.
pub(crate) fn from_the_seed_record(
    metadata: std::io::Result<()>,
    gitdir_exists: bool,
    shadow: &shadow::Shadow,
) -> Option<AtStake> {
    match metadata {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !gitdir_exists => {
            Some(AtStake::Nothing)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(AtStake::Unknown(format!(
            "a sandbox repository at {} and no record of where it started",
            shadow.gitdir.display()
        ))),
        Err(e) => Some(AtStake::Unknown(format!(
            "a sandbox omh could not read at {}: {e}",
            shadow.seed_record.display()
        ))),
    }
}

pub(crate) fn at_stake(paths: &Paths, session: &Session) -> AtStake {
    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    if let Some(answer) = from_the_seed_record(
        std::fs::metadata(&shadow.seed_record).map(|_| ()),
        shadow.gitdir.exists(),
        &shadow,
    ) {
        return answer;
    }

    match shadow.unkept(&session.worktree) {
        Err(e) => AtStake::Unknown(format!("a sandbox omh could not read: {e}")),
        Ok((0, 0)) => AtStake::Nothing,
        Ok((0, files)) => AtStake::Work(format!("{files} uncommitted path{}", plural(files))),
        Ok((commits, 0)) => AtStake::Work(format!("{commits} commit{}", plural(commits))),
        Ok((commits, files)) => AtStake::Work(format!(
            "{commits} commit{} and {files} uncommitted path{}",
            plural(commits),
            plural(files)
        )),
    }
}

/// The `s` on a count, in the one place that decides what that looks like.
pub(crate) fn plural(n: usize) -> &'static str {
    match n {
        1 => "",
        _ => "s",
    }
}

/// The session a command acts on when it acts on work already done.
///
/// Deliberately not `session::pick`: that invents the *next* id when none
/// exists, which is right for a launch — it is about to create that worktree —
/// and wrong for every command that operates on a session that must already be
/// there. Committing into a fabricated id would fail somewhere further down,
/// about a path nobody named.
pub(crate) fn existing_session(paths: &Paths, explicit: Option<&str>) -> Result<Session> {
    let id = match explicit {
        Some(id) => {
            session::validate_id(id)?;
            id.to_string()
        }
        None => session::current(&paths.worktrees())
            .context("no sessions yet — start one with `omh new <harness>`")?,
    };
    let session = Session::new(&paths.worktrees(), id);
    anyhow::ensure!(
        session.worktree.exists(),
        "no session {} — `omh s` lists them",
        session.id
    );
    Ok(session)
}

/// The two ways to land a session's work, as one value.
///
/// They are mutually exclusive and clap already enforces that — this is what
/// stops the *rest* of the code from having to know. As four loose parameters
/// (`message`, `keep`, `edit`, and their combinations) a caller could pass a
/// message and a selection together, and what happened then was decided by
/// which `if` came first. The commit body says out loud why doing both is
/// wrong: the squash lands first and git's patch-id then drops every replanted
/// commit as already applied, so the granular history disappears with nothing
/// said. A pair that cannot be constructed cannot do that.
pub(crate) enum Landing<'a> {
    /// One commit of the files as they stand. `None` opens the editor.
    Squash(Option<&'a str>),
    /// The agent's own commits, replanted.
    Keep {
        /// Empty means everything since the last handover.
        selection: &'a str,
        /// The todo, in the user's editor.
        edit: bool,
    },
}

/// Say which carried files the content scan could not read.
///
/// A thin wrapper over `shadow::unscanned_warning` so the wording lives beside
/// the type that produces it rather than here — see that function for why
/// there is only one copy of this sentence.
pub(crate) fn say_what_went_unscanned(unscanned: &[shadow::Unscanned], ctx: &out::Ctx) {
    if let Some(msg) = shadow::unscanned_warning(unscanned) {
        ctx.warn(&msg);
    }
}

pub(crate) fn commit(
    cwd: &std::path::Path,
    id: Option<&str>,
    landing: Landing,
    skip_carried: bool,
    force: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let base = session::default_branch(&paths.repo);
    may_commit(&session.id, &session.unresolved(&base)?, force)?;

    // Two ways to land the same work, and the user picks which at the moment
    // they land it. `-m` squashes the files into one commit of their own and
    // never reads the sandbox's repository at all — it is the same `git add -A`
    // it always was. `--keep` replants the agent's commits, messages included.
    //
    // One writer either way. Doing both puts the squashed content on the branch
    // first, and git's patch-id then drops every replanted commit as already
    // applied — the granular history disappears with nothing said, which is the
    // whole thing `--keep` exists to deliver.
    // The `--keep` arm returns rather than yielding a message: it is a whole
    // command, and the squash below is the other one. Written as a match so
    // adding a third way to land is a compile error here rather than a
    // fall-through into the squash.
    let message = match landing {
        Landing::Squash(message) => message,
        Landing::Keep { selection, edit } => {
            let branch = session
                .branch
                .as_deref()
                .context("a scratch session has no branch to commit to")?;
            let shadow = crate::shadow::Shadow::new(&paths.shadows(), &session.id);
            let carried = config::policy_list(&paths, "carry_in");
            let keep = what_to_keep(
                &shadow,
                &session,
                selection,
                edit,
                std::io::IsTerminal::is_terminal(&std::io::stdin()),
                &|| shadow::git_supports("cherry-pick", "--empty"),
            )?;
            let named = match &keep {
                shadow::Keep::These(ids) => Some(ids.len()),
                _ => None,
            };
            let harvest = shadow.harvest(&paths.repo, &session.worktree, branch, &carried, keep)?;
            let landed = harvest.landed;
            // Said before the count, not after. This is the sentence that stops
            // "nothing carried reached the branch" from being read as "every
            // carried file was checked", and a caveat printed under the result
            // it qualifies is one people skip.
            say_what_went_unscanned(&harvest.unscanned, ctx);
            // Named four, landed three. git drops a commit whose patch is already
            // on the branch — measured, it says `patch contents already upstream`
            // on stderr and exits 0, and the helper that runs it keeps only stdout
            // on success. Without this the user reads `kept 3` after asking for
            // four and is left to wonder which one, or whether they miscounted.
            if let Some(named) = named.filter(|named| *named > landed) {
                ctx.warn(&format!(
                    "you named {named} checkpoint{}, and {landed} reached {branch} — git drops a \
                 commit whose changes are already there. `omh {} log` shows what is left",
                    if named == 1 { "" } else { "s" },
                    session.id
                ));
            }
            let n = session.commits(&paths.repo, &base);
            warn_uncounted(&n, ctx, &base);
            ctx.say(
                &report::Action::new(
                    "committed",
                    match landed {
                        // Two ways to keep nothing, and they are different news. A
                        // session that has never handed anything over has made no
                        // commits; one that has is simply up to date, and telling
                        // that user their agent "has made no commits" contradicts
                        // the branch they are looking at.
                        // Swallowed rather than propagated, because this only
                        // chooses between two ways of saying nothing happened and
                        // the harvest above already succeeded — failing here would
                        // report a command that worked as a command that did not.
                        //
                        // `true` on error, and the direction matters. `landed`
                        // fails only for a record that *exists* and could not be
                        // read, so the session has handed something over before:
                        // "nothing new" is then the true sentence and "has made no
                        // commits" is a stronger claim than omh can make, about a
                        // branch the user can see. An earlier version defaulted the
                        // other way and called it the vaguer answer. It is not.
                        0 if shadow.landed().map(|l| l.is_some()).unwrap_or(true) => format!(
                            "nothing new to keep — everything {} has committed is already on \
                         the branch",
                            session.label()
                        ),
                        0 => format!("nothing to keep — {} has made no commits", session.label()),
                        _ => format!(
                            "kept {landed} of {}'s own commits{}",
                            session.label(),
                            branch_tally(&n)
                        ),
                    },
                )
                .data(serde_json::json!({
                    "session": session.id,
                    "branch": session.label(),
                    "kept": landed,
                    "commits": n.as_ref().ok(),
                    "base": base,
                })),
            );
            return Ok(());
        }
    };

    // The same list the launcher copies from, so what `commit` refuses to
    // publish and what omh put there cannot disagree.
    let carried = config::policy_list(&paths, "carry_in");
    let policy = if skip_carried {
        session::Carried::skipping(&carried)
    } else {
        session::Carried::refusing(&carried)
    };
    session.commit(message, policy)?;

    // Counted against the base rather than reported as "committed", because the
    // number is what tells you whether the branch is worth pushing — and it is
    // the same number `omh s rm` will use to decide the branch survives.
    let base = session::default_branch(&paths.repo);
    let n = session.commits(&paths.repo, &base);
    warn_uncounted(&n, ctx, &base);
    ctx.say(
        &report::Action::new(
            "committed",
            format!("committed to {}{}", session.label(), branch_tally(&n)),
        )
        .data(serde_json::json!({
            "session": session.id,
            "branch": session.label(),
            "commits": n.as_ref().ok(),
            "base": base,
        })),
    );
    Ok(())
}

/// Say that the count could not be taken, where the answer merely omits it.
///
/// `branch_tally` going quiet is right for the answer — what omh did is true
/// whether or not it can count afterwards — but quiet is not the same as
/// unsaid. This is the same failure `omh s rm` will meet later, and meeting it
/// there for the first time, over a branch, is worse than hearing about it now
/// over a commit that already succeeded. On stderr, like every other warning,
/// so it stays out of anything being redirected.
pub(crate) fn warn_uncounted(n: &Result<usize>, ctx: &out::Ctx, base: &str) {
    if let Err(e) = n {
        ctx.warn(&format!(
            "could not count this branch against {base} — {e:#}"
        ));
    }
}

/// What a session's branch holds, appended to an answer that is true without it.
///
/// Empty when git could not take the count — `commits` returns a `Result`
/// precisely because a base that does not resolve is a question with no answer,
/// and *"(0 commits on the branch)"* is the wrong one. The sentence in front of
/// this reports what omh just did, which is true either way.
pub(crate) fn branch_tally(n: &Result<usize>) -> String {
    match n {
        Ok(n) => format!(
            " ({n} {} on the branch)",
            if *n == 1 { "commit" } else { "commits" }
        ),
        Err(_) => String::new(),
    }
}

pub(crate) fn push(
    cwd: &std::path::Path,
    id: Option<&str>,
    name: Option<&str>,
    pr: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let target = session.push(name)?;
    ctx.say(
        &report::Action::new("pushed", format!("{} → origin/{target}", session.label())).data(
            serde_json::json!({
                "session": session.id,
                "branch": session.label(),
                "target": target,
            }),
        ),
    );

    if !pr {
        return Ok(());
    }

    // Optional accelerant, never a dependency: a repo on a non-GitHub remote is
    // a normal repo, and a box without `gh` still has to be able to push. Saying
    // what to run beats half-succeeding and leaving the user to guess whether
    // the PR exists.
    anyhow::ensure!(
        runtime::installed("gh"),
        "gh is not installed; open it with\n  gh pr create --head {target}"
    );
    let status = Command::new("gh")
        .current_dir(&session.worktree)
        .args(["pr", "create", "--head", &target])
        .status()
        .context("running gh pr create")?;
    anyhow::ensure!(status.success(), "gh pr create did not open a pull request");
    Ok(())
}
