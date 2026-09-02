//! The note store's commands: what is in it, and what is wrong with it.
//!
//! Verbs only. Everything they operate on lives in `crate::memory`, which is
//! where the store's own rules are — these read arguments, call it, and
//! decide what to print.

use crate::out;
use crate::profile::Paths;
use crate::{mcp, memory, report};
use anyhow::Result;

/// Record an observation. The key is derived, never chosen: an agent that picks
/// its own cannot be stopped from recording one event twice.
pub(crate) fn memory_remember(
    cwd: &std::path::Path,
    mut input: memory::Remembered,
    if_exists: memory::IfExists,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if input.source.trim().is_empty() {
        // Provenance is omh's to supply, so that it cannot be omitted. `cli` is
        // what this path is, and it is the whole of what omh can say without
        // being told: a session id used to be spliced in here from `-s`, which
        // made a global flag out of one text field. `--source` says it
        // outright, and `memory serve` supplies it for the agent.
        input.source = "cli".into();
    }
    ctx.say(&match memory::remember(&paths, &input, if_exists)? {
        memory::Wrote::Created(path) => {
            report::Action::new("note-recorded", format!("recorded {}", path.display()))
                .data(serde_json::json!({ "path": path.display().to_string(), "replaced": false }))
        }
        // Said out loud: a note that existed is gone, and only `--if-exists
        // override` gets here, so the caller asked for it and can check.
        memory::Wrote::Replaced(path) => report::Action::new(
            "note-replaced",
            format!(
                "replaced {} — the note that was there is gone",
                path.display()
            ),
        )
        .data(serde_json::json!({ "path": path.display().to_string(), "replaced": true })),
        memory::Wrote::Skipped(key) => report::Action::new(
            "note-already-there",
            format!("`{key}` is already recorded; left alone"),
        )
        .data(serde_json::json!({ "key": key, "replaced": false })),
    });
    Ok(())
}

