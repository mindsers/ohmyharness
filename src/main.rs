//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh new claude      omh new opencode      omh new codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod ask;
mod auth;
mod base;
mod bundled;
mod carry;
mod cli;
mod cmd;
mod config;
mod container;
mod derive;
mod detect;
mod doctor;
mod editor;
mod facts;
mod hook;
mod idle;
mod image;
mod key;
mod mcp;
mod memory;
mod notice;
mod out;
mod persist;
mod profile;
mod render;
mod report;
mod rules;
mod runtime;
mod selection;
mod session;
mod settings;
mod shadow;
mod ssh;
mod stack;
#[cfg(test)]
mod testsrc;
mod why;

use adapter::Adapter;
use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use cli::{Cli, Cmd, MemoryCmd, SessionsCmd, SettingsCmd};
use profile::Paths;

fn main() -> std::process::ExitCode {
    // A closed pipe (`omh info | head`) is not a crash. Without this, Rust's
    // default panics on the failed write and prints a backtrace.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    // One sink for every failure, including the ones that happen before there
    // is a `Cli` to ask about colour — `no_command_writes_to_a_stream_behind_
    // the_output_layer` allows exactly one, and the exemption is this renderer
    // rather than whichever error happened to need it first.
    //
    // The palette starts plain and is replaced the moment the parse resolves
    // one, so a refusal is painted the way every other refusal is.
    let mut palette = out::Palette::plain();
    let outcome = (|| -> Result<()> {
        // The session is lifted out of the command line before clap sees it,
        // so every command below reads it from one place.
        let (named, argv) = cli::session_prefix(std::env::args().collect());
        let mut cli = match Cli::try_parse_from(&argv) {
            Ok(cli) => cli,
            // A line clap cannot read might be one this release renamed. Asked
            // here rather than through a variant, so nothing retired parses.
            Err(e) => match cli::retired(&argv) {
                Some(said) => anyhow::bail!("{said}"),
                None => e.exit(),
            },
        };
        cli.session = cli::the_one_session(named, cli.session.take())?;
        let (format, resolved) = cli.output();
        palette = resolved;
        dispatch(
            &cli,
            &out::Ctx {
                format,
                palette: resolved,
            },
        )
    })();

    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprint!("{}", out::problem(&palette, &e));
            std::process::ExitCode::FAILURE
        }
    }
}

/// Say when the file 0.7.0 renamed is still sitting there unread.
///
/// **Every command, not one.** It lived in `show_settings`, which reaches only
/// people who already know a command was added — while launch, `repo`,
/// `doctor`, `why` and `init` all stayed silent, and `init` is the one moment
/// the template is supposed to matter. Somebody upgrades, their defaults stop
/// applying everywhere at once, and nothing says why: the failure this project
/// keeps writing down, at the scale of every existing user.
///
/// Cheap enough to run unconditionally — one `exists()` on a path already
/// computed — and on stderr, so a redirected pipeline still shows it.
fn say_if_the_template_was_renamed(cwd: &std::path::Path, ctx: &out::Ctx) {
    let Ok(root) = Paths::home() else { return };
    let old = root.join("settings.toml");
    if !old.exists() {
        return;
    }
    let _ = cwd;
    let now = root.join(config::TEMPLATE);
    // `mv` only when there is nothing to overwrite. Advising it unconditionally
    // destroyed a populated `default.toml` — omh printing the command that
    // loses somebody's configuration is worse than omh losing it, because they
    // typed it themselves and have no reason to suspect the tool.
    let next = if now.exists() {
        format!(
            "  both files exist. {} is the one omh reads; merge anything you \
             still want out of {} and delete it",
            now.display(),
            old.display()
        )
    } else {
        format!("  mv {} {}", old.display(), now.display())
    };
    ctx.warn(&format!(
        "{} is not read any more — it became {}, the template a new repo is \
         seeded from.\n{next}",
        old.display(),
        config::TEMPLATE,
    ));
}

/// The sentence for state under the old key that nothing will read again.
///
/// One function because two arms say it: a run where nothing could move, and
/// a run where something else did. The second used to say nothing at all.
fn say_what_was_stranded(paths: &Paths, from: &str, kinds: &[String]) -> String {
    format!(
        "this checkout's {} under `{from}` are from before omh keyed them by checkout, \
         and it already has newer ones — so nothing reads them now. omh will not merge \
         two sets of sessions together; they are in {}, to keep or delete by hand",
        kinds.join(", "),
        paths.root.display()
    )
}

