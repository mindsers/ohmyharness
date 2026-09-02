//! `omh init` — set this repo up, deciding everything and asking nothing.
//!
//! Every question is hassle the tool promised to remove, and most answers are
//! already lying around: manifests name the stack, git log names what you
//! work on. The two questions of last resort are in `crate::ask`, and they
//! only run when nothing on disk can answer.

use crate::adapter::{self, Adapter};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::{
    ask, base, bundled, config, derive, detect, doctor, facts, hook, image, key, memory, render,
    report, runtime, settings, stack,
};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

/// What to do next, in the order somebody does it.
///
/// A function rather than an expression inside `init`, because it is the one
/// instruction the product prints and it was a parse error for a release: the
/// line is *composed*, so the scan that reads printed `omh …` literals cannot
/// see it, and sweeping the doc transcripts to match made the docs show output
/// omh does not produce. Pulled out here so the parser can be asked.
///
/// One line was the whole of this, and it named the launch — which is right
/// and is not enough: the two lines under it are how you get *back* to the
/// session you are about to start, and somebody who never sees them starts a
/// second one instead. That is the failure `omh new` and `omh s resume` were
/// split apart to make impossible to reach by accident, and this is where a
/// first-time reader learns the pair exists.
///
/// `settings`, not `config` — the command that held your defaults was deleted
/// in 0.7.0, and telling a new user to run it on their very first command is
/// the worst possible place to leave a retired spelling.
pub(crate) fn next_after_init(harness: Option<&str>) -> Vec<(String, String)> {
    let Some(harness) = harness else {
        return vec![(
            "omh settings".into(),
            "no harness to start — your defaults are here".into(),
        )];
    };
    vec![
        (format!("omh new {harness}"), "start a session".into()),
        ("omh s resume".into(), "rejoin it later".into()),
        // `omh s attach`, not `omh s01 attach zed`. Two guesses in one line:
        // `s01` is a constant, and `init` is re-runnable — in a repo already
        // carrying `s01`, `omh new` makes `s04` and this advice would open an
        // unrelated session, successfully and silently. And naming `zed`
        // guesses at the machine in the commit that deleted the `editors` row
        // for being a fact about the machine. `attach` with no id takes the
        // session omh picks and `$EDITOR` if you have one, which is right on
        // every run.
        ("omh s attach".into(), "open it in your editor".into()),
    ]
}

/// Write one note per tracked document, plus one for what `init` derived.
///
/// Into the **committed** layer: a stub is reproducible from a document every
/// teammate already has, so it is not a claim from experience and does not need
/// a human to vouch for it. `promote` stays reserved for what an agent
/// observed.
pub(crate) fn seed_store(paths: &Paths) -> Result<String> {
    let templates = memory::templates(paths)?;
    let today = memory::today();
    let dir = memory::Layer::Team.dir(paths);

    let mut written = 0;
    let mut skipped = 0;
    let mut stubs = Vec::new();
    for doc in memory::ingest::documents(&paths.repo)? {
        let note = memory::ingest::stub(&doc, &templates, &today)?;
        stubs.push(note.key.clone());
        match memory::ingest::write(&dir, &note, memory::IfExists::Skip)? {
            true => written += 1,
            false => skipped += 1,
        }
    }

    let seeds = detect::seeds(
        &stack::load_all(&paths.stacks(), &paths.repo_stacks())?,
        &paths.repo,
    );
    if let Some(note) =
        memory::ingest::overview(&paths.repo_name(), &seeds, &stubs, &templates, &today)?
    {
        if memory::ingest::write(&dir, &note, memory::IfExists::Skip)? {
            written += 1;
        } else {
            skipped += 1;
        }
    }

    if written == 0 && skipped == 0 {
        return Ok("nothing to derive yet".into());
    }
    Ok(format!(
        "{written} note{} written, {skipped} already there",
        if written == 1 { "" } else { "s" }
    ))
}

/// Make sure the gitignored layer is actually gitignored before writing it.
///
/// The whole safety argument rests on this file being ignored, and until now
/// `omh set` asserted it rather than establishing it: the ignore line is
/// written by `omh init` alone, so in a repo that was never `omh init`ed —
/// or one where `.omh/.gitignore` was deleted — `omh set carry_in` created a
/// credential map that `git add .` would stage. The command that routed the
/// key there *because* it is secret-bearing is the right place to make that
/// true. Verified by `the_file_omh_hides_a_secret_in_is_a_file_git_ignores`.
pub(crate) fn ensure_ignored(paths: &Paths, ctx: &out::Ctx) -> Result<()> {
    let dir = paths.repo.join(".omh");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&ignore).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == settings::LOCAL) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(settings::LOCAL);
    next.push('\n');
    std::fs::write(&ignore, next).with_context(|| format!("writing {}", ignore.display()))?;
    ctx.warn(&format!(
        "nothing was ignoring {} — added it to {}",
        settings::LOCAL,
        ignore.display()
    ));
    Ok(())
}

/// The repo's first `settings.toml`, seeded from your defaults.
///
/// `~/.omh/default.toml` is a **template**, not a layer: nothing reads it at
/// launch, and this is the one moment it has any effect. That is the whole
/// argument — a repo's behaviour is explained by files inside the repo, which
/// is what a teammate cloning it can see, and what `omh info --repo` can account for
/// without pointing at a file they do not have.
///
/// **Assembled as a document, not as text.** The first version wrote
/// `format!("{k} ={}", item)`, which is correct only while every value is an
/// `Item::Value` and every key is bare — and a template is hand-edited. A
/// `[carry_in]` table emitted `carry_in =x = 1`, `init` wrote that file and
/// then failed parsing it, and `write_if_absent` never revisits, so re-running
/// `init` could not repair it: the repo was broken until somebody found and
/// deleted a file nothing had told them about. `toml_edit` was already in hand
/// and round-trips every one of those shapes.
///
/// Returns the file to write and what it took, so `init` can report it — a
/// seed nobody is told about is indistinguishable from a default.
pub(crate) fn seed_settings(paths: &Paths) -> Result<(String, Vec<String>)> {
    const HEADER: &str = "# What this repo decided. Settings at the top level; `[omh]` switches\n\
         # omh's own features off here without uninstalling anything.\n\
         #\n\
         # Untracked files the worktree needs — a worktree holds only tracked\n\
         # files, so without this the agent lands somewhere that cannot run your\n\
         # app. This is the ONLY path by which a secret reaches the agent, so\n\
         # keep it short and explicit. node_modules belongs in the image, not here.\n\
         #\n\
         # carry_in = [\".env.local\", \"certs/\"]\n";

    let template = config::Layer::Personal.file(paths);
    let doc = config::read_doc(&template)?;
    refuse_what_cannot_be_seeded(&doc, &template)?;

    let mut out = toml_edit::DocumentMut::new();
    let mut took = Vec::new();

    // Keys omh reads, and nothing else. A typo in your template is one you get
    // told about once, rather than one propagated into every repo you start.
    for k in key::KEYS {
        if let Some(item) = doc.get(k.name) {
            // No `is_value()` check here. `refuse_what_cannot_be_seeded` has
            // already refused every table but `[use]` and `[omh]`, so
            // `[carry_in]` never reaches this loop — a second guard for it
            // could not fire, and a check nothing can reach is decoration.
            out.insert(k.name, item.clone());
            took.push(k.name.to_string());
        }
    }
    // **No empty `carry_in`.** Seeding `carry_in = []` made the first thing
    // `omh info --repo` said about a fresh repo a setting nobody had set —
    // `carry_in  []  ← shared`, which reads as somebody's decision. The header
    // above already carries the commented example that teaches the key, and a
    // commented line is what a setting you are not setting looks like.
    //
    // A template that *does* name `carry_in` still travels: the loop above
    // takes it, because then somebody did choose it.

    // `[use]` and `[omh]` travel too: which entries a project takes and which
    // of omh's features it runs with are exactly the answers you do not want to
    // retype per repo.
    for table in [config::USE, config::OMH] {
        let Some(item) = doc.get(table) else { continue };
        let Some(t) = item.as_table_like() else {
            continue;
        };
        if t.iter().next().is_none() {
            continue;
        }
        out.insert(table, item.clone());
        took.push(format!("[{table}]"));
    }

    let body = out.to_string();
    // A backstop, and deliberately one: the refusals above cover every shape
    // known to break, and the original defect was a shape nobody had thought
    // of. Unreachable through anything currently spellable, which is why no
    // test kills a mutation of it — kept because "the input I did not imagine"
    // is exactly what put a corrupt `settings.toml` in a repo that `init`
    // could not then repair.
    let assembled = format!("{HEADER}{body}");
    toml::from_str::<toml::Table>(&assembled).with_context(|| {
        format!(
            "{}: omh could not turn this into a repo's settings file",
            template.display()
        )
    })?;
    Ok((assembled, took))
}

