//! What this project takes from your catalogue, and how a setup you already
//! have gets into it.
//!
//! `use`/`unuse` write the allowlist; `import` copies entries in from a
//! harness you configured by hand. They share this module because they are
//! two halves of one question — which of your things this repo uses — and
//! because both work on the same lists in `<repo>/.omh/settings.toml`.

use crate::adapter::{self, Adapter};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::{base, config, hook, render, report, selection, settings, stack};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Select a catalogue entry for this repo, or resync the whole list.
///
/// Writes the **committed** file. What a project uses is a fact about the
/// project, and a teammate cloning it should get the same selection — never
/// the gitignored file, which is where a value that must not be published
/// goes.
///
/// A capability with no list is following the whole catalogue, so adding one
/// name to it has to write the catalogue out first. Writing `["tdd"]` alone
/// would silently turn off everything else, which is the one thing a command
/// called `use` must never do.
pub(crate) fn use_cmd(
    cwd: &std::path::Path,
    capability: Option<&str>,
    name: Option<&str>,
    all: bool,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if all {
        if capability.is_some() {
            anyhow::bail!("`--all` resyncs every capability — it takes no arguments");
        }
        let lists = catalogue_lists(&paths)?;
        ctx.say(&report::Resynced {
            dry_run,
            wrote: write_lists(&paths, &lists, dry_run)?
                .into_iter()
                .map(|w| w.path.display().to_string())
                .collect(),
            counts: lists
                .iter()
                .map(|(cap, names)| (cap.to_string(), names.len()))
                .collect(),
        });
        return Ok(());
    }

    let (Some(key), Some(name)) = (capability, name) else {
        anyhow::bail!(
            "omh use <capability> <name>, or omh use --all\n  capabilities: {}",
            crate::cmd::settings::capability_list()
        );
    };
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    // A name nothing answers to is a typo far more often than a plan, and the
    // launcher would only report it later. `omh settings edit` is how you create
    // the entry first.
    let available = catalogue_names(&paths, cap)?;
    if !available.iter().any(|n| n == name) {
        // **Two states, and only one of them is absence.** `catalogue_names`
        // narrows hooks to the ecosystems this repo actually is, so in a rust
        // repo `go-test` falls out of it — and the refusal used to word that
        // narrowing as *your catalogue has no hooks called `go-test`*, which
        // `omh info` contradicts one command later by listing it.
        //
        // The second half cost more than the wrong sentence: it offered
        // `omh settings edit hooks go-test`, which creates a second
        // `go-test.json` beside the one already there.
        let held = Profile::resolve(&paths).entries(cap)?;
        if held.iter().any(|n| n == name) {
            anyhow::bail!(
                "{cap}/{name} is in your catalogue, but names an ecosystem this repo is \
                 not — nothing here would run it.\n  {cap} this repo can take: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            );
        }
        anyhow::bail!(
            "your catalogue has no {cap} called `{name}`. `omh settings edit {cap} {name}` \
             creates it.\n  {cap}: {}",
            if available.is_empty() {
                "(empty)".to_string()
            } else {
                available.join(", ")
            }
        );
    }
    let already = names.iter().any(|n| n == name);
    // "Already used" only means something once there *is* a list. While a
    // capability is still following the whole catalogue every name is used, and
    // saying so would leave `omh use` unable to start a selection at all.
    if already && !was_open {
        ctx.say(
            &report::Action::new(
                "capability-already-used",
                format!("{cap}/{name} is already used here"),
            )
            .data(serde_json::json!({
                "capability": cap.to_string(),
                "name": name,
                "changed": false,
            })),
        );
        return Ok(());
    }
    if !already {
        names.push(name.to_string());
    }
    let written = write_lists(
        &paths,
        &std::collections::BTreeMap::from([(cap, names.clone())]),
        dry_run,
    )?;
    // Said out loud, because this is the moment a capability turns from
    // "follows the catalogue" into "this list" — everything is still selected,
    // but from now on by name, and an entry added later will not be.
    // The tense the write actually happened in. Withholding it and printing
    // `wrote →` is the same lie one layer up from the one this flag was fixed
    // for: the file is untouched and the report says otherwise, which is the
    // part somebody reads.
    let did = |past: String, future: String| match dry_run {
        true => future,
        false => past,
    };
    let froze = was_open.then(|| {
        did(
            format!(
                "{cap} was following your whole catalogue; wrote its {} entries as the list",
                names.len()
            ),
            format!(
                "{cap} follows your whole catalogue; its {} entries would become the list",
                names.len()
            ),
        )
    });
    let paths = written_paths(&written);
    let mut action = report::Action::new(
        "capability-used",
        did(
            format!("using {cap}/{name}"),
            format!("would use {cap}/{name}"),
        ),
    )
    .data(serde_json::json!({
        "capability": cap.to_string(),
        "name": name,
        "changed": true,
        "froze_selection": was_open,
        "paths": paths,
    }));
    if let Some(line) = &froze {
        action = action.note(line);
    }
    for path in &paths {
        action = action.note(did(
            format!("wrote → {path}"),
            format!("would write → {path}"),
        ));
    }
    ctx.say(&action);
    Ok(())
}

/// Every file a write landed in, collapsed into one list for one report.
///
/// **This is what stops a command saying itself twice.** A repo can declare a
/// capability in both its shared and its gitignored layer, so these writers
/// loop; a `ctx.say` inside that loop emits a JSON document per layer, and two
/// documents concatenated are a parse error in whatever reads them. Calling
/// this is the shape that cannot make the mistake — the plural is in the value
/// rather than in the number of times the command speaks.
///
/// Guarded by `every_json_answer_is_one_document_and_not_several`.
pub(crate) fn written_paths(written: &[config::Written]) -> Vec<String> {
    written
        .iter()
        .map(|w| w.path.display().to_string())
        .collect()
}

/// Stop using a catalogue entry here.
pub(crate) fn unuse_cmd(
    cwd: &std::path::Path,
    key: &str,
    name: &str,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    if !names.iter().any(|n| n == name) {
        // Refused rather than written as a no-op: a name this repo never used
        // is a typo, and writing the list back would report success for it.
        anyhow::bail!(
            "{cap}/{name} is not used here. `omh info --repo` lists what is.\n  \
             using: {}",
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            }
        );
    }
    names.retain(|n| n != name);
    // The same disclosure `use_cmd` makes, and for the same reason: this is the
    // moment the capability stops following the catalogue. Discarding the flag
    // here was an oversight rather than a decision — `unuse` performs the
    // identical conversion, so a repo with no list at all freezes into one on
    // the command that was meant to remove one name.
    let froze = was_open.then(|| {
        format!(
            "{cap} was following your whole catalogue; wrote its remaining {} entries as the list",
            names.len()
        )
    });
    let remaining = names.len();
    let written = write_lists(
        &paths,
        &std::collections::BTreeMap::from([(cap, names)]),
        dry_run,
    )?;
    let paths = written_paths(&written);
    // **Said in the tense it happened in.** The write is withheld under
    // `--dry-run` and the sentence was not, so the file was left untouched and
    // the output still read `wrote →` — the same lie one layer up from the one
    // this flag was fixed for.
    let did = |past: String, future: String| match dry_run {
        true => future,
        false => past,
    };
    let mut action = report::Action::new(
        "capability-unused",
        did(
            format!("no longer using {cap}/{name}"),
            format!("would stop using {cap}/{name}"),
        ),
    )
    .data(serde_json::json!({
        "capability": cap.to_string(),
        "name": name,
        "froze_selection": was_open,
        "remaining": remaining,
        "paths": paths,
    }));
    if let Some(line) = &froze {
        action = action.note(line);
    }
    for path in &paths {
        action = action.note(did(
            format!("wrote → {path}"),
            format!("would write → {path}"),
        ));
    }
    ctx.say(&action);
    Ok(())
}