/// Speak MCP until stdin closes.
///
/// Nothing but protocol may reach stdout, and one stray line breaks the very
/// first handshake. Every other command now writes through `out::Ctx`, which
/// puts answers on stdout and diagnostics on stderr — so the rule this comment
/// used to enforce by vigilance is enforced by the type. This function is the
/// exception that still owns its own stdout, because what it writes there is
/// not a report at all.
pub(crate) fn memory_serve(
    team: std::path::PathBuf,
    local: std::path::PathBuf,
    session: Option<String>,
) -> Result<()> {
    let mut server = memory::tools::Server {
        team,
        local,
        templates: memory::shipped_templates(),
        // omh already sets `OMH_SESSION` in the sandbox, so the base set can
        // declare static arguments and still record real provenance.
        session: session
            .or_else(|| std::env::var("OMH_SESSION").ok())
            .unwrap_or_else(|| "unknown".into()),
        client: None,
        today: memory::today,
        // Told, never worked out. This process runs inside the sandbox, where
        // the PEM `ca_cert` names is not mounted and the recipe cannot be
        // computed at all.
        recipe: memory::expiry::Recipe::from_env(),
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    mcp::serve(stdin, stdout, &mut server)
}

/// A join against facts omh already holds. Three groups, because they are
/// three different claims: the world moved, omh cannot tell, and omh was never
/// asked to tell.
pub(crate) fn memory_stale(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    use memory::expiry::Verdict;
    let paths = Paths::discover(cwd)?;
    let judged = memory::expiry::judge(&paths, &memory::load(&paths)?)?;

    /// Which group a verdict belongs to.
    ///
    /// A `match` on the verdict alone, so the compiler owns the mapping. The
    /// grouping used to match on `(&verdict, <integer tag>)`, where a `_` arm
    /// is unavoidable — a verdict added later fell through it, was counted in
    /// no group and tallied nowhere, so the note simply vanished from the
    /// command. `evaluate` refusing to collapse the third answer buys nothing
    /// if the printer drops the fourth.
    ///
    /// Returns `report::Age`, not the heading text. Handing the renderer a
    /// string put the same failure back one layer out: it filtered on literals
    /// that had to be spelled identically in two files, with nothing checking
    /// they were.
    fn age(verdict: &Verdict) -> report::Age {
        match verdict {
            Verdict::Stale { .. } => report::Age::Stale,
            Verdict::Unknown { .. } => report::Age::Unknown,
            Verdict::NoTrigger => report::Age::NoTrigger,
            Verdict::Fresh => report::Age::Fresh,
        }
    }

    let report = report::Stale {
        judged: judged
            .iter()
            .map(|j| report::Judged {
                key: j.key.clone(),
                layer: j.layer.to_string(),
                recorded: j.recorded.clone(),
                age: age(&j.verdict),
                because: match &j.verdict {
                    Verdict::Stale { because } | Verdict::Unknown { because } => {
                        Some(because.clone())
                    }
                    Verdict::NoTrigger | Verdict::Fresh => None,
                },
            })
            .collect(),
    };

    let stale = report.count(report::Age::Stale);
    let unknown = report.count(report::Age::Unknown);
    ctx.say(&report);

    // The report is the product, so it prints in full before this decides the
    // exit code — the same order `lint` uses.
    //
    // Three states in the output and one in the exit code is the same lie
    // `Unknown` exists to refuse, moved to the boundary a script actually
    // reads. Without a code of its own, a run where git was missing and *not
    // one probe could be answered* is indistinguishable from a clean store.
    if stale > 0 {
        anyhow::bail!(
            "{stale} note{} the world has moved past",
            if stale == 1 { "" } else { "s" }
        );
    }
    if unknown > 0 {
        std::process::exit(2);
    }
    Ok(())
}

/// local → team. §12's one human gate, because it is the one place a wrong
/// note reaches somebody else.
pub(crate) fn memory_promote(cwd: &std::path::Path, keys: &[String], ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let notes = memory::load(&paths)?;
    let repo = paths.repo.clone();

    let steps = match memory::promote::plan(&notes, &paths, keys, &|p: &std::path::Path| {
        memory::promote::git_ignores(&repo, p)
    }) {
        // `git_ignores` now answers or refuses to; a promotion is never
        // planned against a guess about where the note would land.
        Ok(steps) => steps,
        Err(blocked) => {
            // Nothing moved. A partial promotion would leave a store nobody
            // planned, and the human who ran the gate would have to work out
            // which half landed.
            for b in &blocked {
                ctx.warn(&b.say());
            }
            anyhow::bail!("promoted nothing");
        }
    };
    memory::promote::apply(&steps)?;
    ctx.say(&report::Promoted {
        text: memory::promote::report(&steps, &paths),
        keys: steps.iter().map(|s| s.key.clone()).collect(),
    });
    Ok(())
}

/// The store, by layer, with what points at each note.
pub(crate) fn memory_ls(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    ctx.say(&report::Notes {
        notes: memory::load(&paths)?,
    });
    Ok(())
}

/// The store-quality meter. Violations are grouped by rule rather than listed
/// flat, because the count per rule is the signal and the individual lines are
/// how you act on it.
pub(crate) fn memory_lint(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let found = memory::lint(&paths)?;
    let tally = memory::tally(&found);
    let report = report::Lint {
        violations: found,
        tally,
    };

    // The report is the product, so it prints in full before this decides the
    // exit code. Warnings do not fail the command: `Orphan` fires on every
    // note nothing links to, and a gate that is always red gates nothing.
    ctx.say(&report);
    let refused = report.refused();
    if refused > 0 {
        anyhow::bail!(
            "{refused} violation{} the schema refuses",
            if refused == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// One note, and a report of what pointed at it. Deletion never cascades: a
/// dangling link is visible and the lint finds it, while a silently pruned
/// neighbourhood is neither.
pub(crate) fn memory_rm(
    cwd: &std::path::Path,
    key: &str,
    layer: Option<memory::Layer>,
    at: Option<&str>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let removed = memory::remove(&paths, layer, key, at, dry_run)?;

    let mut action = report::Action::new(
        "note-removed",
        match dry_run {
            true => format!("would remove {key} ({})", removed.layer),
            false => format!("removed {key} ({})", removed.layer),
        },
    )
    .data(serde_json::json!({
        "key": key,
        "layer": removed.layer.to_string(),
        "committed": removed.layer.is_committed(),
        "inbound": removed.inbound,
    }));
    // The file is gone here, but a teammate still has it until the deletion is
    // committed. Saying so beats letting someone believe a shared note
    // disappeared for everybody.
    if removed.layer.is_committed() {
        action = action.note("it was committed — teammates keep it until you commit the deletion");
    }
    if !removed.inbound.is_empty() {
        action = action.note(format!(
            "still linked from {} — those links now dangle, and `omh memory lint` lists them",
            removed.inbound.join(", ")
        ));
    }
    ctx.say(&action);
    Ok(())
}