/// What a template may not hand on, refused by name.
///
/// Silence is the one option the reasoning rules out. `[mcp]` holds a server's
/// environment, which can be a token, and the file `init` writes is committed;
/// `[provision]` records which provides applied *on a machine*, so seeding one
/// claims a resolution that never ran. Dropping either without a word leaves
/// somebody believing a token is in force, or an opt-out is.
pub(crate) fn refuse_what_cannot_be_seeded(
    doc: &toml_edit::DocumentMut,
    at: &std::path::Path,
) -> Result<()> {
    anyhow::ensure!(
        doc.get("mcp").is_none(),
        "{}: `[mcp]` is not seeded into a repo — a server's environment can be \
         a token, and this template seeds a **committed** file.\n  \
         omh settings mcp add <name> <command> --env KEY=value   sets it on the \
         server instead",
        at.display()
    );
    anyhow::ensure!(
        doc.get(config::PROVISION).is_none(),
        "{}: `[{}]` is not seeded into a repo — it records what a *machine* \
         resolved, and copying it claims provisions that never ran here.\n  \
         omh init   records them per repo",
        at.display(),
        config::PROVISION
    );
    // `[omh]`'s contents, against the features omh actually ships. A template
    // naming a feature that no longer exists — an `[omh]` switch carried
    // forward from an older release, which is exactly what the rename's own
    // `mv` advice produces — was copied verbatim, `init` reported success, and
    // every later command failed with an error naming the **repo's** file. The
    // user debugs a file they never wrote, with nothing pointing back here.
    //
    // Checked against the bundled manifest rather than the installed one: on a
    // fresh machine `~/.omh/base` does not exist yet, and what ships is what
    // `init` is about to install.
    if let Some(table) = doc
        .get(config::OMH)
        .and_then(toml_edit::Item::as_table_like)
    {
        let shipped: std::collections::BTreeSet<String> = bundled::Shipped::Base
            .files()
            .iter()
            .filter_map(|f| toml::from_str::<base::Manifest>(f.contents).ok())
            .flat_map(|m| m.entries.into_iter().map(|e| e.feature))
            .collect();
        for (name, value) in table.iter() {
            anyhow::ensure!(
                value.as_bool().is_some(),
                "{}: `[{}] {name}` is not true or false, and omh reads it as a \
                 switch.",
                at.display(),
                config::OMH
            );
            anyhow::ensure!(
                shipped.contains(name),
                "{}: `[{}] {name}` names no feature omh ships ({}). Seeding it \
                 would make every new repo unreadable.",
                at.display(),
                config::OMH,
                shipped.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }

    // Anything else omh does not read. Named, because a table nobody seeds and
    // nobody warns about is a setting you believe is in force.
    for (name, item) in doc.iter() {
        if !item.is_table_like() {
            continue;
        }
        anyhow::ensure!(
            name == config::USE || name == config::OMH,
            "{}: `[{name}]` is read by nobody and is not seeded into a repo. \
             This file holds settings at the top level, `[{}]` for omh's own \
             features, and `[{}]` for what a project takes from your catalogue.",
            at.display(),
            config::OMH,
            config::USE
        );
    }
    Ok(())
}

pub(crate) fn init(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    // Fail fast. Everything below is wasted work outside a repo.
    let paths = Paths::discover(cwd)?;
    // And the template, for the same reason: it depends on nothing `init`
    // computes, and a refusal after fifteen writes leaves a half-made repo
    // while the message reads as though nothing happened.
    let seeded = seed_settings(&paths)?;

    // Filled in as the run goes and reported once at the end. See
    // `report::Init` for why this is not printed as it happens.
    let mut summary = report::Init::default();

    // A fresh install has no adapters, so `omh <harness>` would fail no matter
    // what else init did. Ship them before anything else.
    let adapters = install_bundled_adapters(&paths, ctx)?;
    // Shipped, and no longer reported here: what editors exist is a fact
    // about the machine, which is `omh info`'s question.
    install_bundled(&paths.editors(), bundled::Shipped::Editors, ctx)?;
    // The base set ships as data next to the adapters, for the same reason: the
    // opinion should be reviewable by the people it is imposed on. It travels
    // *inside* the binary now — otherwise a released omh installs nothing — but
    // it still lands as a file in `~/.omh/base`, which is where the
    // reviewability actually lives. `omh why` reads the file init seeds from.
    install_bundled(&paths.base(), bundled::Shipped::Base, ctx)?;
    // The stacks, for the same reason and by the same route: what a project
    // needs installed is omh's opinion, and an opinion imposed on somebody
    // should be one they can read. Managed, so a shipped fix always lands.
    install_bundled(&paths.stacks(), bundled::Shipped::Stacks, ctx)?;
    // And the conventional hooks, which used to be a `match` in Rust written
    // into every repo as two files. As catalogue data they are one body per
    // ecosystem instead of one per checkout, so a fix reaches everybody; a repo
    // needing its own spelling shadows the name, which is the rule hooks
    // already had. Each names the stack it belongs to and nothing else about
    // it — the marker stays in `stacks/`, so the two cannot drift.
    install_bundled(&paths.hooks(), bundled::Shipped::Hooks, ctx)?;
    // And the markers: ecosystems omh can recognise and cannot yet set up.
    // Data rather than a `match` for the same reason the stacks are — a marker
    // is removed by the same release that ships its stack, and the curation
    // test refuses the pair being true at once.
    install_bundled(&paths.markers(), bundled::Shipped::Markers, ctx)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    std::fs::create_dir_all(paths.worktrees())?;

    // The catalogue, empty and ready. Created rather than left absent so
    // `omh settings edit` has somewhere to open and the shape is discoverable
    // without reading a document.
    for cap in adapter::Capability::ALL {
        if cap != adapter::Capability::Mcp {
            std::fs::create_dir_all(paths.root.join(cap.source()))?;
        }
    }

    // Detect rather than ask — from the stacks just installed above, so this,
    // the provisioning below and the hook catalogue all read one set of
    // definitions rather than registries free to drift.
    //
    // One list now, where there used to be two: detection filtered through a
    // view that dropped any stack omh had no hook opinion about, so a
    // contributed ecosystem was provisioned and invisible in the report. A hook
    // names its stack instead, so a stack with no hooks is simply a stack with
    // no hooks — visible, provisioned, and waiting for somebody to contribute
    // one.
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let stacks = stack::detected(&stack_defs, &paths.repo);
    let names: Vec<String> = adapters.to_vec();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h));

    // What a repo holds: settings, memory configuration, and hooks. No skills,
    // no MCP servers, no commands, no subagents — those are yours, and a repo
    // names them rather than shipping them.
    let repo_omh = paths.repo.join(".omh");
    std::fs::create_dir_all(repo_omh.join("hooks"))?;
    // Both halves of the note store. The committed half lives in the repo
    // because that is what makes it reach a teammate; the local half lives
    // under `~/.omh`, because a worktree holds only tracked files and
    // `omh s rm` removes it with `--force`.
    for layer in memory::Layer::ALL {
        std::fs::create_dir_all(layer.dir(&paths))?;
    }
    // `write_if_absent`, never the refresh path the adapters use: a shipped
    // template that changed under an existing store would silently re-key
    // every note in it, and every existing key would stop being derivable.
    write_if_absent(&repo_omh.join(memory::TEMPLATES), memory::SHIPPED_KEYS)?;
    // No `AGENTS.md` is written. omh's own sections are base-set entries,
    // composed into every session from the manifest, which is what lets a fix
    // reach a repo that ran `init` a year ago. The detected stack is not prose
    // either: it produces hooks, and a sentence describing a test command is
    // not the thing that runs it.

    // The base set: omh's opinion, seeded into your catalogue where it is
    // visible, reviewable, and removable rather than hidden in the binary.
    // `write_if_absent`, so a server you removed does not come back.
    let base_mcp =
        serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": manifest.servers() }))?
            + "\n";
    write_if_absent(&config::mcp_path(&paths), &base_mcp)?;
    let (contents, from_template) = seeded;
    // Only when this run actually created the file. Reporting a seed over a
    // settings.toml that was already there would claim an effect the template
    // did not have — `write_if_absent` never revisits.
    if write_if_absent(&repo_omh.join("settings.toml"), &contents)? {
        summary.seeded = from_template;
    }
    // No hooks are seeded into the repo. omh's own are generated from the
    // manifest at launch, which is the only arrangement in which omh can ship a
    // fix to them: `write_if_absent` never revisits, so a repo initialised
    // before `git-unavailable` was rewritten would have run the broken pattern
    // forever. The conventional ones are catalogue files for the same reason —
    // `cargo test` is what a rust project runs, not what *this* rust project
    // runs, so one body per ecosystem is the honest scope and a fix reaches
    // everybody who already ran `init`.
    //
    // What a repo still declares is a hook only it could want, in
    // `<repo>/.omh/hooks/`, which shadows a catalogue name by the rule
    // `merge_hooks` already applies. That is the whole of what changed: the
    // *scope* of the conventional hooks, not whether a repo may have its own.
    //
    // Some of those omh can work out. A node project's test command depends on
    // which package manager it uses and whether it declared a `test` script at
    // all, so the catalogue cannot hold it and `derive` reads it off the files
    // the project already commits — for ecosystems the catalogue does not
    // already cover, so a rust repo's `Makefile` does not earn a second hook
    // that runs the suite again.
    //
    // `write_if_absent`, so a hook somebody has since edited is never
    // rewritten, and **serialised** rather than formatted: a command with a
    // quote in it — which is now a command omh read out of somebody's
    // `package.json` rather than one of four literals — would otherwise
    // produce a file nothing can parse.
    let covered = crate::cmd::catalogue::covered_here(&[paths.hooks()], &stacks)?;
    let derived = derive::hooks(
        &paths.repo,
        &settings::resolve(&paths, &manifest)?.provision,
        &covered,
    );
    if !derived.is_empty() {
        std::fs::create_dir_all(repo_omh.join("hooks"))?;
        for d in &derived {
            write_if_absent(
                &repo_omh.join("hooks").join(format!("{}.json", d.name)),
                &format!("{}\n", serde_json::to_string_pretty(&d.hook)?),
            )?;
        }
    }

    // And only now, the two questions — after every derivation has had its go,
    // which is what makes them *last* resort rather than a wizard's opening.
    //
    // Two conditions, both narrow. A marker omh recognises and no stack claims
    // is the one case where the repo plainly is something and omh cannot say
    // what its sandbox needs. A project with no test hook from any source is
    // the one case where the agent cannot check its own work.
    let markers = stack::markers(&paths.markers())?;
    let unclaimed = stack::unclaimed(&markers, &stack_defs, &paths.repo);
    let has_test = covered.iter().any(|s| stacks.iter().any(|d| &d.name == s))
        || derived.iter().any(|d| d.hook.on == hook::Event::TurnEnd)
        || repo_omh.join("hooks").join("test.json").exists();
    let (asked, answered) = questions(&repo_omh, &unclaimed, has_test, ctx)?;

    // **Reloaded, because an answer is a stack file.** `how_is_it_installed`
    // writes `<repo>/.omh/stacks/<name>.toml`, and everything below — the
    // report, the predicates, the recorded resolution, the image layer — reads
    // `stack_defs`. Left stale, somebody typed how to install elixir, watched
    // omh say `stack elixir — from what you told it`, and then watched the same
    // run print `stack none detected` and build a sandbox with no elixir in it.
    // Their answer took effect on the *next* `init`, and nothing said so.
    //
    // Unconditional rather than gated on `asked > 0`: it costs one directory
    // read, and a gate is a second thing to keep true.
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let stacks = stack::detected(&stack_defs, &paths.repo);
    //
    // The selection, written out with every catalogue entry named — after the
    // catalogue is installed and the derived hooks are written, so both are in
    // the list it writes.
    //
    // Expanded rather than `"*"`, because an explicit list is editable and
    // reviewable in a way a wildcard is not: you curate by deleting lines. That
    // has one failure mode — an entry added to the catalogue *afterwards* is not
    // in the list, so it is off and the reason is invisible — and the launcher
    // reports exactly that, which is what makes writing it expanded safe.
    //
    // Only when there is no `[use]` already: `write_if_absent` guards the file,
    // not the table, and re-running `init` in a curated repo must not resync a
    // list somebody pruned on purpose. `omh use --all` is how you ask for that.
    if !crate::cmd::mcp::repo_has_selection(&paths)? {
        let lists = crate::cmd::catalogue::catalogue_lists(&paths)?;
        config::write_selection(&paths, config::Layer::Shared, &lists)?;
    } else {
        // A curated list is not resynced — and a hook **this run just wrote**
        // still has to reach it, or it lands dead. `merge_hooks` drops any hook
        // the selection does not name, so a repo `init`ed six months ago that
        // has since gained a `package.json` gets `pnpm-test.json` written, sees
        // it reported, and never runs it. `import_hooks` already guards exactly
        // this; the same rule applies to what `init` writes.
        //
        // Added, never resynced: the point of a curated list is that omh does
        // not put back what somebody pruned. These are names that did not exist
        // when they pruned it.
        let mine: Vec<String> = derived
            .iter()
            .map(|d| d.name.clone())
            .chain(answered.iter().cloned())
            .collect();
        if !mine.is_empty() {
            let (cap, mut names, _) =
                crate::cmd::catalogue::current_list(&paths, "hooks", &mine[0])?;
            names.extend(mine);
            names.sort();
            names.dedup();
            let lists = std::collections::BTreeMap::from([(cap, names)]);
            crate::cmd::catalogue::write_lists(&paths, &lists, false)?;
        }
    }

    // Appended, not overwritten: re-running init must not eat a line you added.
    let gitignore = paths.repo.join(".omh/.gitignore");
    // Left tracked, a machine-local override gets committed to the team's repo.
    ensure_line(&gitignore, settings::LOCAL)?;

    // Only now the image, and the question about what it turned out to hold.
    //
    // Everything above configures the repo and cannot fail for want of a
    // container; everything here needs one and propagates when there is none.
    // Ordered this way round deliberately: an earlier arrangement built the
    // image first, so `omh init` on a box with no runtime — somebody who
    // installed omh before docker, which is the order most people do it in —
    // left the repo with hooks, no `[use]` list, and `settings.local.toml`
    // still tracked. Setting a repo up must not be abandoned half-done because
    // the machine cannot build an image yet.
    // Which of this repo's hooks the sandbox turned out to be unable to run.
    // Measured, not asked about — see the block below.
    // One value, so *nothing held back* and *nothing asked* cannot both be
    // said and cannot be confused. The reason is written by whichever gate
    // stops the measurement, and it starts as the first of them.
    //
    // A single string set here and only ever cleared was the first attempt,
    // and it told a repo that has a harness, an image, and a probe that failed
    // `not measured — no harness`, two rows under the line naming its harness.
    // A value that exists to end a misleading silence must not replace it with
    // a misleading sentence.
    let mut hooks = report::Hooks::Unchecked("no harness, so no sandbox to ask".into());
    // Once, above every block that needs it. `init` read this setting three
    // times — twice in this function and again inside `sandbox()` — so a PEM
    // edited mid-run produced two different tags for one command. Reading it
    // here also fails early, before any image is built, which is where a
    // certificate omh cannot read should stop.
    let ca = image::ca_for(&paths)?;
    if let Some(h) = &harness {
        // Past the first gate, so the harness is no longer the reason. Set
        // before the probe rather than after it, because the two arms that
        // fail below return an empty answer and read as ordinary — the reason
        // has to be true from the moment it stops being the previous one.
        hooks = report::Hooks::Unchecked("the sandbox could not be asked".into());
        let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
            runtime::installed(p)
        })?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        // Without it the headline command cannot run, so init is not finished
        // until this exists — and until it exists there is no sandbox to ask
        // about a toolchain.
        if image::exists(backend.program(), &image::tag_for(&adapter, ca.as_deref())) {
            summary.image = Some(format!(
                "{} (already built)",
                image::tag_for(&adapter, ca.as_deref())
            ));
        } else {
            // Progress, not report: this is the minutes-long step, and
            // somebody watching a blank terminal needs to know it is alive.
            ctx.progress(&format!(
                "building {} — first run only…",
                image::tag_for(&adapter, ca.as_deref())
            ));
            image::ensure(backend.program(), &adapter, ca.as_deref())?;
            summary.image = Some(image::tag_for(&adapter, ca.as_deref()));
        }

        // Which provides apply here. Evaluated **in the sandbox**, with the repo
        // mounted read-only: a predicate is arbitrary shell out of a stack file,
        // and running it on the host during `init` is the one thing omh exists
        // to avoid.
        let detected = stack::detected(&stack_defs, &paths.repo);
        let candidates: Vec<(String, Option<&str>)> = detected
            .iter()
            .flat_map(|d| {
                d.provides
                    .iter()
                    .map(move |p| (stack::key(&d.name, &p.name), p.when.as_deref()))
            })
            .collect();

        {
            // No `if !candidates.is_empty()` guard. A repo with nothing to ask
            // has still been answered — the answer is "nothing applies" — and
            // skipping would leave a resolution recorded when this repo *was* a
            // rust project asserting `rust/toolchain = true` for ever.
            // Why the probe itself failed, when it did. Kept apart from
            // `hooks_unchecked` so the specific reason outranks the generic
            // one derived below: *the sandbox could not be asked (exit 3)*
            // says what to go and fix, and *answered for 0 of 2 conditions*
            // is the same event counted from the other end.
            let mut probe_problem: Option<String> = None;
            let answered = if candidates.is_empty() {
                Vec::new()
            } else {
                match Command::new(backend.program())
                    .args(stack::predicate_args(
                        &image::tag_for(&adapter, ca.as_deref()),
                        &paths.repo,
                        &stack::predicate_script(&candidates),
                    ))
                    .output()
                {
                    // A container that ran and failed is not an answer. Only
                    // `Err` was handled before, so `docker run` failing — image
                    // gone, mount refused, no space — produced empty stdout,
                    // read as "nothing applies", and `init` went on to print
                    // its summary with nothing said. The `Err` arm's own
                    // comment forbids exactly that.
                    Ok(out) if !out.status.success() => {
                        summary.problems.push(format!(
                            "the sandbox could not be asked ({}) — nothing recorded",
                            out.status
                        ));
                        probe_problem =
                            Some(format!("the sandbox could not be asked ({})", out.status));
                        for line in String::from_utf8_lossy(&out.stderr).lines().take(3) {
                            summary.problems.push(line.to_string());
                        }
                        Vec::new()
                    }
                    Ok(out) => doctor::parse(&String::from_utf8_lossy(&out.stdout)),
                    Err(e) => {
                        // Non-fatal, and never fatal *silently*: `init` sets a
                        // repo up, and failing that over a diagnostic would be
                        // the tail wagging the dog — but saying nothing would
                        // let somebody believe the sandbox had been checked.
                        summary.problems.push(format!(
                            "could not ask the sandbox ({e}) — nothing recorded"
                        ));
                        probe_problem = Some(format!("could not ask the sandbox ({e})"));
                        Vec::new()
                    }
                }
            };

            for a in answered.iter().filter(|a| !a.ok) {
                if let stack::Verdict::CouldNotAnswer(code) = stack::verdict(a) {
                    summary.problems.push(format!(
                        "{}'s condition could not answer{} — not applied",
                        a.name,
                        code.map(|c| format!(" (exit {c})")).unwrap_or_default()
                    ));
                }
            }

            // Recorded only when something was actually measured. `reconcile`
            // drops every `true` it is not told about, so writing an empty
            // answer would erase the repo's resolution rather than leave it be.
            if fired_from(candidates.len(), &answered).is_none() {
                hooks = report::Hooks::Unchecked(probe_problem.unwrap_or_else(|| {
                    format!(
                        "the sandbox answered for {} of {} conditions",
                        answered.len(),
                        candidates.len()
                    )
                }));
            }
            if let Some(fired) = fired_from(candidates.len(), &answered) {
                let recorded = record_resolution(&paths, &fired)?;
                for key in recorded.iter().filter(|(_, on)| **on).map(|(k, _)| k) {
                    summary.provisioned.push(key.clone());
                }

                // The stack layer, through the same function every launch
                // reads — so what `init` reports built is what `omh new` and
                // `omh sNN resume` run, by construction rather than by two
                // implementations agreeing.
                //
                // Re-resolved from disk rather than reusing `recorded`, which
                // is the committed table alone: `record_resolution` has just
                // written it, and a `false` in `settings.local.toml` means *not
                // on this laptop*, which is the laptop building the image.
                let (own, repo) = crate::cmd::session::resolved(&paths)?;
                let sandbox = sandbox(&paths, &adapter, &repo)?;
                image::ensure_stack(
                    backend.program(),
                    &adapter,
                    &sandbox.recipe(),
                    // The sandbox's own reading, not a fresh one. `sandbox.tag`
                    // was computed from it, and two reads of a file can differ.
                    sandbox.ca.as_deref(),
                    &paths.repo,
                )?;
                if sandbox.tag != image::tag_for(&adapter, ca.as_deref()) {
                    summary.stack_image = Some(sandbox.tag.clone());
                }

                // And what that image turned out to contain, measured once and
                // remembered: every launch afterwards reads `~/.omh/facts.json`
                // rather than starting a container to ask again.
                //
                // Two readings of one probe. A `needs` that did not resolve is
                // a **provisioning failure** — the recipe ran and the
                // environment still does not work, which is exactly what
                // shipping rustup with no `cc` looked like. The same
                // measurements hold back a hook whose program is missing, which
                // is a different question about the same fact.
                let hook_dirs = Profile::resolve(&paths).sources(adapter::Capability::Hooks)?;
                let mut sandbox = sandbox;
                sandbox.top_up(
                    &paths,
                    backend.program(),
                    &adapter,
                    &hook_dirs,
                    &own,
                    &repo,
                    ctx,
                )?;
                for name in &sandbox.owed {
                    if sandbox.resolves.get(name) == Some(&false) {
                        summary
                            .problems
                            .push(format!("{name} did not resolve after installing"));
                    }
                }

                // And the other reading, through the launcher's own function so
                // `init` cannot report one thing and a launch do another.
                //
                // No question here any more, and that is the point of the whole
                // design. What stood here asked, for every program the sandbox
                // lacked, whether to switch its hook off — and recorded the
                // answer in a committed file. It was asking somebody to
                // configure around a broken environment, and the answer
                // outlived the breakage: a repo whose sandbox later gained
                // `cargo` still had `cargo = "skip"` on file, so the hook
                // stayed off for everybody who cloned it, with nothing to
                // re-ask. Now nothing is on file, because nothing had to be
                // decided.
                // **Only when the image was actually asked.** `measure`
                // reports and swallows a probe that will not run, which is
                // right for a launch — an unmeasured program suppresses
                // nothing, so no hook is dropped on a guess — and reaches this
                // report as an empty `resolves`, from which `render::held_back`
                // derives an empty list. Read as `Measured`, that is a clean
                // bill of health issued by a doctor who was out. It is the same
                // defect one level in from the one this value was added for,
                // and it is why the ask is asked about rather than assumed.
                hooks = match &sandbox.unmeasured {
                    Some(why) => report::Hooks::Unchecked(why.clone()),
                    None => report::Hooks::Measured(
                        render::held_back(&hook_dirs, &own, &repo, &sandbox.resolves)?
                            .iter()
                            .map(|d| (d.name.clone(), d.wanted.clone()))
                            .collect(),
                    ),
                };
            }
        }
    }
    // No harness is no image, and no image is no sandbox to ask about. The
    // hooks are already written either way.

    // Report every decision, so `omh why` has something to explain. Collected
    // here and printed once, which is `report::Init`'s own rule — the comment
    // that stood here claimed the opposite, and had since the report was
    // centralised.
    //
    // The headline is a claim about this run, so it has to be able to stop
    // being true. omh derives what it can and asks only what nothing could
    // derive; printing "asked nothing" after putting a question on screen would
    // make the promise the tagline is selling into a thing the user just
    // watched it break.
    //
    // Counted from what was actually *put*, not from what was answered — a
    // question declined was still a question asked, and claiming otherwise
    // would let omh interrogate somebody and then deny it.
    summary.asked = asked;
    summary.harness_on_host = harness.as_deref().is_some_and(runtime::installed);
    summary.harness = harness.clone();
    summary.stacks = stacks
        .iter()
        .map(|s| (s.name.clone(), s.marker.clone()))
        .collect();
    // Named, with the evidence, because the alternative is the failure this
    // whole design replaces: a hook that runs on turn one and reports
    // `cargo: not found`, saying nothing about who decided to run cargo or
    // where it looked.
    //
    // "will not run", and it is safe to say so now. This list comes from
    // `render::held_back`, which is the function the launcher itself uses — so
    // a hook named here is a hook the session will not ship, rather than one
    // omh hoped somebody would go and disable.
    //
    // The hook file stays where it is either way. `.omh/hooks/` is the repo's
    // statement about itself and it is committed; whether a program exists is
    // a fact about one image, and it decides what runs here, never what the
    // repo contains.
    summary.hooks = hooks;

    // Hooks somebody already has, somewhere omh can see them. **Noticed, never
    // acted on**: importing writes executable content into the repo, and doing
    // that because `init` happened to find a file is not a decision omh gets to
    // make on somebody's behalf. It says what is there and what would bring it
    // across.
    summary.importable = crate::cmd::catalogue::importable(&paths, &adapters);

    // What the repo already documents becomes notes that *point* at it.
    // Printing the seeds instead would derive them every run, show them once,
    // and keep them nowhere.
    summary.memory = match seed_store(&paths) {
        Ok(report) => report,
        // Never fatal. A repo that cannot be ingested is still a repo omh set
        // up, and failing `init` over the note store would be the tail
        // wagging the dog.
        Err(e) => format!("not seeded: {e:#}"),
    };

    summary.catalogue_dir = paths.root.display().to_string();
    summary.repo_dir = repo_omh.display().to_string();
    // The index lives in a container volume, so it has to be built inside the
    // sandbox — one built on the host would land where no session can read it.
    if let Some(h) = &harness {
        let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
            runtime::installed(p)
        })?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        let args = base::index_args(
            &image::tag_for(&adapter, ca.as_deref()),
            &paths.cache_volume(),
            &paths.repo,
            &paths.repo_name(),
        );
        match Command::new(backend.program())
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Backgrounded: init returns now and the first launch waits only if
            // this has not finished.
            Ok(_) => {
                summary.graph = Some(format!("indexing in background → {}", paths.cache_volume()))
            }
            Err(e) => summary.graph = Some(format!("could not start indexing: {e}")),
        }
    }

    summary.base_set = manifest.version.to_string();
    summary.rationale = manifest
        .rationale()
        .into_iter()
        .map(|(name, why)| (name.to_string(), why.to_string()))
        .collect();
    summary.next = next_after_init(harness.as_deref());
    // The selection as it stands now that `init` has written it, read the way
    // `omh info --repo` reads it. Built from the same policy rather than from
    // what this function happened to write, so the two commands cannot
    // disagree about what this repo takes — and so a re-run over a curated
    // list reports the curated list rather than the list it did not write.
    // **Reported and withdrawn, never fatal**, which is the rule every other
    // read in this function follows and the one these two arrived without.
    // They are the *report*, and `init` reaches here having already built two
    // images, written `settings.toml`, the gitignore, the memory notes and the
    // provisioning table. An unreadable `~/.omh/skills` — reachable on a first
    // `init`, because a template carrying `[use]` skips the earlier catalogue
    // read — turned all of that into exit 1 and a one-line permission error,
    // with nothing said about the work that had actually been done.
    //
    // The manifest is the one this function already holds. Loading a second
    // put two `Manifest` values in one `init`, with `base_set` reporting the
    // version of one and the selection resolved against the `owns()` map of
    // the other.
    match crate::cmd::settings::using_here(&paths, &manifest) {
        Ok(using) => summary.using = using,
        Err(e) => summary
            .problems
            .push(format!("what this repo takes could not be read ({e:#})")),
    }
    // The same advisory lines `omh info --repo` prints, from the same
    // function. `init`'s rows count the raw catalogue and these narrow to what
    // this repo could ever take; between them a `[use]` entry that answers to
    // nothing is named here rather than filtered out of both.
    match crate::cmd::settings::selection_notices(&paths, &manifest) {
        Ok(notices) => summary.notices = notices,
        Err(e) => summary
            .problems
            .push(format!("the selection could not be checked ({e:#})")),
    }

    ctx.say(&summary);
    Ok(())
}