/// Write these lists to every repo layer that has a say in them.
///
/// One capability at a time, because which layers declare `skills` and which
/// declare `mcp` are different questions — `omh use --all` in a repo whose
/// gitignored file overrides exactly one capability must not acquire the other
/// five there.
pub(crate) fn write_lists(
    paths: &Paths,
    lists: &std::collections::BTreeMap<adapter::Capability, Vec<String>>,
    dry_run: bool,
) -> Result<Vec<config::Written>> {
    let mut out = Vec::new();
    for (cap, names) in lists {
        let one = std::collections::BTreeMap::from([(*cap, names.clone())]);
        for layer in config::declaring(paths, config::USE, &cap.to_string())? {
            // **Everything but the write.** The layers are resolved, the list
            // is built, and the record the report reads is the same one — only
            // persistence is skipped. A `--dry-run` that took a shortcut here
            // would be describing a different command from the one it claims
            // to be previewing.
            if dry_run {
                out.push(config::Written {
                    path: layer.file(paths),
                    layer,
                    committed: layer.is_committed(),
                });
                continue;
            }
            out.push(config::write_selection(paths, layer, &one)?);
        }
    }
    // Two capabilities can share a layer, and reporting the same file twice
    // reads as two writes.
    //
    // **Sorted first.** `dedup_by` only drops *adjacent* duplicates, and this
    // vec is built capability-outer/layer-inner, so a repo whose shared and
    // local files both declare `[use]` produces `[shared, local, shared,
    // local, …]` — where no two duplicates are ever adjacent and the dedup
    // removes nothing. `omh use --all` reported five writes to two files, and
    // `--json` said so in a five-element array.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// This capability's effective list, and whether it had one at all.
///
/// The name is validated here rather than at the write, which is the same rule
/// `[use]` follows: a name is checked where it is minted, so `omh use` cannot
/// put something in the file that reading the file would refuse.
/// Bring one capability across from a harness you already use.
///
/// **Hooks go to the repo; everything else goes to the catalogue.** That
/// asymmetry is the design rather than an accident: a hook binds to one
/// project's commands, and a skill, a rule or a command is a way *you* work and
/// travels with you. Importing a skill into a repo would be a skill you only
/// had in one place; importing a hook into the catalogue would put one
/// project's formatter in front of every other project you open.
pub(crate) fn import_cmd(
    cwd: &std::path::Path,
    capability: &str,
    harness: &str,
    from: Option<&std::path::Path>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let cap = adapter::Capability::from_key(capability).with_context(|| {
        format!(
            "`{capability}` is not a capability — expected {}",
            crate::cmd::settings::capability_list()
        )
    })?;
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let binding = adapter
        .supports(cap)
        .with_context(|| format!("{harness} has no {cap} for omh to read"))?;

    let source = match from {
        Some(f) => f.to_path_buf(),
        None => {
            let template = binding.import.as_deref().with_context(|| {
                format!(
                    "{harness} keeps its {cap} somewhere omh cannot read — \
                     `omh import {capability} {harness} --from <path>` if you know where"
                )
            })?;
            let home = dirs::home_dir().context("no home directory")?;
            adapter::expand_host(template, &home, &paths.repo)
        }
    };
    if !source.exists() {
        ctx.say(
            &report::Action::new(
                "import-nothing-there",
                format!("{harness} has no {cap} here ({})", source.display()),
            )
            .data(serde_json::json!({
                "harness": harness,
                "capability": cap.to_string(),
                "source": source.display().to_string(),
                "exists": false,
            })),
        );
        return Ok(());
    }

    match cap {
        // Hooks are translated rather than copied — they are the one capability
        // whose format is omh's own — and they land in the repo.
        adapter::Capability::Hooks => {
            import_hooks(&paths, &adapter, binding, &source, dry_run, ctx)
        }
        adapter::Capability::Mcp => anyhow::bail!(
            "MCP servers are `omh settings mcp import {harness}` — a server is a \
             record in one file, not an entry with its own"
        ),
        _ => import_entries(&paths, harness, cap, binding.render, &source, ctx),
    }
}

/// Copy into the catalogue what a harness already holds, entry by entry.
///
/// **Into `~/.omh/`, not the repo** — the opposite of hooks, and for the reason
/// `docs/configuration.md` gives: a skill is a way *you* work and travels with
/// you across projects, while a hook binds to one repo's commands. Importing a
/// skill into a repo would be a skill you only had in one place.
///
/// Rules are one file becoming one entry named after the harness it came from;
/// everything else is a directory whose children each become an entry. Which
/// shape a capability has is read off the adapter's `render`, not hardcoded —
/// the same field the launcher stages by.
///
/// Never clobbers. An entry already in your catalogue is left exactly as it is
/// and reported, so re-running is a no-op and an import cannot quietly replace
/// something you have since edited.
pub(crate) fn import_entries(
    paths: &Paths,
    harness: &str,
    cap: adapter::Capability,
    render: adapter::Render,
    source: &std::path::Path,
    ctx: &out::Ctx,
) -> Result<()> {
    let dest = paths.root.join(cap.source());

    let entries: Vec<(String, std::path::PathBuf)> = match render {
        // One file, one entry. Named after the harness rather than after the
        // file: `CLAUDE.md` in your catalogue says nothing about whose rules
        // they were, and `omh why rules/claude` is the question somebody asks.
        adapter::Render::Concat => vec![(format!("{harness}.md"), source.to_path_buf())],
        _ => {
            let mut found = Vec::new();
            let listing = std::fs::read_dir(source)
                .with_context(|| format!("reading {}", source.display()))?;
            for entry in listing {
                let path = entry
                    .with_context(|| format!("reading {}", source.display()))?
                    .path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                found.push((name.into_owned(), path));
            }
            found.sort();
            found
        }
    };

    let mut considered = Vec::new();
    for (name, from) in entries {
        // The stem, because a catalogue entry is a name and `review-diff.md` is
        // a filename. `validate_entry_name` then refuses `..`, a separator, and
        // every dotfile in one arm — so `../evil` cannot name an entry, and a
        // path cannot be smuggled in as one.
        let stem = std::path::Path::new(&name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if let Err(e) = selection::validate_entry_name(&stem, cap, source) {
            considered.push(report::Considered {
                name,
                verdict: report::Verdict::Skipped,
                detail: format!("{e:#}"),
            });
            continue;
        }
        let to = dest.join(if from.is_dir() {
            stem.clone()
        } else {
            name.clone()
        });
        if to.exists() {
            considered.push(report::Considered {
                name: stem,
                verdict: report::Verdict::Kept,
                detail: "already in your catalogue".into(),
            });
            continue;
        }
        considered.push(match copy_entry(&from, &to) {
            Ok(()) => report::Considered {
                name: stem,
                verdict: report::Verdict::Took,
                detail: String::new(),
            },
            Err(e) => report::Considered {
                name: stem,
                verdict: report::Verdict::Skipped,
                detail: format!("{e:#}"),
            },
        });
    }

    // Where the entries landed. `None` here said "nothing was written" to both
    // audiences on a run that had just copied files into the catalogue —
    // `mcp import` sets this and these did not, which is what made it an
    // omission rather than a convention.
    let took = considered
        .iter()
        .any(|c| c.verdict == report::Verdict::Took);
    ctx.say(&report::Imported {
        what: format!("{harness} {cap}"),
        source: source.display().to_string(),
        considered,
        noun: cap.to_string(),
        dry_run: false,
        wrote: took.then(|| dest.display().to_string()),
        selected_in: Vec::new(),
    });
    Ok(())
}

/// Copy one catalogue entry — a file, or a directory whole.
///
/// **Refuses any symlink**, at any depth, rather than following it or copying
/// it as a link. Following one lets a skill directory reach outside itself, and
/// the catalogue is mounted into every sandbox omh launches — so a link to
/// `~/.ssh` in somebody's skill would become a file the agent can read, in
/// every project, from a copy they had no reason to inspect. Copying the link
/// verbatim is no better: it points somewhere that means something else once
/// the entry has moved.
///
/// Refusing whole rather than skipping the link: an entry with a piece missing
/// is not a smaller version of that entry, and this is the same rule
/// `render::parse_hooks` applies to a handler it cannot say completely.
pub(crate) fn copy_entry(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    // Looked at **before** anything is written, so the common refusal never
    // starts a copy — and undone below if a write fails for any other reason,
    // because "refused whole" has to mean nothing was left behind. A
    // half-copied skill is mounted into every sandbox exactly as a whole one
    // is, and reads as an entry somebody chose.
    refuse_symlinks(from)?;
    if let Err(e) = copy_tree(from, to) {
        // Safe to remove: `import_entries` only calls this for a destination
        // that did not exist, so everything here is what this call just wrote.
        let undone = if to.is_dir() {
            std::fs::remove_dir_all(to)
        } else {
            std::fs::remove_file(to)
        };
        // **And the undo is not allowed to fail quietly.** It fails for the
        // same reasons the copy did — a read-only destination, a
        // permission-denied child — so the residue survives precisely in the
        // cases that produced it. The caller then prints `skipped`, which means
        // *nothing was written*, and the **next** run sees the partial entry,
        // reports `kept — already in your catalogue`, and mounts it into every
        // sandbox omh launches. A skill with its `SKILL.md` and none of its
        // scripts, presented as one somebody chose to keep.
        if let Err(u) = undone {
            return Err(e).with_context(|| {
                format!(
                    "and {} could not be removed ({u}) — a partial copy is still \
                     there, and the next import will report it as an entry you \
                     already have. Delete it before re-running.",
                    to.display()
                )
            });
        }
        return Err(e);
    }
    Ok(())
}

/// Refuse a symlink at any depth, before a byte is written.
pub(crate) fn refuse_symlinks(from: &std::path::Path) -> Result<()> {
    let meta =
        std::fs::symlink_metadata(from).with_context(|| format!("reading {}", from.display()))?;
    anyhow::ensure!(
        !meta.file_type().is_symlink(),
        "{} is a symlink, and omh will not copy one into a catalogue that is \
         mounted into every sandbox",
        from.display()
    );
    if meta.is_dir() {
        let listing =
            std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))?;
        for entry in listing {
            refuse_symlinks(&entry?.path())?;
        }
    }
    Ok(())
}