/// Move this checkout's state off the pre-2026.08 key, and say so.
///
/// Runs before every command that is not a preview, because there is no
/// natural moment for it: the state it rescues is read by `omh s`, by a
/// launch, by `omh memory` and by `why`, and a migration wired into one of
/// them leaves the others reading an empty directory. It is a handful of
/// `exists()` calls once the move has happened, which is the ordinary case
/// for ever after.
///
/// **Never fatal.** A checkout that is not a git repository has no paths to
/// migrate and no business failing `omh settings` over it, and a refusal is
/// something the user must decide about rather than something that should
/// stop the command they typed. Both are reported and stepped over — though a
/// refusal is a warning, because the state it names stays invisible to every
/// command until it is dealt with.
fn say_what_moved_off_the_old_key(cwd: &std::path::Path, ctx: &out::Ctx) {
    let Ok(paths) = Paths::discover(cwd) else {
        return;
    };
    // Sessions under the *old* key, since those are what would be moved. A
    // runtime that cannot be asked counts as **running**: this is the check
    // that stops a live container's mounts being renamed underneath it, and
    // "cannot tell" must not spell the same as "no".
    //
    // Both halves of that, and the first version only had one. The inner
    // `!matches!(…, Running::No)` was right — `Running::Unknown` counts as
    // running. But the backend itself was resolved with `.ok()` and read
    // through `is_some_and`, so *no runtime at all* — Docker's CLI missing
    // from a GUI-launched shell's PATH, a typo'd `runtime =` — came back
    // `false`, meaning not running, and the rename went ahead over live
    // mounts. `is_none_or` is the fix, and it is one word.
    //
    // Resolved **inside** the closure, which is only called when `migrate`
    // has something pending. `runtime::select` shells out to `command -v`
    // once per candidate, and `auto` has two — so eagerly, every `omh info`
    // and `omh why` forked two shells to answer a question almost no run asks.
    let running = |legacy: &str| {
        let dir = paths.root.join("worktrees").join(legacy);
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)).ok();
        session::list(&dir).into_iter().any(|id| {
            backend.as_ref().is_none_or(|b| {
                !matches!(
                    image::container_running(b, &format!("omh-{legacy}-{id}")),
                    image::Running::No
                )
            })
        })
    };

    match profile::migrate(&paths, &running) {
        Ok(profile::Migration::NothingToDo) => {}
        Ok(profile::Migration::Moved {
            from,
            kinds,
            stranded,
        }) => {
            // `warn`, not `progress`. `progress` is suppressed under `--json`
            // — correctly, for a launcher narrating a routine step — and this
            // renamed six directories of the user's state, once, irreversibly.
            // It was the only arm of this match that a scripted consumer
            // could not see.
            ctx.warn(&format!(
                "moved this checkout's {} off `{from}` — keyed by checkout now, so two \
                 projects of the same name no longer share them",
                kinds.join(", ")
            ));
            if !stranded.is_empty() {
                ctx.warn(&say_what_was_stranded(&paths, &from, &stranded));
            }
        }
        Ok(profile::Migration::Stranded { from, kinds }) => {
            ctx.warn(&say_what_was_stranded(&paths, &from, &kinds))
        }
        Ok(profile::Migration::Refused(why)) => ctx.warn(&why),
        Err(e) => ctx.warn(&format!(
            "omh could not move this checkout's state off its old location, so \
             anything under it is invisible to omh until this is resolved: {e}"
        )),
    }
}