/// Adapters ship with omh but live in `~/.omh`. Without this a fresh install
/// cannot launch anything, which is the state the tool was in until now.
pub(crate) fn install_bundled_adapters(paths: &Paths, ctx: &out::Ctx) -> Result<Vec<String>> {
    install_bundled(&paths.adapters(), bundled::Shipped::Adapters, ctx)?;
    Ok(Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect())
}

/// Put the two questions of last resort, and write down what comes back.
///
/// **A terminal is a precondition, not a fallback.** With stdin closed — a CI
/// runner, a script — nothing is asked and nothing is written, which is the
/// same outcome as declining and is reached without printing a prompt nobody
/// can answer. `ask::prompt` reads EOF as a stop for the same reason.
///
/// Returns how many questions were actually put, so `init`'s headline can stop
/// claiming it asked nothing the moment it did.
///
/// `write_if_absent`, so an answer somebody has since edited is never
/// overwritten by a later `init` re-asking and getting a different reply.
pub(crate) fn questions(
    repo_omh: &std::path::Path,
    unclaimed: &[&stack::Marker],
    has_test: bool,
    ctx: &out::Ctx,
) -> Result<(usize, Vec<String>)> {
    if unclaimed.is_empty() && has_test {
        return Ok((0, Vec::new()));
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Ok((0, Vec::new()));
    }

    let stdin = std::io::stdin();
    let (asked, answers) = ask_all(
        unclaimed,
        has_test,
        &mut stdin.lock(),
        &mut std::io::stderr(),
    )?;

    let mut hooks = Vec::new();
    for a in answers {
        let path = repo_omh.join(&a.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_if_absent(&path, &a.body)?;
        // Confirmed as it happens rather than saved for the summary: the user
        // is sitting at a prompt they just answered, and the answer to "what
        // did that do" is owed now, not forty lines later.
        ctx.progress(&a.said);
        // Handed back so `init` can put it in `[use]`. A hook written into a
        // repo whose selection is already curated is one `merge_hooks` drops,
        // so an answered question would produce a file, a report line, and a
        // session that never runs it.
        if a.path.starts_with("hooks") {
            if let Some(stem) = a.path.file_stem() {
                hooks.push(stem.to_string_lossy().into_owned());
            }
        }
    }
    Ok((asked, hooks))
}

/// The exchange itself, with the terminal handed in.
///
/// Split from [`questions`] so its rules can be asserted at all — how many
/// questions were put, what a decline does to the ones after it, and that a
/// declined question is still a question asked.
pub(crate) fn ask_all(
    unclaimed: &[&stack::Marker],
    has_test: bool,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> Result<(usize, Vec<ask::Answer>)> {
    let mut asked = 0usize;
    let mut answers = Vec::new();

    for marker in unclaimed {
        asked += 1;
        match ask::how_is_it_installed(marker, input, out)? {
            Some(a) => answers.push(a),
            // **Stop the marker questions, rather than working through them.**
            // A decline and a closed pipe arrive here identically, and the one
            // that matters is the pipe: a polyglot repo with three unclaimed
            // markers would otherwise print three questions into a void and
            // count them. One "no" is answer enough to stop asking about the
            // rest — and the test question below is still put, because it is
            // the one most repos reach.
            None => break,
        }
    }
    // Asked last, because it is the question most repos reach and the one most
    // worth answering — putting it after an exchange somebody has already
    // declined would waste it.
    if !has_test {
        asked += 1;
        if let Some(a) = ask::what_tests_it(input, out)? {
            answers.push(a);
        }
    }
    Ok((asked, answers))
}

/// Copy definitions that ship with omh into `~/.omh`.
///
/// Bundled files are **managed**: they are refreshed on every `init`, because a
/// fix omh ships has to reach people who already ran it once. The one that
/// mattered was a wrong credential path, which made `omh auth` capture nothing
/// while reporting success. Definitions you add yourself are left alone.
///
/// The contents come from [`bundled`], embedded at compile time. Reading them
/// from the source tree instead is what made a released binary install nothing
/// at all — and say nothing, because the `read_dir` error was discarded.
pub(crate) fn install_bundled(
    dest: &std::path::Path,
    kind: bundled::Shipped,
    ctx: &out::Ctx,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("creating {} for the bundled {}", dest.display(), kind.dir()))?;
    for &bundled::File { name, contents } in kind.files() {
        let target = dest.join(name);

        // Bytes, not text. `read_to_string` fails on a single non-UTF-8 byte,
        // and treating that failure as "no file here" overwrote the file
        // without the backup promised below — the read failed, the write
        // succeeded, and somebody's edit was gone. Only "not found" means
        // absent; every other error is reported rather than assumed benign.
        let existing = match std::fs::read(&target) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", target.display())),
        };

        if !existing.is_empty() && existing != contents.as_bytes() {
            // Managed files are refreshed so shipped fixes land, but
            // silently discarding an edit is not acceptable.
            //
            // **Appended, never `with_extension`.** That replaces the
            // extension, so it produced the right name only while everything
            // omh shipped was TOML: an edited `rust-test.json` was saved as
            // `rust-test.toml.yours` while the line below said
            // `rust-test.json.yours`, and somebody looking where omh told them
            // to look would conclude their edit had been thrown away.
            let backup = target.with_file_name(format!("{name}.yours"));
            std::fs::write(&backup, &existing)
                .with_context(|| format!("saving your {name} as {}", backup.display()))?;
            // stderr: this is a warning about data, and stdout is the report.
            ctx.warn(&format!(
                "replaced {} — yours saved as {name}.yours",
                target.display()
            ));
        }
        std::fs::write(&target, contents)
            .with_context(|| format!("writing {}", target.display()))?;
    }

    // Not `.flatten()`. An unreadable entry here would be dropped from the
    // list omh then prints as `harnesses N (...)` and hands to
    // `detect::preferred_harness` — under-reporting and choosing from an
    // incomplete set, silently. That is the shape of bug this file just
    // finished removing.
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dest).with_context(|| format!("reading {}", dest.display()))? {
        let path = entry
            .with_context(|| format!("listing {}", dest.display()))?
            .path();
        if path.extension().is_some_and(|x| x == "toml") {
            names.push(path.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Append a line if absent. Rewriting the file would eat anything you added.
pub(crate) fn ensure_line(path: &std::path::Path, line: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    std::fs::write(path, out)?;
    Ok(())
}

pub(crate) fn write_if_absent(path: &std::path::Path, contents: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(path, contents)?;
    Ok(true)
}

/// Which provides applied, from what the predicates answered.
///
/// `None` when nothing was answered — the container never ran, the runtime
/// hiccuped, the image was missing. That case is not "nothing applies", and the
/// difference is destructive rather than academic: `stack::reconcile` drops
/// every `true` it is not told about, so recording an empty answer would erase
/// a repo's resolution and leave the next launch provisioning nothing.
///
/// A provide that could not answer is simply absent from the set, which is the
/// safe direction — it is not installed, so it is not recorded, so its `needs`
/// are not claimed and nothing reports a gap omh invented. Installing on a
/// coin-flip would be silent either way.
///
/// `asked` is how many provides there were to ask about, and it separates two
/// things an empty report cannot: **nothing to ask** is an answer, **nothing
/// answered** is silence. A repo that stops being a stack has no candidates and
/// runs no container, and that has to clear the resolution rather than preserve
/// it — otherwise `[provision]` keeps asserting `rust/toolchain = true` after
/// the `Cargo.toml` is gone, and the stack layer keeps installing a toolchain
/// nothing uses.
pub(crate) fn fired_from(asked: usize, answered: &[doctor::Outcome]) -> Option<BTreeSet<String>> {
    if asked == 0 {
        return Some(BTreeSet::new());
    }
    // One line per provide, so fewer lines than provides is a report that did
    // not finish — not a report saying "no". Accepting the prefix would make
    // `reconcile` drop every `true` it was not told about and rewrite a
    // committed file without them. The now-deleted `[toolchain]` question had
    // this same shape and had to be fixed for it, where it only cost a spurious
    // question; here it deletes.
    if answered.len() != asked {
        return None;
    }
    Some(
        answered
            .iter()
            .filter(|o| stack::verdict(o) == stack::Verdict::Applies)
            .map(|o| o.name.clone())
            .collect(),
    )
}

/// Write what fired into the repo's **shared**, committed settings, and hand
/// back what the file now says.
///
/// A function rather than four lines inline, because the layer it names on both
/// sides is the whole of its correctness and inline it is reachable only
/// through a container. Both halves are load-bearing in opposite directions:
///
/// - **Read `Shared`.** `reconcile` writes what it is given, so reading the
///   merge would take a `false` from `settings.local.toml` — one laptop's *not
///   here* — and commit it for everybody who clones.
/// - **Write `Shared`.** The resolution is the repo's, and a teammate cloning
///   it is the reason it lives in a committed file at all. Written to `Local`
///   it would be re-derived, and re-asked, on every machine.
pub(crate) fn record_resolution(
    paths: &Paths,
    fired: &BTreeSet<String>,
) -> Result<BTreeMap<String, bool>> {
    let recorded = stack::reconcile(
        &config::read_provision(paths, config::Layer::Shared)?,
        fired,
    );
    config::write_provision(paths, config::Layer::Shared, &recorded)?;
    Ok(recorded)
}

/// The recipes to run, in the order the stack files gave them.
///
/// File order is install order — `corepack enable pnpm` needs the node the
/// provide above it asserted — so this walks the definitions rather than the
/// resolution, which is a map sorted by name and would silently reorder them.
///
/// A provide with no `install` contributes nothing: it asserts the base image
/// already ships something, so it changes neither the recipe nor the tag.
///
/// **`resolved` is the only input**, and that is the point. It is the
/// `[provision]` table as all three settings layers resolve it: `init` writes
/// what its predicates found, a person may write `false` to opt out, and every
/// launch afterwards reads the same table. A launch that re-derived this from
/// anything else — the predicates it cannot run, a set of provides that
/// "fired" — would build a different image from the one `init` reported, and
/// the disagreement would be invisible because both are plausible.
///
/// Only `true` provisions. Absent is not `false` and does not need to be: an
/// entry nobody recorded is one no predicate has said applies here.
pub(crate) fn installs_for<'a>(
    detected: &[&'a stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> Vec<&'a str> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .filter_map(|(_, p)| p.install.as_deref())
        .collect()
}

/// What the stacks this repo provisions said must resolve once they had run.
///
/// Only provides the resolution recorded `true`. A provide nobody recorded was
/// never installed, and a provide somebody opted out of was deliberately not
/// installed — reporting either as a failure would be a gap omh invented. The
/// consequence of an opt-out is not silenced by that: if a hook names the
/// program, it is probed anyway through `render::hook_programs`, and a hook
/// that cannot run is dropped by name.
///
/// This includes provides with **no `install`**, which is the point of letting
/// them exist: `stacks/node.toml`'s `runtime` asserts the base image already
/// ships `node` and `npm`, and the only way that assertion is worth writing is
/// if something checks it.
pub(crate) fn needs_of(
    detected: &[&stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> BTreeSet<String> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .flat_map(|(_, p)| p.needs.iter().cloned())
        .collect()
}

/// Everything worth asking one image about: what the stacks promised, and what
/// the hooks will actually run.
///
/// **The union, and it has to be.** The two lists answer different questions
/// and neither contains the other. A stack's `needs` is what provisioning owes
/// — the reading that catches rustup installing a `cargo` that cannot link.
/// A hook's program is what will be handed to a shell — and a hand-written
/// `shellcheck` hook is in no `needs` list, so a probe built from `needs` alone
/// ships it into a sandbox that cannot run it. That is the original
/// `cargo: not found` with a different program in it.
pub(crate) fn probe_targets(
    hook_dirs: &[PathBuf],
    own: &base::Own,
    repo: &settings::RepoPolicy,
    owed: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut wanted = render::hook_programs(hook_dirs, own, repo)?;
    wanted.extend(owed.iter().cloned());
    Ok(wanted)
}

/// Ask the image about the programs nobody has asked it about yet, remember
/// the answers, and hand back everything known about it.
///
/// The cache is the reason a launch is not a container run: `Facts::unseen`
/// narrows the question to what has never been answered for this tag, and a
/// repo whose hooks and stacks have not changed asks nothing at all.
///
/// Never fatal. A runtime that will not start, an image that is not there, a
/// probe that says nothing — all of them leave the facts as they were, which
/// reads as *nobody has looked* and suppresses nothing. The alternative is a
/// diagnostic failure taking a launch down with it.
/// What a probe run amounts to: what it measured, or **why nobody could be
/// asked**.
///
/// Split out of `measure` because the reason is the whole value of the guard,
/// and a reason that only exists as an `eprintln!` inside a function that shells
/// out is a reason no test can see disappear.
///
/// A container that ran and **failed** is not an answer. Checking only the
/// `Err` arm — a runtime that would not start — was the shape `init`'s
/// predicate call already had to be fixed for: `docker run` failing because the
/// image is gone, the daemon is refusing, or the disk is full exits non-zero
/// with empty stdout, which parses to no outcomes and reads as *nothing was
/// measured*. Unmeasured suppresses nothing, so the direction is safe; the
/// silence is not. Without a reason the user gets a session with every hook
/// shipped into a sandbox nobody could ask about, and nothing said.
///
/// Stderr is trimmed to three lines. A runtime failing to pull or mount can
/// produce a page of it, and a diagnostic that buries the line above it in its
/// own output is one people learn to scroll past.
pub(crate) fn measured_or_reason(
    ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Vec<doctor::Outcome>, String> {
    if !ok {
        let mut reason = String::from("could not ask the sandbox what it has");
        for line in stderr.lines().filter(|l| !l.trim().is_empty()).take(3) {
            reason.push_str("\n     ");
            reason.push_str(line);
        }
        return Err(reason);
    }
    Ok(doctor::parse(stdout))
}

/// What an image was found to have — and whether it was actually asked.
///
/// The two together, because apart they are indistinguishable at the far end:
/// a probe that could not run leaves the facts as they were, `has` comes back
/// as *nobody has looked*, and `render::held_back` reads an unmeasured program
/// as *not blocked*. For a launch that is right and deliberate — an
/// unmeasured program suppresses nothing, so nothing is dropped on a guess.
/// For `init`'s report it is a clean bill of health issued by a doctor who was
/// out, which is the exact sentence that report was rewritten to stop printing.
pub(crate) struct Measured {
    pub(crate) has: BTreeMap<String, bool>,
    /// Why the ask did not happen. `None` means these answers are answers.
    pub(crate) gave_up: Option<String>,
}

pub(crate) fn measure(
    program: &str,
    paths: &Paths,
    tag: &str,
    wanted: &BTreeSet<String>,
    ctx: &out::Ctx,
) -> Result<Measured> {
    let mut facts = facts::Facts::load(paths);
    let unseen = facts.unseen(tag, wanted);
    if !unseen.is_empty() {
        let borrowed: Vec<&str> = unseen.iter().map(String::as_str).collect();
        let ran = Command::new(program)
            .args(image::probe_args(tag, &doctor::probe_programs(&borrowed)))
            .output();
        let outcomes = match ran {
            Ok(out) => measured_or_reason(
                out.status.success(),
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(e) => Err(format!("could not ask the sandbox what it has ({e})")),
        };
        let outcomes = match outcomes {
            Ok(outcomes) => outcomes,
            Err(reason) => {
                ctx.warn(&reason);
                // Still not fatal — see below — but no longer indistinguishable
                // from an image that was asked and had nothing.
                return Ok(Measured {
                    has: facts.about(tag),
                    gave_up: Some(reason.lines().next().unwrap_or_default().to_string()),
                });
            }
        };
        if !outcomes.is_empty() {
            facts.learn(tag, &outcomes);
            // Reported and swallowed, never fatal. This is a cache beside the
            // catalogue; a read-only home, a full disk or a `facts.json`
            // somebody replaced with a directory would otherwise abort every
            // `omh new`, `omh sNN resume`, `omh s attach`, `omh doctor` and `omh
            // init` on the machine — every caller of `top_up`, which is all of
            // them — a launch
            // killed by a file whose entire design premise is that losing it
            // degrades to "nobody has looked". `Facts::load` already treats the
            // read side this way and says why.
            if let Err(e) = facts.save(paths) {
                ctx.warn(&format!(
                    "measurements not cached ({e:#}) — the sandbox is asked again next time"
                ));
            }
        }
    }
    Ok(Measured {
        has: facts.about(tag),
        gave_up: None,
    })
}

/// What this repo's sandbox is: the recipe its stacks provision, the image that
/// recipe produces, and what that image has been measured to contain.
///
/// The four fields are **one answer**, and holding them together is what makes
/// a mismatch hard to write: `tag` is derived from `installs`, `resolves` is
/// keyed on `tag`, and `owed` is what `installs` promised. Nothing outside
/// [`sandbox`] constructs one.
pub(crate) struct Sandbox {
    /// Owned, because the definitions they are read from do not outlive this.
    pub(crate) installs: Vec<String>,
    pub(crate) tag: String,
    pub(crate) resolves: BTreeMap<String, bool>,
    /// What the provides this repo installed said must resolve once they had.
    /// Carried here rather than re-derived, so the caller that tops the
    /// measurements up asks about the same list `init` reported on.
    pub(crate) owed: BTreeSet<String>,
    /// Why `resolves` is not an answer, when it is not.
    ///
    /// `None` until `top_up` has asked. A launch does not read this — an
    /// unmeasured program suppresses nothing either way — but a report that
    /// says what is held back has to be able to say *nothing was asked*, and
    /// `resolves` alone cannot: an empty map is what both outcomes look like.
    pub(crate) unmeasured: Option<String>,
    /// The corporate root `tag` was computed with, for the same reason
    /// `installs` is carried: the layer a launch builds and the tag it runs
    /// must come from one resolution. `session_up` read the setting a second
    /// time, which is the split its own doc comment warns about — a second read
    /// can differ from the first, and then omh builds one image and names
    /// another.
    pub(crate) ca: Option<String>,
}

impl Sandbox {
    pub(crate) fn recipe(&self) -> Vec<&str> {
        self.installs.iter().map(String::as_str).collect()
    }

    /// Ask this image about anything nobody has asked it yet, and keep the
    /// answers.
    ///
    /// Launch does this too, not only `init`. A hook added after the last
    /// `init` names a program no measurement covers, and an unmeasured program
    /// suppresses nothing — so without this the hook ships into a sandbox that
    /// may not have it and fails at turn one with `not found`, which is the
    /// failure this whole design starts from. The cache is what makes it
    /// affordable: a repo whose hooks and stacks have not changed asks nothing
    /// and starts no container.
    ///
    /// **Builds the image first**, and that ordering is the method's reason for
    /// existing rather than a detail inside it.
    ///
    /// `init` had it right and all three launch paths had it backwards: they
    /// measured, then built inside `session_up`. So the first launch after a
    /// recipe changed — a `[provision]` opt-out, a fresh clone of a repo whose
    /// resolution is committed, anything after `docker image prune` — probed a
    /// tag with no image behind it, learned nothing, and shipped every hook
    /// unsuppressed into a sandbox that did not have their programs. It healed
    /// on the *second* launch, which is precisely the broken-first-turn this
    /// design exists to remove.
    ///
    /// Fixing it in three call sites would have left the fourth caller to get
    /// it right. Here it cannot be got wrong: asking an image a question and
    /// making sure there is an image to ask are one operation.
    ///
    /// Failures inside `measure` are reported and swallowed — a runtime that
    /// will not start leaves the facts as they were, which reads as *nobody has
    /// looked* and suppresses nothing. The build is **not** swallowed: an image
    /// that will not build is the session, not a diagnostic about it.
    //
    // Eight arguments, one over clippy's default. Every one is a distinct
    // input this cannot derive — the paths, the runtime, the adapter, the hook
    // directories, both halves of the resolved settings, and where to report.
    // Bundling them into a struct only to unpack it here would move the list
    // rather than shorten it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn top_up(
        &mut self,
        paths: &Paths,
        program: &str,
        adapter: &Adapter,
        hook_dirs: &[PathBuf],
        own: &base::Own,
        repo: &settings::RepoPolicy,
        ctx: &out::Ctx,
    ) -> Result<()> {
        let recipe: Vec<String> = self.installs.clone();
        // The reading `self.tag` was computed from. This used to resolve the
        // setting again here, with a comment arguing that a parameter is one
        // more place to forget — but the value was already on `self`, and a
        // second read of a file that has moved builds a layer the tag beside
        // it does not name.
        let ca = self.ca.as_deref();
        image::ensure_stack(
            program,
            adapter,
            &recipe.iter().map(String::as_str).collect::<Vec<_>>(),
            ca,
            &paths.repo,
        )?;
        let wanted = probe_targets(hook_dirs, own, repo, &self.owed)?;
        let measured = measure(program, paths, &self.tag, &wanted, ctx)?;
        self.resolves = measured.has;
        self.unmeasured = measured.gave_up;
        Ok(())
    }
}

/// Work out which image this repo runs, and what is already known about it.
///
/// **One function, because these are one answer.** For the whole of the first
/// milestone `init` built a stack layer and `container::plan` hardcoded
/// `image::tag_for(adapter)`, so the layer was built by one command and run by
/// none — and nothing was wrong with either half on its own. Two places
/// deciding which image a session runs is the shape of that bug, so there is
/// one place, and the measurements come back keyed on the tag it returned.
///
/// Fatal when the stacks will not load, which is the opposite of `say_hooks`'
/// answer to the same directory and is right for the opposite reason. There, an
/// unreadable directory costs a report. Here it decides *which sandbox you get*:
/// falling back to the harness image would launch a session with no toolchain
/// in it, silently, which is the failure this whole design starts from.
///
/// Reads the cache but never the container. Asking the image anything is
/// [`Sandbox::top_up`], which builds it first — so this stays cheap enough to
/// call on every launch path before anything has been decided.
pub(crate) fn sandbox(
    paths: &Paths,
    adapter: &Adapter,
    repo: &settings::RepoPolicy,
) -> Result<Sandbox> {
    let defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected = stack::detected(&defs, &paths.repo);
    let installs: Vec<String> = installs_for(&detected, &repo.provision)
        .into_iter()
        .map(str::to_string)
        .collect();
    // Once, and carried. Every later question about this image — the tag, the
    // layer that gets built, the digest a note pins — is answered from this
    // one read, so a PEM edited between two of them cannot produce two
    // different images.
    let ca = image::ca_for(paths)?;
    let tag = image::stack_tag(
        adapter,
        &installs.iter().map(String::as_str).collect::<Vec<_>>(),
        ca.as_deref(),
    );
    let resolves = facts::Facts::load(paths).about(&tag);
    let owed = needs_of(&detected, &repo.provision);
    Ok(Sandbox {
        installs,
        tag,
        resolves,
        owed,
        // Nothing has been asked yet — this reads the cache and never the
        // container. `top_up` is what turns it into an answer.
        unmeasured: Some("the sandbox has not been asked".into()),
        ca,
    })
}