pub(crate) fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        let listing =
            std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))?;
        for entry in listing {
            let child = entry?.path();
            let name = child
                .file_name()
                .context("a path from read_dir has a name")?;
            copy_tree(&child, &to.join(name))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    Ok(())
}

/// Harnesses on this machine whose hooks omh could bring across.
///
/// **A report, never an action.** Importing writes executable content into
/// somebody's repo, and doing that because `init` found a file would be omh
/// deciding on their behalf what runs at the end of their turns. So `init`
/// names what is there and what would take it; `omh import hooks` is a
/// separate act somebody chooses.
///
/// Never fatal and never noisy: a harness with no config, a config that will
/// not parse, an adapter that declares no import path — all of them are simply
/// not mentioned. There is nothing to tell somebody about a file that is not
/// there.
pub(crate) fn importable(paths: &Paths, harnesses: &[String]) -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for name in harnesses {
        let Ok(adapter) = Adapter::find(&paths.adapters(), name) else {
            continue;
        };
        let Some(binding) = adapter.supports(adapter::Capability::Hooks) else {
            continue;
        };
        let Some(template) = binding.import.as_deref() else {
            continue;
        };
        let source = adapter::expand_host(template, &home, &paths.repo);
        // **Absent and unreadable are not the same thing**, and this function's
        // own justification used to conflate them: "there is nothing to tell
        // somebody about a file that is not there" is true, and a
        // `~/.claude/settings.json` full of hooks that is one comma short of
        // parsing *is* there. Silent, it produces the same output as a clean
        // machine — so somebody works in omh with none of their hooks, believes
        // omh found nothing of theirs, and never runs the one command that
        // would print the reason.
        let raw = match std::fs::read_to_string(&source) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.push(format!(
                    "import     {name}'s hooks are at {} and omh could not read \
                     it ({e})",
                    source.display()
                ));
                continue;
            }
        };
        let Ok(vocab) = hook::Vocabulary::of(binding, &adapter.tools) else {
            continue;
        };
        let (found, residue) = match render::parse_hooks(&raw, &vocab) {
            Ok(v) => v,
            Err(e) => {
                out.push(format!(
                    "import     {name} has hooks in {} that omh could not read \
                     ({e:#}) — omh import hooks {name}  to see why",
                    source.display()
                ));
                continue;
            }
        };
        if found.is_empty() && residue.is_empty() {
            continue;
        }
        out.push(format!(
            "import     {name} has {} hook{} omh can read{} — omh import hooks {name}",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            if residue.is_empty() {
                String::new()
            } else {
                format!(" and {} it cannot", residue.len())
            }
        ));
    }

    // And the capabilities that are copied rather than translated. Counted by
    // what is actually there — an empty `~/.claude/commands` says nothing worth
    // a line, and a line per harness per capability would bury the report in
    // things nobody has.
    for name in harnesses {
        let Ok(adapter) = Adapter::find(&paths.adapters(), name) else {
            continue;
        };
        for cap in adapter::Capability::ALL {
            if matches!(cap, adapter::Capability::Hooks | adapter::Capability::Mcp) {
                continue;
            }
            let Some(template) = adapter.supports(cap).and_then(|b| b.import.as_deref()) else {
                continue;
            };
            let source = adapter::expand_host(template, &home, &paths.repo);
            let held = match std::fs::read_dir(&source) {
                Ok(listing) => listing.count(),
                // A rules import is one file rather than a directory, so it
                // counts as one thing when it is there.
                Err(_) if source.is_file() => 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                // Same rule as the hooks half: a directory omh cannot read is
                // not a directory with nothing in it, and reporting zero would
                // be indistinguishable from a machine that has none.
                Err(e) => {
                    out.push(format!(
                        "import     {name}'s {cap} are at {} and omh could not \
                         read it ({e})",
                        source.display()
                    ));
                    continue;
                }
            };
            if held > 0 {
                out.push(format!(
                    "import     {name} has {held} {cap} — omh import {cap} {name}"
                ));
            }
        }
    }
    out
}