fn dispatch(cli: &Cli, ctx: &out::Ctx) -> Result<()> {
    let cwd = std::env::current_dir()?;
    say_if_the_template_was_renamed(&cwd, ctx);

    // Before anything reads it. A scope omh cannot honour is refused where it
    // was named, rather than at whatever depth the handler would have ignored
    // it — and refusing is the safe half: a command taught to read one later
    // turns this into an answer, while a command that silently dropped one
    // never announces that it started mattering.
    if let Some(id) = cli.session.as_deref() {
        anyhow::ensure!(
            cli::consumes_session(&cli.cmd),
            "`{id}` names a session, and this command does not act on one:\n  omh <command>    without the session"
        );
    }

    // Same shape, same reason. A flag this command cannot honour is refused
    // where it was typed, rather than accepted and dropped at whatever depth
    // stopped reading it — which is what made `omh --dry-run use --all` write
    // the file and then say `wrote →`.
    anyhow::ensure!(
        !cli.dry_run || cli::previews(&cli.cmd),
        "`--dry-run` is not something this command can answer yet:\n  \
         omh <command>    to run it"
    );

    // And the third flag of that shape. A command that hands you a program
    // has no answer for `--json` to carry, so it was accepted and produced
    // nothing — which a script reads as an empty answer rather than a
    // mistake. A preview is an answer, so `--dry-run` lifts the refusal.
    anyhow::ensure!(
        !cli.json || cli.dry_run || cli::answers_json(&cli.cmd),
        "`--json` is not something this command can answer: it hands you a program \
         and prints nothing to parse:\n  \
         omh <command>    without --json"
    );

    // Before any command reads per-repo state, and after `--dry-run` has been
    // settled — a preview writes nothing, and a migration is a write.
    if !cli.dry_run {
        say_what_moved_off_the_old_key(&cwd, ctx);
    }

    match &cli.cmd {
        Cmd::Init => cmd::init::init(&cwd, ctx),
        Cmd::Auth { harness, account } => cmd::auth::auth_cmd(&cwd, harness, account, ctx),
        Cmd::Info { repo } => {
            if *repo {
                cmd::settings::show_repo(&cwd, ctx)
            } else {
                cmd::inspect::info(&cwd, ctx)
            }
        }
        Cmd::Eject { harness, to } => cmd::eject::eject(&cwd, harness, to, cli.dry_run, ctx),
        Cmd::Prune {
            dangerously_include_unsafe,
        } => cmd::prune::prune_cmd(
            &cwd,
            cli.dry_run,
            *dangerously_include_unsafe,
            cmd::harvest::Interactive::of_stdin(),
            ctx,
            &mut std::io::stdin().lock(),
            &mut std::io::stderr(),
        ),
        Cmd::Doctor { harness } => {
            cmd::inspect::doctor_cmd(&cwd, harness.as_deref(), cli.dry_run, ctx)
        }
        Cmd::Why { thing } => cmd::inspect::why_cmd(&cwd, thing, ctx),
        Cmd::Graph { stop } => cmd::inspect::graph(&cwd, *stop, ctx),

        // No verb: the listing. With a session named, the same listing scoped
        // to it — the prefix means *this one* everywhere else, and this is the
        // last place it did not.
        Cmd::Sessions { cmd: None } => cmd::session::sessions_ls(&cwd, cli.session.as_deref(), ctx),
        Cmd::Sessions { cmd: Some(cmd) } => match cmd {
            // One source for which session a command acts on, now that the
            // prefix and `--session` both land in `cli.session`. `rm` used to
            // require its own positional and `diff` accepted either, which is
            // how the same question came to have two answers.
            SessionsCmd::Rm { force } => {
                let id = cli.session.as_deref().context(
                    "which session? name it first:\n  omh s01 rm\n  omh s      lists them",
                )?;
                cmd::session::rm(
                    &cwd,
                    id,
                    // Wrapped where the two bools are born, so no frame
                    // carries them side by side. Moving `Consent` to
                    // `may_remove` only pushed the swap up to `rm`; this is
                    // the boundary it was pushed to.
                    cmd::harvest::Consent::read(
                        cmd::harvest::Forced(*force),
                        cmd::harvest::Interactive::of_stdin(),
                    ),
                    ctx,
                )
            }
            SessionsCmd::Attach { editor } => {
                cmd::session::attach(&cwd, cli.session.as_deref(), editor.as_deref(), ctx)
            }
            SessionsCmd::Resume { harness, args } => {
                let paths = Paths::discover(&cwd)?;
                let session = cmd::harvest::existing_session(&paths, cli.session.as_deref())?;
                // Where the name came from is kept, not flattened. It decides
                // both what may be said about it and whether it has been
                // checked: a name off the command line is the user's word and
                // `Adapter::find` judges it, a name off the marker is omh's own
                // record and `harness_of` has already refused anything that is
                // not a harness name.
                let (harness, from_record) = match harness {
                    Some(named) => {
                        // Naming a harness the session did not run is a switch,
                        // not a resume: an image is built per harness, so the
                        // sandbox stops and starts on the other one, and the
                        // record is rewritten so every later `resume` follows.
                        // Both are wanted — running two harnesses against one
                        // session is a feature — but the word says *rejoin*, so
                        // the other meaning has to be said rather than assumed.
                        if let session::Ran::Harness(before) =
                            session::harness_of(&paths.runs(), &session.id)
                        {
                            if before != *named {
                                ctx.warn(&format!(
                                    "{} was running {}; resuming as {named} \
                                     restarts its sandbox and records {named}",
                                    session.id,
                                    out::untrusted(&before)
                                ));
                            }
                        }
                        (named.clone(), false)
                    }
                    None => match session::harness_of(&paths.runs(), &session.id) {
                        session::Ran::Harness(name) => (name, true),
                        // Refused, never guessed. `detect::preferred_harness`
                        // would answer for a session it knows nothing about,
                        // and the answer would be indistinguishable from a
                        // right one — claude attached to a worktree an
                        // afternoon of opencode built. Every session made
                        // before this release is here, and so is every one
                        // `omh s attach` created for an editor.
                        session::Ran::NeverRecorded => anyhow::bail!(
                            "omh has no record of a harness running in {id}, so \
                             it cannot rejoin as one.\n  \
                             omh {id} resume <harness>   rejoin it as that\n  \
                             omh s                       what is here",
                            id = session.id
                        ),
                        session::Ran::CouldNotTell(why) => anyhow::bail!(
                            "omh recorded a harness for {id} and cannot read it \
                             back: {why}\n  \
                             omh {id} resume <harness>   rejoin it as that, \
                             which rewrites the record",
                            id = session.id
                        ),
                    },
                };
                let mut argv = vec![harness.clone()];
                argv.extend(args.iter().cloned());
                let launched = cmd::session::run(
                    &cwd,
                    &argv,
                    session::Start::Named(&session.id),
                    cli.dry_run,
                    ctx,
                );
                // Said only when it is true. The record is what omh knows and
                // the user does not, so a failure needs the provenance; a name
                // they typed a second ago does not, and claiming omh recorded
                // it is a statement about history omh cannot support — the
                // inverse of the mistake this context was added to fix.
                if from_record {
                    launched.with_context(|| {
                        format!("{} recorded `{}`", session.id, out::untrusted(&harness))
                    })
                } else {
                    launched
                }
            }
            SessionsCmd::Down { all } => cmd::session::down(
                &cwd,
                cli.session.as_deref(),
                *all,
                std::io::IsTerminal::is_terminal(&std::io::stdin()),
                ctx,
            ),
            SessionsCmd::Sync { base, down } => {
                cmd::harvest::sync(&cwd, cli.session.as_deref(), base.as_deref(), *down, ctx)
            }
            SessionsCmd::Log { turns } => {
                cmd::harvest::log_cmd(&cwd, cli.session.as_deref(), *turns, ctx)
            }
            SessionsCmd::Diff {
                checkpoint,
                base,
                patch,
            } => cmd::harvest::diff(
                &cwd,
                cli.session.as_deref(),
                *checkpoint,
                base.as_deref(),
                *patch,
                ctx,
            ),
            SessionsCmd::Commit {
                message,
                skip_carried,
                keep,
                edit,
                force,
            } => cmd::harvest::commit(
                &cwd,
                cli.session.as_deref(),
                match keep.as_deref() {
                    Some(selection) => cmd::harvest::Landing::Keep {
                        selection,
                        edit: *edit,
                    },
                    None => cmd::harvest::Landing::Squash(message.as_deref()),
                },
                *skip_carried,
                *force,
                ctx,
            ),
            SessionsCmd::Push { name, pr } => {
                cmd::harvest::push(&cwd, cli.session.as_deref(), name.as_deref(), *pr, ctx)
            }
        },

        // Outside a repo too. `Paths::discover` refuses there, correctly — a
        // session is a worktree — but this command's whole subject is the file
        // you configure *before* a repo exists, and its own docs say so. The
        // refusal reasoned about worktree branches to somebody setting a
        // default in their home directory.
        Cmd::Settings { cmd } => {
            let paths = Paths::anywhere(&cwd)?;
            match cmd {
                None => cmd::settings::show_settings(&paths, ctx),
                Some(SettingsCmd::Set { key, value }) => {
                    cmd::settings::no_legacy_write_over_a_name_omh_owns(&paths, key, ctx)?;
                    // Both doors, for the reason the guard above exists: a
                    // check on one spelling of a write is a check somebody
                    // routes around by typing the other.
                    if key == "account" {
                        cmd::settings::no_account_that_no_login_answers_to(&paths, value, ctx)?;
                    }
                    cmd::settings::set(
                        &paths,
                        key,
                        value,
                        cmd::settings::Reach::named(config::Layer::Personal),
                        cli.dry_run,
                        ctx,
                    )
                }
                Some(SettingsCmd::Unset { key }) => {
                    cmd::settings::no_legacy_write_over_a_name_omh_owns(&paths, key, ctx)?;
                    cmd::settings::unset(
                        &paths,
                        key,
                        cmd::settings::Reach::named(config::Layer::Personal),
                        cli.dry_run,
                        ctx,
                    )
                }
                Some(SettingsCmd::Edit { capability, name }) => cmd::settings::edit(
                    &paths,
                    capability.as_deref(),
                    name.as_deref(),
                    config::Layer::Personal,
                ),
                Some(SettingsCmd::Mcp { cmd }) => cmd::mcp::mcp(&paths, cmd, cli.dry_run, ctx),
            }
        }

        Cmd::Set {
            key,
            value,
            save,
            local,
        } => {
            let paths = Paths::discover(&cwd)?;
            match cmd::settings::names(&paths, key, ctx) {
                cmd::settings::Names::AFeature => cmd::settings::feature_switch(
                    &paths,
                    key,
                    cmd::settings::on_or_off(key, value)?,
                    cmd::settings::reach_in(&paths, config::OMH, key, *local, *save)?,
                    cli.dry_run,
                    ctx,
                ),
                cmd::settings::Names::AnEntryOf(feature) => {
                    Err(cmd::settings::an_entry_is_not_a_feature(key, &feature))
                }
                cmd::settings::Names::ACatalogueEntry(cap) => {
                    Err(cmd::settings::a_catalogue_entry_is_not_a_setting(key, cap))
                }
                cmd::settings::Names::ASetting | cmd::settings::Names::Neither => {
                    if key == "account" {
                        cmd::settings::no_account_that_no_login_answers_to(&paths, value, ctx)?;
                    }
                    let reached = cmd::settings::reach(&paths, key, *local, *save)?;
                    cmd::settings::set(&paths, key, value, reached, cli.dry_run, ctx)
                }
            }
        }
        Cmd::Unset { key, save, local } => {
            let paths = Paths::discover(&cwd)?;
            match cmd::settings::names(&paths, key, ctx) {
                cmd::settings::Names::AFeature => cmd::settings::feature_forget(
                    &paths,
                    key,
                    cmd::settings::reach_in(&paths, config::OMH, key, *local, *save)?,
                    cli.dry_run,
                    ctx,
                ),
                cmd::settings::Names::AnEntryOf(feature) => {
                    Err(cmd::settings::an_entry_is_not_a_feature(key, &feature))
                }
                cmd::settings::Names::ACatalogueEntry(cap) => {
                    Err(cmd::settings::a_catalogue_entry_is_not_a_setting(key, cap))
                }
                cmd::settings::Names::ASetting | cmd::settings::Names::Neither => {
                    let reached = cmd::settings::reach(&paths, key, *local, *save)?;
                    cmd::settings::unset(&paths, key, reached, cli.dry_run, ctx)
                }
            }
        }

        Cmd::Use {
            capability,
            name,
            all,
        } => cmd::catalogue::use_cmd(
            &cwd,
            capability.as_deref(),
            name.as_deref(),
            *all,
            cli.dry_run,
            ctx,
        ),
        Cmd::Unuse { capability, name } => {
            cmd::catalogue::unuse_cmd(&cwd, capability, name, cli.dry_run, ctx)
        }

        Cmd::Import {
            capability,
            harness,
            from,
        } => {
            cmd::catalogue::import_cmd(&cwd, capability, harness, from.as_deref(), cli.dry_run, ctx)
        }

        Cmd::Memory { cmd } => match cmd {
            None => cmd::memory::memory_ls(&cwd, ctx),
            Some(MemoryCmd::Lint) => cmd::memory::memory_lint(&cwd, ctx),
            Some(MemoryCmd::Stale) => cmd::memory::memory_stale(&cwd, ctx),
            Some(MemoryCmd::Promote { keys }) => cmd::memory::memory_promote(&cwd, keys, ctx),
            Some(MemoryCmd::Serve {
                team,
                local,
                session,
            }) => cmd::memory::memory_serve(team.clone(), local.clone(), session.clone()),
            Some(MemoryCmd::Rm { key, layer, at }) => {
                cmd::memory::memory_rm(&cwd, key, *layer, at.as_deref(), cli.dry_run, ctx)
            }
            Some(MemoryCmd::Remember {
                expected,
                observed,
                evidence,
                answers,
                relates_to,
                invalidated_by,
                source,
                if_exists,
            }) => cmd::memory::memory_remember(
                &cwd,
                memory::Remembered {
                    expected: expected.clone(),
                    observed: observed.clone(),
                    evidence: evidence.clone(),
                    answers: answers.clone(),
                    relates_to: relates_to.clone(),
                    invalidated_by: invalidated_by.clone(),
                    source: source.clone().unwrap_or_default(),
                    recorded: memory::today(),
                },
                *if_exists,
                ctx,
            ),
        },

        // Before `run` looks anything up: which flags are whose is a question
        // about the command line, and answering it after resolving an adapter
        // would report an unknown harness for a mistyped flag.
        Cmd::New { harness, args } => {
            // The `--` is rebuilt, not dropped. `passthrough` is what decides
            // whose a flag is, and it decides by looking for that separator —
            // handing it `[claude, --json]` where the user typed
            // `claude -- --json` asks it the wrong question, and it answered
            // by refusing a flag the user had already assigned.
            // `[harness, ...args]`. There is no separator to strip: clap's
            // `last = true` means `args` holds only what followed one.
            let mut argv = vec![harness.clone()];
            argv.extend(args.iter().cloned());
            cmd::session::run(&cwd, &argv, session::Start::Fresh, cli.dry_run, ctx)
        }
    }
}