/// Bring hooks somebody already configured in a harness into this repo.
///
/// **Into `<repo>/.omh/hooks/`, never the catalogue.** A catalogue hook runs in
/// every repo you ever open, so importing one project's `prettier --write`
/// there would put it in front of every other project you touch — worse than
/// not importing at all, and invisible until it ran somewhere it should not
/// have.
///
/// **Copy, never move.** The harness keeps working exactly as it did; adopting
/// omh is not a migration you cannot back out of. The source file is not
/// touched at all.
///
/// Two failure modes this is written against, and both are silent:
///
/// - **A hook that lands and never runs.** `[use]` is what the launcher reads,
///   so a file written without being selected is a hook `omh import` counted
///   and no session will ever ship. The report would say `+6` and the launch
///   would ship none.
/// - **A hook that stops every launch.** A file answering to a name omh's base
///   manifest owns makes `merge_hooks` bail, which fails the whole session
///   rather than that one hook. Refused here, by name.
pub(crate) fn import_hooks(
    paths: &Paths,
    adapter: &Adapter,
    binding: &adapter::Binding,
    source: &std::path::Path,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let harness = &adapter.name;
    let raw =
        std::fs::read_to_string(source).with_context(|| format!("reading {}", source.display()))?;

    let vocab = hook::Vocabulary::of(binding, &adapter.tools)
        .with_context(|| format!("reading {harness}'s vocabulary backwards"))?;
    let (found, residue) = render::parse_hooks(&raw, &vocab)?;

    let manifest = base::Manifest::load_dir(&paths.base())?;
    // Every hook name the manifest owns, whether or not its feature is on
    // here — a repo with `codegraph` disabled must still not be handed a file
    // called `graph-refresh`, because enabling it later would then fail every
    // launch rather than that one hook.
    let reserved: std::collections::BTreeSet<String> = manifest
        .owns()
        .get(&adapter::Capability::Hooks)
        .map(|owned| owned.keys().cloned().collect())
        .unwrap_or_default();
    let dir = paths.repo.join(".omh/hooks");

    let mut considered = Vec::new();
    let mut written = Vec::new();
    for (name, hook) in &found {
        // A name omh's manifest owns is not a hook that would be shadowed —
        // it is a file `merge_hooks` refuses, which takes the whole session
        // down rather than just this hook. Refused here, where the person can
        // still see why.
        if reserved.contains(name) {
            considered.push(report::Considered {
                name: name.clone(),
                verdict: report::Verdict::Skipped,
                detail: "omh ships a hook by that name".into(),
            });
            continue;
        }
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            considered.push(report::Considered {
                name: name.clone(),
                verdict: report::Verdict::Kept,
                detail: "already here, left as it is".into(),
            });
            continue;
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(hook)?))?;
        considered.push(report::Considered {
            name: name.clone(),
            verdict: report::Verdict::Took,
            detail: hook.does().to_string(),
        });
        written.push(name.clone());
    }

    // Selected, or they land dead. This is the failure the whole feature is
    // most likely to have: files on disk, a report saying six, and a launch
    // that ships none of them because `[use]` never named them.
    let mut selected_in = Vec::new();
    if !written.is_empty() && crate::cmd::mcp::repo_has_selection(paths)? {
        let (cap, mut names, _) = current_list(paths, "hooks", &written[0])?;
        names.extend(written.iter().cloned());
        names.sort();
        names.dedup();
        let lists = std::collections::BTreeMap::from([(cap, names)]);
        for w in write_lists(paths, &lists, dry_run)? {
            selected_in.push(w.path.display().to_string());
        }
    }

    // Named, never silently left behind. A hook omh could not bring across is
    // still in the harness's own file and still running there, which is the
    // honest outcome — but somebody who was not told would think omh had taken
    // everything.
    for d in &residue {
        considered.push(report::Considered {
            name: d.name.clone(),
            verdict: report::Verdict::Left,
            detail: d.wanted.clone(),
        });
    }

    ctx.say(&report::Imported {
        what: format!("{harness} hooks"),
        source: source.display().to_string(),
        considered,
        noun: "hooks".into(),
        dry_run: false,
        // The hooks directory, for the same reason as `import_entries`: a run
        // that wrote files has to say where they went.
        wrote: (!written.is_empty()).then(|| dir.display().to_string()),
        selected_in,
    });
    Ok(())
}

pub(crate) fn current_list(
    paths: &Paths,
    key: &str,
    name: &str,
) -> Result<(adapter::Capability, Vec<String>, bool)> {
    let cap = adapter::Capability::from_key(key).with_context(|| {
        format!(
            "`{key}` is not a capability — expected {}",
            crate::cmd::settings::capability_list()
        )
    })?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let policy = settings::resolve(paths, &manifest)?;
    let file = config::Layer::Shared.file(paths);
    selection::validate_entry_name(name, cap, &file)?;
    if let Some(feature) = manifest
        .owns()
        .get(&cap)
        .and_then(|owned| owned.get(name))
        .cloned()
    {
        anyhow::bail!(
            "{cap}/{name} is omh's — part of the `{feature}` feature. `[use]` names \
             your entries; a feature is all or nothing, so `omh set {feature} on` \
             and `omh set {feature} off` are its switches, and \
             `omh unset {feature}` hands the decision back to omh's own default."
        );
    }
    match policy.selection.order(cap) {
        Some(names) => Ok((cap, names.to_vec(), false)),
        // No list: this capability follows the whole catalogue, so the list
        // that keeps that true is the catalogue itself.
        None => Ok((cap, catalogue_names(paths, cap)?, true)),
    }
}

/// Every capability's catalogue entries, minus the ones omh owns.
pub(crate) fn catalogue_lists(
    paths: &Paths,
) -> Result<std::collections::BTreeMap<adapter::Capability, Vec<String>>> {
    let mut out = std::collections::BTreeMap::new();
    for cap in adapter::Capability::ALL {
        out.insert(cap, catalogue_names(paths, cap)?);
    }
    Ok(out)
}