/// What to tell someone whose word matched nothing. Pure so it can be tested:
/// the message is the entire value of this path.
pub(crate) fn tool_hint(name: &str, harnesses: &[String], editors: &[String]) -> String {
    if editors.iter().any(|e| e == name) {
        return format!("`{name}` is an editor — try `omh s attach {name}`");
    }
    format!(
        "unknown harness `{name}`\n  available: {}",
        harnesses.join(", ")
    )
}

/// Neither a harness nor a reserved word — say what is available, since the
/// user cannot tell from the name alone which kind they meant.
pub(crate) fn unknown_tool(paths: &Paths, name: &str, original: anyhow::Error) -> anyhow::Error {
    let harnesses: Vec<String> = Adapter::load_dir(&paths.adapters())
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.name)
        .collect();
    if harnesses.is_empty() {
        return original;
    }
    let editors: Vec<String> = editor::Editor::load_dir(&paths.editors())
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.name)
        .collect();
    anyhow::anyhow!("{}", tool_hint(name, &harnesses, &editors))
}

/// Read one policy key through the usual layer merge.
pub(crate) fn policy_value(paths: &Paths, key: &str) -> Option<String> {
    config::policy(paths)
        .ok()?
        .into_iter()
        .find(|s| s.key == key)
        .map(|s| s.value)
}

pub(crate) fn runtime_preference(paths: &Paths) -> String {
    policy_value(paths, "runtime").unwrap_or_else(|| "auto".into())
}

/// The agent's working directory inside the sandbox. Named once, so the note
/// store and the launch plan cannot disagree about it.
pub(crate) fn container_workdir() -> &'static str {
    "/work"
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