/// Which of these hooks this repo could ever take.
///
/// A hook naming an ecosystem this repo is not is dropped; a hook naming none
/// is kept, and so is a name nothing declared. **Applicability, not
/// selection** — `[use]` records what you chose from what you could have
/// chosen, and offering a rust repo `go-test` makes the unselected report
/// unreadable rather than more complete.
///
/// The asymmetry is deliberate: this drops what names an *undetected* stack,
/// rather than keeping what names a detected one. Written the other way it
/// would hide every hook that belongs everywhere, which is most of them.
/// Which of **this repo's** ecosystems something already speaks for.
///
/// The intersection is the whole of it, and leaving it out made a milestone's
/// worth of code unreachable. `declared_stacks` over the catalogue answers
/// `{rust, go, python}` in every repo on earth, because that is what omh
/// ships — so handed to `derive::hooks` as *covered* it meant
/// `covered.is_empty()` was never true, and every `Makefile`, `justfile` and
/// `Taskfile` derivation could not fire for anybody. Only node worked, because
/// omh ships no node hook, which is why nothing looked broken.
///
/// The user-visible end was worse than a missing hook: `ask::what_tests_it`
/// then said *"no stack it knows, no lockfile, no runner"* about a repo whose
/// `Makefile` omh had just read and whose `test` target it had found.
pub(crate) fn covered_here(
    hook_dirs: &[std::path::PathBuf],
    detected: &[&stack::Definition],
) -> Result<BTreeSet<String>> {
    Ok(render::declared_stacks(hook_dirs)?
        .into_values()
        .flatten()
        .filter(|named| detected.iter().any(|d| &d.name == named))
        .collect())
}

pub(crate) fn applicable_hooks(
    names: Vec<String>,
    declared: &BTreeMap<String, Option<String>>,
    detected: &BTreeSet<String>,
) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| match declared.get(n) {
            Some(Some(stack)) => detected.contains(stack),
            _ => true,
        })
        .collect()
}

/// The names a `[use]` list may hold for `cap`: what the catalogue and this
/// repo declare, minus omh's own, which `[omh]` governs and `[use]` refuses.
pub(crate) fn catalogue_names(paths: &Paths, cap: adapter::Capability) -> Result<Vec<String>> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let owned = manifest.owns();
    let profile = Profile::resolve(paths);
    let names: Vec<String> = profile
        .entries(cap)?
        .into_iter()
        .filter(|n| !owned.get(&cap).is_some_and(|o| o.contains_key(n)))
        .collect();
    if cap != adapter::Capability::Hooks {
        return Ok(names);
    }
    // Hooks alone can belong to an ecosystem, and omh now ships one set per
    // ecosystem. Offering a rust repo `go-test` would put every stack omh
    // knows into the list `init` writes and the launcher reports.
    let defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected: BTreeSet<String> = stack::detected(&defs, &paths.repo)
        .into_iter()
        .map(|d| d.name.clone())
        .collect();
    let declared = render::declared_stacks(&profile.sources(cap)?)?;
    Ok(applicable_hooks(names, &declared, &detected))
}
