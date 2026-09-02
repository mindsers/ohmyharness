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
                    image::container_running(b.as_ref(), &format!("omh-{legacy}-{id}")),
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
                cmd::session::rm(&cwd, id, *force, ctx)
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
mod tests {
    use super::*;
    use crate::session::Session;
    use clap::CommandFactory;
    use std::collections::{BTreeMap, BTreeSet};
    use std::process::Command;

    /// The tally omits itself rather than saying zero.
    ///
    /// `omh s rm` deleted a branch because a count it could not take read as
    /// `0`; this is the same mistake one layer out, where it would be printed
    /// rather than acted on. A pure function over the `Result`, so the guard
    /// needs no repository and cannot be defeated by a fixture.
    #[test]
    fn a_tally_omh_could_not_take_is_absent_rather_than_zero() {
        assert_eq!(
            cmd::harvest::branch_tally(&Ok(1)),
            " (1 commit on the branch)"
        );
        assert_eq!(
            cmd::harvest::branch_tally(&Ok(3)),
            " (3 commits on the branch)"
        );
        assert_eq!(
            cmd::harvest::branch_tally(&Ok(0)),
            " (0 commits on the branch)",
            "a real zero is still an answer and still gets said"
        );
        assert_eq!(
            cmd::harvest::branch_tally(&Err(anyhow::anyhow!("bad revision"))),
            "",
            "and a count nobody took says nothing at all"
        );
    }

    const BUNDLED_ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BUNDLED_EDITORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/editors");

    /// A full command line, program name included, the way `main` sees it.
    fn cli_argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("omh")
            .chain(parts.iter().copied())
            .map(str::to_string)
            .collect()
    }

    /// The session goes first, and everything after it is what you would have
    /// typed anyway.
    ///
    /// Four spellings meant the same thing before this — `s diff`, `s diff s01`,
    /// `s -s s01 diff`, `-s s01 s diff` — over two mechanisms applied unevenly:
    /// `rm` required the positional, `commit` and `push` ignored it, and `push`
    /// could not have one because that slot is the branch name. This is the one
    /// form they collapse into.
    #[test]
    fn a_session_named_first_is_the_session_the_command_acts_on() {
        // a session verb: the prefix desugars to the sessions namespace
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01", "diff"])),
            (Some("s01".to_string()), cli_argv(&["s", "diff"]))
        );
        // The verbs come from the parser, not from a list here, so `log` joins
        // this the moment `SessionsCmd` has it — which is the next step in the
        // spec and exactly why the list is derived.
        assert!(
            !cli::session_prefix(cli_argv(&["s01", "push", "fix/x"]))
                .1
                .contains(&"s01".to_string()),
            "the id is lifted out, never left in the arguments"
        );
        // …carrying its own flags untouched
        assert_eq!(
            cli::session_prefix(cli_argv(&["s02", "commit", "--keep", "1,3"])),
            (
                Some("s02".to_string()),
                cli_argv(&["s", "commit", "--keep", "1,3"])
            )
        );
        // A session verb whose flags do not parse stays a session verb. This
        // is the case `--keep 1,3` used to cover, before #56 gave `--keep` a
        // value and both readings started parsing: without the verb check, the
        // fallback would offer to launch a harness called `commit`.
        assert_eq!(
            cli::session_prefix(cli_argv(&["s02", "commit", "--whatever"])),
            (
                Some("s02".to_string()),
                cli_argv(&["s", "commit", "--whatever"])
            ),
            "a session verb omh cannot parse is not a harness"
        );
    }

    /// The one place the desugaring is not a pure alias: a launch.
    ///
    /// `sessions` has no verb for starting a harness, so when what follows is
    /// not a session verb the prefix still names the session and the command
    /// runs where it lives. Everything after a harness name is still the
    /// harness's argv.
    #[test]
    fn a_session_named_first_also_works_for_what_sessions_has_no_verb_for() {
        // The launch used to be the worked example here — `sessions` had no
        // verb for starting a harness, so `omh s01 claude` fell through to the
        // line as written. It has one now (`resume`), and a bare word is not a
        // launch. `attach` was the example after that, and it stopped being one
        // the moment it became a session verb: the sessions reading now parses,
        // which is the *other* branch. `doctor` is the genuine case left — a
        // top-level command the prefix scopes, with no verb under `sessions`.
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01", "doctor"])),
            (Some("s01".to_string()), cli_argv(&["doctor"]))
        );
        // And the branch `attach` moved to: a real session verb, desugared.
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01", "attach", "zed"])),
            (Some("s01".to_string()), cli_argv(&["s", "attach", "zed"])),
            "a verb `sessions` has is rewritten through it, not left as written"
        );
        // `graph` had a positional of its own until the prefix landed, and for
        // one commit it had both — the prefix set the session and `graph` read
        // the positional, so the browser opened on whichever session `pick`
        // chose. This asserts the *lifting*, which is all this function does.
        // What happens next is a refusal: the graph is one server per repo, so
        // `omh s01 graph` names a scope nothing can honour and `dispatch` says
        // so rather than opening on a session the id had no part in choosing.
        let (named, argv) = cli::session_prefix(cli_argv(&["s01", "graph"]));
        assert_eq!(
            cli::the_one_session(named, Cli::try_parse_from(&argv).unwrap().session).unwrap(),
            Some("s01".to_string()),
            "the prefix is lifted whatever follows it — `consumes_session` is \
             what decides whether the command may have it"
        );
    }

    /// A session and nothing to do with it is a question, and `s` asks it.
    ///
    /// Neither reading parses, and which error surfaces is a choice: as
    /// written, `omh s01` is a request to launch a harness called `s01` and the
    /// answer would be ``unknown harness `s01` `` — true, useless, and about
    /// the wrong thing.
    #[test]
    fn a_session_with_nothing_to_do_is_asked_what_to_do() {
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01"])),
            (Some("s01".to_string()), cli_argv(&["s"]))
        );
    }

    /// A harness's own `-s` is not omh naming the session twice.
    ///
    /// `passthrough` already records why, in the same file: "`-s` is omh's
    /// session flag and is also a flag plenty of harnesses have; refusing
    /// shorts would break launches that work today to guard a mistake nobody
    /// has made." Everything after a harness name belongs to the harness, and
    /// the duplicate check has to respect the same boundary — a check that
    /// reads the whole line refuses a launch that has always worked.
    #[test]
    fn a_harness_flag_is_not_omh_naming_the_session_twice() {
        // Spelled through `resume` now that a bare word is not a launch. The
        // prefix has to survive *and* the harness's own `-s` has to stay the
        // harness's — an earlier version of this test used `omh new`, where
        // `session_prefix` returns before the arbitration runs, so both halves
        // were vacuous.
        let (prefix, argv) =
            cli::session_prefix(cli_argv(&["s01", "resume", "claude", "--", "-s", "some"]));
        assert_eq!(prefix, Some("s01".to_string()), "the prefix is lifted");
        let parsed = Cli::try_parse_from(&argv).expect("a launch is a valid line");
        assert_eq!(parsed.session, None, "the harness keeps its own flags");
    }

    /// omh's own flags may sit between the session and the verb.
    ///
    /// `omh s01 --json diff` is an ordinary thing to type, and the first
    /// version of this could not read it: it classified `argv[2]` and nothing
    /// else, so a flag there was neither a verb nor a harness name and the line
    /// went to the harness dispatcher as `unknown harness \`diff\``.
    ///
    /// Rather than teach this function which flags take values — knowledge that
    /// already lives in the parser and would rot here — the sessions reading is
    /// tried and kept when it parses.
    #[test]
    fn omhs_own_flags_may_sit_between_the_session_and_the_verb() {
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01", "--json", "diff"])),
            (Some("s01".to_string()), cli_argv(&["s", "--json", "diff"]))
        );
        assert_eq!(
            cli::session_prefix(cli_argv(&["s01", "--dry-run", "resume"])),
            (
                Some("s01".to_string()),
                cli_argv(&["s", "--dry-run", "resume"])
            ),
            "and a verb is still a verb"
        );
    }

    /// Every Rust source under these directories, however deep.
    ///
    /// The scans below used `read_dir` and a loop, which reads the top floor
    /// of a directory and stops there. `src/memory/` is seven files and about
    /// five thousand lines, and a spliced doc comment or a stranded `#[test]`
    /// inside it was invisible to the guards that exist to catch exactly
    /// those — verified by planting one and watching them pass.
    fn rust_sources(dirs: &[&str]) -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut out = Vec::new();
        let mut stack: Vec<std::path::PathBuf> = dirs.iter().map(|d| root.join(d)).collect();
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Every word that can follow `omh`, taken from the parser rather than
    /// written down.
    ///
    /// Keying admission on the current vocabulary is the thing this file
    /// forbids, for a stated reason: a guard that only reads words the parser
    /// still knows cannot see a word *leaving*, and retiring `ls` is exactly
    /// how two printed lines went stale unnoticed. So this is never the only
    /// way in — the retired spelling this change caught in `ssh.rs` is not in
    /// here, by definition, and was admitted by its backticks.
    ///
    /// As a *supplement* it reaches what no position can: a literal that opens
    /// with a command, `"omh use <capability> <name>, or …"`, which sits in the
    /// same place as omh's error voice and cannot be told apart from it by
    /// looking at what comes before. The blind spot that buys — a retired verb
    /// at a literal open — is precisely what
    /// `nothing_still_offers_a_verb_that_was_retired` reads the whole tree for.
    /// The two guards compose; neither alone is enough.
    fn vocabulary() -> std::collections::BTreeSet<String> {
        use clap::CommandFactory;
        let mut out: std::collections::BTreeSet<String> = Cli::command()
            .get_subcommands()
            .flat_map(|c| {
                std::iter::once(c.get_name().to_string())
                    .chain(c.get_all_aliases().map(str::to_string))
            })
            .collect();
        // A scan that read no vocabulary would admit nothing through this arm
        // and say nothing about it.
        assert!(
            out.len() > 10,
            "clap named {} subcommands, fewer than omh has",
            out.len()
        );
        out.insert("s01".to_string());
        out
    }

    /// The versioned base sets. `omh why` prints strings out of these, so the
    /// printed-line guard reads them beside the source — it is the one caller.
    fn manifests() -> Vec<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("base");
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        assert!(!out.is_empty(), "no base set to read");
        out.sort();
        out
    }

    /// A scan that stopped early agrees with anything.
    ///
    /// Named files rather than a count alone: a count answers *did it read
    /// something*, and the failure being guarded against is *did it read the
    /// part nobody thought of*.
    fn the_whole_tree(files: &[std::path::PathBuf]) {
        assert!(
            files.len() > 30,
            "the scan read {} sources, fewer than this crate has",
            files.len()
        );
        assert!(
            files.iter().any(|f| f.ends_with("memory/tools.rs")),
            "the scan never descended into src/memory/"
        );
    }

    /// No attribute has been separated from the item it applies to.
    ///
    /// The other half of the same accident, and the half that keeps happening:
    /// inserting before an anchor without reading what precedes it walks back
    /// over the *next* item's `#[test]` and strands it above the new one. The
    /// stranded attribute then applies to whatever the insertion brought, and
    /// the test it came from silently stops being a test — which is how a
    /// helper it alone called became dead code and took the lint run down.
    ///
    /// Three times in two changes. Clippy refuses `duplicated attribute`, but
    /// clippy does not run on this machine — a 1.81 shim against a 1.85 crate
    /// — so CI is the only place it is caught, one push and four minutes
    /// later. This is the same guard, here.
    ///
    /// Two shapes: a `#[test]` that no longer sits on a function, and **any**
    /// attribute that sits above a doc comment. The second was added after the
    /// first failed to catch the same mistake made to a `#[derive(…)]` — this
    /// paragraph used to say derives "legitimately stack and sit above doc
    /// comments in this tree", which was asserted rather than checked. There
    /// were zero such instances; a doc comment always comes first here.
    #[test]
    fn no_test_attribute_was_stranded_from_its_function() {
        let mut stranded = Vec::new();
        let files = rust_sources(&["src", "tests"]);
        the_whole_tree(&files);
        {
            for file in &files {
                let body = std::fs::read_to_string(file).unwrap();
                let lines: Vec<&str> = body.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    if line.trim() != "#[test]" {
                        continue;
                    }
                    // What may follow: the function, or more attributes
                    // (`#[should_panic]`, `#[ignore]`). Anything else — a doc
                    // comment, a blank line, another `#[test]` — means this one
                    // is no longer attached to what it was written for.
                    let next = lines.get(n + 1).map(|l| l.trim()).unwrap_or("");
                    let attached = next.starts_with("fn ")
                        || next.starts_with("async fn ")
                        || (next.starts_with("#[") && next != "#[test]");
                    if !attached {
                        stranded.push(format!(
                            "{}:{}: followed by `{next}`",
                            file.display(),
                            n + 1
                        ));
                    }
                }
                // Any attribute above a doc comment — the same accident
                // wearing a different hat. An insertion before an anchor
                // swallowed `report::Down`'s `#[derive(…)]` and left it
                // heading the new block, after the `#[test]` half of this was
                // already written, so the rule is the general one now.
                for (n, line) in lines.iter().enumerate() {
                    if line.trim().starts_with("#[")
                        && lines
                            .get(n + 1)
                            .is_some_and(|l| l.trim().starts_with("///"))
                    {
                        stranded.push(format!(
                            "{}:{}: `{}` sits above a doc comment",
                            file.display(),
                            n + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
        assert!(
            stranded.is_empty(),
            "`#[test]` separated from its function: {stranded:#?}"
        );
    }

    /// No source or document still tells anyone to type a verb that is gone.
    ///
    /// `the_lines_omh_prints_are_lines_omh_accepts` cannot do this, and the
    /// reason is worth writing down. It used to check only a line whose second
    /// word was a **known** session verb, so retiring `ls` did not make those
    /// lines fail — it quietly removed them from the scan, and two user-facing
    /// messages went on naming a command that no longer parses. A guard keyed
    /// on the current vocabulary cannot see a word leaving it; this one is
    /// keyed on the words that left.
    ///
    /// That clause is gone now, and the two guards no longer overlap the way
    /// this paragraph once claimed. Renaming a command is caught over there —
    /// spelling `Cmd::Why` as `wye` produces five refusals across `derive.rs`,
    /// `why.rs` and `stack.rs`. What that guard cannot see is a *file* it does
    /// not read; what this one cannot see is a *verb* that left the needle
    /// list below. Neither sees arity drift.
    ///
    /// Docs are included because nothing else reads them for command
    /// spellings: `tests/docs.rs` checks links, anchors and reachability.
    ///
    /// The whole tree is walked rather than an allowlist of directories. The
    /// first version read `src`, `tests` and `docs`, and stayed green while
    /// `README.md` — the front page, and the file most people read first —
    /// still printed the retired verb. An allowlist is the same shape of
    /// mistake as keying on the current vocabulary: both go quiet about the
    /// place nobody thought of.
    ///
    /// It cannot tell *offering* a spelling from *saying it is gone*, so prose
    /// explaining the removal has to name the verb rather than the whole
    /// invocation — "there is no `ls` verb", not the line somebody used to
    /// type. That is a real limitation and the cheaper side of it: a scan that
    /// tried to judge intent would let the next one through.
    /// A wrapped line with its comment leader off, so joining it to the line
    /// above reads as the one sentence the author wrote.
    ///
    /// Without this, a continuation opens with its comment leader, and no
    /// needle whose first word is `omh` can match across the break.
    fn continuation(line: &str) -> &str {
        let text = line.trim_start();
        for leader in ["///", "//!", "//", "#", "*", ">"] {
            if let Some(rest) = text.strip_prefix(leader) {
                return rest.trim_start();
            }
        }
        text
    }

    #[test]
    fn nothing_still_offers_a_verb_that_was_retired() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // Spellings omh used to accept and does not. `ls` became the absence
        // of a verb in 2026.08 — `omh s` is the listing and `omh sNN` is one
        // row of it.
        //
        // Assembled rather than written whole, and that is not decoration: the
        // sweep that retired the verb was a search-and-replace over `src` and
        // `docs`, and it rewrote this very constant, turning the guard into
        // one that matched every file in the tree. A guard against a spelling
        // cannot spell it.
        // Both the written spelling and the argv form the tests use. The ~15
        // call sites the sweep rewrote were argv, so the shape most likely to
        // be left behind was the one a prose-only needle cannot see — and one
        // was: the JSON guard went on invoking a line that no longer parsed,
        // and passed, because its empty stdout read as nothing to say.
        const ON_PURPOSE: &str = "types the retired verb on purpose";
        let gone: [String; 19] = [
            // `attach` became a session verb in 2026.08.
            //
            // **No trailing character.** The first version of this needle had a
            // trailing space and a trailing quote, and matched *nothing* — this
            // repo cites a command inside backticks, and a backtick is
            // neither. Seventeen sites survived while the guard reported clean,
            // one of them the header omh writes into `~/.ssh/config.d/`.
            //
            // That is this defect's third outing, and the shape is always the
            // same: the needle gets written to match the last leftover somebody
            // remembers rather than how the tree actually spells a command. The
            // `code` needle below never had a trailing character and never had
            // the problem.
            format!("omh {}", "attach"), // types the retired verb on purpose
            format!("{:?}, {:?}", "--json", "attach"), // types the retired verb on purpose
            format!("omh s {}", "ls"),   // types the retired verb on purpose
            format!("omh sessions {}", "ls"), // types the retired verb on purpose
            format!("{:?}, {:?}", "s", "ls"), // types the retired verb on purpose
            format!("{:?}, {:?}", "sessions", "ls"), // types the retired verb on purpose
            // The bare-name launch and the flag that forced a fresh session,
            // both retired with the catch-all. The bare name is the one people
            // have in their fingers and the one the docs said most often, so
            // it is the likeliest to be left behind.
            format!("omh {}", "claude"), // types the retired verb on purpose
            format!("omh {}", "opencode"), // types the retired verb on purpose
            format!("omh {}new", "--"),  // types the retired verb on purpose
            // Retired long enough ago that nothing here was watching them, and
            // found by hand: `attach` replaced the first two, and the third has
            // only ever been reachable under `config`. **Two** were still being
            // offered — one from inside the file omh writes into your
            // `~/.ssh/config.d/`, one from a shipped adapter.
            //
            // The middle one is prophylactic and says so rather than borrowing
            // the other two's evidence: its last site went with `scripts/
            // smoke.sh` one commit before this one, so nothing offered it when
            // this was written. A needle costs nothing; a needle presented as
            // a catch that never caught anything costs the next reader.
            //
            // **None of these end in a space.** Two did, and `attach` was added
            // the same way and matched nothing at all while seventeen sites
            // survived — this repo writes a command inside backticks, and a
            // backtick is not a space. Audited when that was found: no site was
            // hiding behind the two, so this is hardening rather than a repair,
            // but the shape is the one that has now failed three times.
            //
            // Their absence from this list is what let a sweep leave them
            // behind, which is the argument for adding a name here whenever
            // one leaves rather than when someone next trips over it. `run` is
            // not among them: `omh runs` is ordinary prose in four files, and
            // a needle that matches prose is a needle that gets deleted.
            // `config` went in 0.7.0. Added to the *needle* list as well as to
            // `RETIRED`, which is a different list for a different job: that
            // one gives a person the replacement, this one stops a stale
            // spelling surviving in the tree. Adding to one is not adding to
            // the other, and eight lines in the base manifest were invisible to
            // both for exactly that reason — the parse guard had dropped them
            // silently the moment `config` left the vocabulary.
            format!("omh {}", "config"), // types the retired verb on purpose
            // Three shapes, because a bare needle would also match "omh
            // reports" — `repo` is a prefix of ordinary English here in a way
            // `attach` never was. The lesson from that one is not *never
            // terminate*; it is *cover the shapes the tree actually uses*:
            // backticked prose, prose with a verb after it, and an argv array.
            format!("omh {}`", "repo"), // types the retired verb on purpose
            format!("omh {} ", "repo"), // types the retired verb on purpose
            format!("[{:?}", "repo"),   // types the retired verb on purpose
            format!("omh {}", "code"),  // types the retired verb on purpose
            format!("omh {}", "fwd"),   // types the retired verb on purpose
            format!("omh {}", "mcp"),   // types the retired verb on purpose
            // The wide listing's verb, retired in favour of the noun. It still
            // listed sessions — what it never showed was what any of them was
            // *doing*, which is `omh s`, so the name promised a summary it did
            // not give.
            //
            // Both shapes, and the second is not decoration. The prose needle
            // alone shipped in the commit that retired the verb, and the argv
            // form it cannot see went on invoking it inside the JSON guard,
            // which passed because clap writes to stderr and empty stdout read
            // as *nothing to say* — the same defect this file already records
            // one retirement earlier, repeated because only half the needle
            // was written.
            format!("omh {}", "ls"), // types the retired verb on purpose
            // Scoped to the sessions argv forms. It was `format!("{:?}]", "ls")`
            // — any array ending in `"ls"` — which also matched
            // `["settings", "mcp", "ls"]`, where `ls` is a live verb of a
            // different command. A needle that fires on a spelling that still
            // works teaches the reader to route around the guard.
            format!("{:?}, {:?}]", "s", "ls"), // types the retired verb on purpose
            format!("{:?}, {:?}]", "sessions", "ls"), // types the retired verb on purpose
        ];
        let mut found = Vec::new();
        let mut read = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Build output and git's own storage are not ours to
                    // read, and `target` alone is large enough to matter.
                    //
                    // Dot-directories go too, except `.github`, whose prose
                    // and templates name commands like any other page. The
                    // one that forced this is `.omh/`: gitignored, holding
                    // this checkout's own notes, and *read* until now — so a
                    // stale spelling in a machine-local file failed this guard
                    // on a maintainer's laptop and passed on CI, which is
                    // worse than either being wrong consistently.
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let skipped = name == "target" || (name.starts_with('.') && name != ".github");
                    if !skipped {
                        stack.push(path);
                    }
                    continue;
                }
                // Not just `.rs` and `.md`. Every file below names a command
                // somewhere, and none of them was read: `base/2026.08.toml`
                // holds eight `remove =` strings that `omh why` prints
                // verbatim, `install.sh` and the workflows run omh, and
                // `packaging/homebrew/omh.rb.tmpl` *asserts a command
                // spelling* inside `brew test` — which fails at release time,
                // not in CI, because nothing runs it before a tag.
                let readable = matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rs" | "md" | "toml" | "sh" | "yml" | "yaml" | "tmpl")
                );
                if !readable {
                    continue;
                }
                let body = std::fs::read_to_string(&path).unwrap();
                read.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
                let lines: Vec<&str> = body.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    // Declared, not inferred. Two lines have to type the verb
                    // to do their job — the needles just above, and the test
                    // that checks typing it is refused — and a scan that tried
                    // to work out which those were would be a scan that let
                    // the next one through. Saying so on the line is cheap and
                    // greppable; guessing is neither.
                    if line.contains(ON_PURPOSE) {
                        continue;
                    }
                    // **And the line after it, joined.** Every needle here is
                    // two words with a space between them, and rustfmt wraps
                    // comments wherever the column runs out — so `omh` ending
                    // one line and `config mcp add` opening the next is a
                    // spelling the scan simply could not see. Four sites
                    // survived that way, one of them user-facing prose
                    // offering a command that exits 1.
                    //
                    // The needles reason carefully about *terminators* — a
                    // backtick is not a space, and that has cost this guard
                    // three outings. A line break supplies no terminator at
                    // all, which is the same blind spot one step further out.
                    let wrapped = lines
                        .get(n + 1)
                        .filter(|next| !next.contains(ON_PURPOSE))
                        .map(|next| format!("{} {}", line.trim_end(), continuation(next)));
                    for spelling in &gone {
                        let across = wrapped
                            .as_deref()
                            .is_some_and(|w| w.contains(spelling.as_str()));
                        if line.contains(spelling.as_str()) || across {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        // Named files rather than a count. A count cannot tell a walk that
        // stopped early from one that read everything, and reading everything
        // is the only claim this guard makes that is worth anything.
        for must in [
            "README.md",
            "src/main.rs",
            "docs/commands.md",
            "base/2026.08.toml",
            "packaging/homebrew/omh.rb.tmpl",
        ] {
            assert!(
                read.iter().any(|p| p == std::path::Path::new(must)),
                "the scan never read {must}, so its silence says nothing"
            );
        }
        assert!(
            found.is_empty(),
            "these still offer a command omh no longer accepts: {found:#?}"
        );
    }

    /// A file's path from the repository root, for a map key.
    ///
    /// The basename was the key until `src/cmd/` existed, and then
    /// `src/memory.rs` and `src/cmd/memory.rs` were one entry — so a printed
    /// line moving from one to the other left the total unchanged and this
    /// scan said nothing at all. That is the exact failure the exact map below
    /// exists to prevent, arriving through the key rather than the count.
    fn under_src(file: &std::path::Path) -> String {
        let root = format!("{}/", env!("CARGO_MANIFEST_DIR"));
        let full = file.to_string_lossy().replace('\\', "/");
        full.strip_prefix(&root).unwrap_or(&full).to_string()
    }

    /// The command lines the **docs** print are lines omh accepts.
    ///
    /// A synopsis block in `docs/commands.md` is the one artefact in this repo
    /// that is neither compiled nor parsed nor grepped for arity, and it is the
    /// first thing a migrating reader looks at. The 0.7.0 rename left four
    /// wrong lines in a five-line block, under a heading naming the command
    /// that replaced the deleted one: two byte-identical lines that had lost
    /// the argument telling them apart, a flag removed a release earlier, and a
    /// write target stated as the opposite of what omh does.
    ///
    /// **The notation is expanded rather than skipped**, because a rule that
    /// declines to read the hard lines is a rule that reads the ones already
    /// right:
    ///
    /// - `<key>` and `{id}` become a value, by [`regex_lite_fill`]
    /// - `a|b` becomes `a`; one alternative is enough to check an arity
    /// - `[…]` loses its brackets and **keeps its contents**, because dropping
    ///   the group would hide exactly the defect that occurred — `[--shared]`
    ///   named a flag clap refuses, and a rule that checks only the required
    ///   core never asks
    /// - a bare word inside brackets is a hole too: `[n]`, `[name]`
    ///
    /// **What is not a command.** The first word has to be one clap knows, or
    /// the line is omh talking — `omh: …`, `omh has no record …`, and the
    /// prose in every fence that shows output. The line stops at the gutter,
    /// at a `#`, at `--`, and at the first word carrying a character no
    /// command line has, which is how the box-drawing in a diagram ends a
    /// reading rather than failing one.
    ///
    /// A comment saying **an error** inverts the assertion: `omh new claude
    /// --resume x` is documented *because* omh refuses it, and a doc that
    /// promises a refusal has to be checked in that direction or the sentence
    /// beneath it stops being true.
    #[test]
    fn the_lines_the_docs_print_are_lines_omh_accepts() {
        let mut pages: Vec<std::path::PathBuf> = std::fs::read_dir("docs")
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .collect();
        pages.extend(
            std::fs::read_dir("docs/design")
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "md")),
        );
        pages.push("README.md".into());
        pages.sort();
        // Named, not counted: a walk that stopped early and a walk that found
        // nothing look the same from a total.
        for must in [
            "docs/commands.md",
            "docs/configuration.md",
            "docs/design/profile.md",
            "README.md",
        ] {
            assert!(
                pages.iter().any(|p| p == std::path::Path::new(must)),
                "the scan never read {must}, so its silence says nothing"
            );
        }

        // What a hole stands for, read off the name the docs gave it. The
        // sibling scan fills the *first* hole with a session id because that
        // is what omh's own messages mean by one; a page writes `<id>` when it
        // means that and `<n>` when it means a checkpoint, and filling
        // `omh s diff <n>` with `s01` refuses a line that is correct.
        fn hole(name: &str) -> &'static str {
            match name.trim_matches(['<', '>', '[', ']', '{', '}']) {
                "id" | "session" | "sNN" => "s01",
                "n" | "m-o" | "checkpoint" | "turns" => "1",
                _ => "x",
            }
        }

        let vocab = vocabulary();
        let mut checked: std::collections::BTreeMap<String, usize> = Default::default();
        let mut refused: Vec<String> = Vec::new();
        for page in &pages {
            let body = std::fs::read_to_string(page).unwrap();
            let name = page.file_name().unwrap().to_string_lossy().to_string();
            let mut fenced = false;
            // A shell continuation is one line to the reader and to omh.
            let mut joined: Vec<(usize, String)> = Vec::new();
            let mut carry: Option<(usize, String)> = None;
            for (n, raw) in body.lines().enumerate() {
                match carry.take() {
                    Some((at, mut held)) => {
                        held.push(' ');
                        held.push_str(raw.trim());
                        match held.strip_suffix('\\') {
                            Some(head) => carry = Some((at, head.trim_end().to_string())),
                            None => joined.push((at, held)),
                        }
                        continue;
                    }
                    None => match raw.trim_end().strip_suffix('\\') {
                        Some(head) => carry = Some((n, head.trim_end().to_string())),
                        None => joined.push((n, raw.to_string())),
                    },
                }
            }
            for (n, raw) in &joined {
                let (n, raw) = (*n, raw.as_str());
                if raw.trim_start().starts_with("```") {
                    fenced = !fenced;
                    continue;
                }
                if !fenced {
                    // Prose names a command mid-clause, in the middle of a
                    // sentence about what it used to do. Reading those would
                    // make this a guard against writing about omh's history.
                    continue;
                }
                let line = raw.trim_start();
                let line = line.strip_prefix("$ ").unwrap_or(line);
                let Some(rest) = line.strip_prefix("omh ") else {
                    continue;
                };
                // A refusal the page is *showing*. Inverted rather than
                // skipped: a documented error that quietly starts working is a
                // paragraph that has stopped being true.
                let inverted = rest
                    .split_once('#')
                    .is_some_and(|(_, note)| note.contains("an error"));
                // Where the line ends and the page resumes.
                // The gutter, but not one inside a quoted value: a synopsis
                // separates a command from its explanation with two spaces,
                // and `--observed "a  b"` holds two of its own. Tracked rather
                // than tested for, because an apostrophe in the *explanation*
                // — "the agent's own commits" — is not a quote opening
                // anything, and a rule that read it as one put the whole
                // sentence back into the command.
                let mut rest = rest;
                let mut quote: Option<char> = None;
                let mut gutter = None;
                let bytes: Vec<char> = rest.chars().collect();
                let mut at = 0;
                for (i, c) in bytes.iter().enumerate() {
                    match quote {
                        Some(mark) if *c == mark => quote = None,
                        Some(_) => {}
                        None if "\"'".contains(*c) => quote = Some(*c),
                        None if *c == ' ' && bytes.get(i + 1) == Some(&' ') => {
                            gutter = Some(at);
                            break;
                        }
                        None => {}
                    }
                    at += c.len_utf8();
                }
                if let Some(at) = gutter {
                    rest = &rest[..at];
                }
                for stop in [" #", " -- "] {
                    if let Some(at) = rest.find(stop) {
                        rest = &rest[..at];
                    }
                }
                let mut words: Vec<String> = Vec::new();
                let mut quote: Option<char> = None;
                let mut owed = false;
                let mut judgeable = true;
                for word in rest.split_whitespace() {
                    // A quoted value is one argument however many spaces it
                    // holds — `omh s01 commit -m "Fix the tap guard"`.
                    if let Some(mark) = quote {
                        if word.ends_with(mark) {
                            quote = None;
                        }
                        continue;
                    }
                    let bare = word.trim_matches(['[', ']']);
                    if let Some(mark) = bare.chars().next().filter(|c| "\"'".contains(*c)) {
                        words.push("x".into());
                        owed = false;
                        if !(bare.len() > 1 && bare.ends_with(mark)) {
                            quote = Some(mark);
                        }
                        continue;
                    }
                    // `…` is *whatever you were typing*. As a flag's value it
                    // is one word; on its own it is the rest of a line this
                    // scan cannot see, and a line it cannot see is one it must
                    // not judge — reading `omh memory remember --if-exists
                    // override …` as complete would refuse a correct page.
                    if let Some(head) = bare.strip_suffix('…') {
                        // `<key>…` is one-or-more of the hole it is glued to,
                        // and one is enough to check an arity.
                        if !head.is_empty() {
                            words.push(hole(head).to_string());
                            owed = false;
                            continue;
                        }
                        // On its own it is the rest of a line this scan cannot
                        // see, and a line it cannot see is one it must not
                        // judge — reading `omh memory remember --if-exists
                        // override …` as complete would refuse a correct page.
                        match owed {
                            true => {
                                words.push("x".into());
                                owed = false;
                            }
                            false => judgeable = false,
                        }
                        continue;
                    }
                    // Nothing a command line can carry. In a diagram this is
                    // the box-drawing; in a captured report it is the em dash
                    // omh writes a summary with. Either way the command is
                    // whole before it.
                    let typeable =
                        |c: char| c.is_ascii_alphanumeric() || "-_./,:=@+~$|".contains(c);
                    if bare.is_empty() || !bare.chars().all(typeable) {
                        let held = bare.trim_matches(['<', '>', '{', '}']);
                        if !held.is_empty() && held.chars().all(typeable) {
                            words.push(hole(bare).to_string());
                            owed = false;
                            continue;
                        }
                        break;
                    }
                    let word = match bare.split_once('|') {
                        Some((first, _)) => first,
                        None => bare,
                    };
                    // A bracketed word that is not a flag, and does not follow
                    // one, is a hole the reader fills: `[n]`, `[name]`.
                    let optional = raw.contains(&format!("[{word}]"));
                    words.push(match optional && !word.starts_with('-') && !owed {
                        true => hole(word).to_string(),
                        false => word.to_string(),
                    });
                    owed = word.starts_with('-') && !word.contains('=');
                }
                let Some(first) = words.first() else {
                    continue;
                };
                if !judgeable || !vocab.contains(first.as_str()) {
                    // omh's own voice: `omh: …`, `omh has no record …`, and
                    // every sentence in a captured report that opens with the
                    // program's name.
                    continue;
                }
                *checked.entry(name.clone()).or_insert(0) += 1;
                let argv: Vec<String> = std::iter::once("omh".to_string())
                    .chain(words.iter().cloned())
                    .collect();
                let (_, argv) = cli::session_prefix(argv);
                let read = Cli::try_parse_from(&argv);
                if read.is_err() != inverted {
                    refused.push(format!(
                        "{}:{}: `omh {}` {}",
                        page.display(),
                        n + 1,
                        words.join(" "),
                        match read {
                            Err(e) => format!("→ {}", e.to_string().lines().next().unwrap_or("")),
                            Ok(_) => "parses, and the page says it is an error".into(),
                        }
                    ));
                }
            }
        }
        assert!(
            refused.is_empty(),
            "the docs print lines omh refuses: {refused:#?}\ncounts: {checked:#?}"
        );
        // An exact map, for the reason the sibling guard keeps one: a floor
        // notices a fall and nothing else, and the way a scan like this fails
        // is by quietly ceasing to recognise a shape. Every count below was
        // read off a run, and the two big ones are the pages a rename touches
        // first.
        //
        // The edit this costs is the point: it is where somebody reads what
        // the scan now sees. A page dropping out is loud, a page appearing is
        // loud, and a shape leaving one page while another gains lines is
        // loud — none of which a total or a floor can say.
        let expected: std::collections::BTreeMap<String, usize> = [
            ("README.md", 55),
            ("accounts.md", 4),
            ("adapters.md", 1),
            ("code-graph.md", 1),
            ("commands.md", 131),
            ("configuration.md", 44),
            ("decisions.md", 1),
            ("editors.md", 4),
            ("getting-started.md", 14),
            ("git.md", 18),
            ("memory.md", 5),
            ("profile.md", 3),
            ("sessions.md", 11),
            ("troubleshooting.md", 11),
            ("trust.md", 2),
        ]
        .into_iter()
        .map(|(f, n)| (f.to_string(), n))
        .collect();
        assert_eq!(
            checked, expected,
            "the set of documented command lines this scan reads has changed. \
             If you added or removed one, update the map. If you did not, a \
             cut stopped admitting a shape it used to."
        );
    }

    /// No doc comment has been spliced onto itself.
    ///
    /// A specific accident with a specific cause: these files are edited by
    /// matching an anchor and inserting before it, and an anchor chosen
    /// without reading the line above it lands the new text *inside* the
    /// previous item's doc block, and the sentence that was there ends up
    /// carrying a second doc marker partway along. It compiles, it renders as
    /// one run-on sentence, and it reads as deliberate.
    ///
    /// It shipped five times before anything looked for it, across four files
    /// and four separate changes. Nothing else in the suite can see it: the
    /// text is a comment, so no behaviour changes and no assertion fails.
    ///
    /// A doc comment that quotes the shape trips this — the first draft of
    /// this one did. Reword it; the check has no way to tell an example from
    /// the thing, and the thing is worth more than the example.
    ///
    /// It does not catch every splice, and this test was itself installed by
    /// one it cannot see: the insertion walked back over the *next* test's
    /// `#[test]` and left it stranded above this one. That shape is a
    /// duplicated attribute, which clippy already refuses — so the two
    /// together cover the accident, and only the comment half needed
    /// something new.
    #[test]
    fn no_doc_comment_was_spliced_onto_itself() {
        let mut doubled = Vec::new();
        let files = rust_sources(&["src", "tests"]);
        the_whole_tree(&files);
        {
            for file in &files {
                for (n, line) in std::fs::read_to_string(file).unwrap().lines().enumerate() {
                    // Two on one line. A doc comment is the whole line by
                    // construction, so a second one is always a splice — and
                    // `///` inside a string or a URL is a different shape
                    // (`http://`), which has two slashes and not three.
                    if line.matches("///").count() > 1 {
                        doubled.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            doubled.is_empty(),
            "doc comments spliced together: {doubled:#?}"
        );
    }

    /// A command list is scannable, and none of it is markdown.
    ///
    /// Two shapes, both read off clap's own rendering rather than off the doc
    /// comments, because what matters is what a person sees:
    ///
    /// - **One line per entry.** `omh use` and `omh sessions commit` carried
    ///   paragraph-length descriptions, so `omh --help` and `omh s --help` each
    ///   had one entry sprawling over four lines while every sibling was a
    ///   scannable line. The reasoning belongs in that command's own `--help`,
    ///   where somebody has asked for it.
    /// - **No markdown.** `omh settings --help` printed its emphasis markers
    ///   literally, asterisks and all. A doc comment on a clap item is rustdoc
    ///   *and* terminal output, and only one of them renders emphasis.
    #[test]
    fn a_command_list_is_scannable_and_none_of_it_is_markdown() {
        use clap::CommandFactory;

        fn walk(cmd: &clap::Command, path: &str, wide: &mut Vec<String>, marked: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                let here = format!("{path} {}", sub.get_name());
                // The line clap puts in the parent's command list.
                if let Some(about) = sub.get_about() {
                    let about = about.to_string();
                    // cmd::init::Measured, not guessed: on the tree where this was
                    // written the longest entry that reads as a line is
                    // `omh sessions rm` at 85 characters, and the two that
                    // read as paragraphs are 155 and 200. 100 sits in the gap
                    // with room either side.
                    if about.len() > 100 || about.contains('\n') {
                        wide.push(format!("{here}: {about}"));
                    }
                }
                for text in [sub.get_about(), sub.get_long_about()]
                    .into_iter()
                    .flatten()
                    .map(|t| t.to_string())
                {
                    if text.contains("**") || text.contains("*`") {
                        marked.push(format!("{here}: {text}"));
                    }
                }
                walk(sub, &here, wide, marked);
            }
        }

        let cmd = Cli::command();
        let (mut wide, mut marked) = (Vec::new(), Vec::new());
        walk(&cmd, "omh", &mut wide, &mut marked);
        assert!(
            wide.is_empty(),
            "a command list is read by scanning it, and these entries are \
             paragraphs — move the reasoning into their own --help: {wide:#?}"
        );
        assert!(
            marked.is_empty(),
            "a doc comment is terminal output as well as rustdoc, and the \
             terminal does not render emphasis: {marked:#?}"
        );
    }

    /// Every command line omh prints is one omh accepts.
    ///
    /// Deleting the positionals broke three printed suggestions at once, in
    /// three files — `omh s down {id}`, `omh s diff {id}` and *clear each with
    /// `omh s rm <id>`* — and every test stayed green, because a suggestion is
    /// a string until someone types it. One of them had been wrong since it was
    /// written for an unrelated reason, which is what advice nobody runs looks
    /// like.
    ///
    /// The line goes through `session_prefix` before the parser, because that
    /// is the path a typed line takes — checking it against `Cli` alone would
    /// call `omh s01 diff` a failure and `omh s diff s01` a success, both
    /// backwards.
    ///
    /// **What is read.** 130 command lines, from `src/` above each file's test
    /// module and from `base/*.toml`, where `omh why` prints a `remove` field
    /// verbatim as the way out of a feature. A line qualifies by sitting behind
    /// a backtick, the two-space gutter, the em dash the `import` report
    /// introduces a command with, or that field — or by opening with a word
    /// [`vocabulary`] knows, which is the only way to reach a literal that
    /// begins with a command.
    ///
    /// The shape left out is a literal beginning with a word omh does *not*
    /// know, because that is omh's error voice: `omh could not tell`, `omh
    /// ships`. Admitting it reads four more lines and refuses all four — three
    /// sentences and `shadow.rs`'s `AUTHOR_NAME`, the git identity the sandbox
    /// commits as. No defect among them.
    ///
    /// **Two cuts that are not about position.** A line whose first word omh
    /// does not know, run longer than `SENTENCE`, is prose. And a command named
    /// without the argument the reader supplies is prose — *"Add it with
    /// `omh settings mcp add`"* is not a line to type. clap says which is which, so
    /// nothing keeps a list; 14 lines sit in that second bucket.
    ///
    /// **Where the strictness is not uniform, and why.** That second excuse is
    /// withheld from any line naming a session, because that is precisely the
    /// class the rule this replaced asserted `is_ok()` on. Granting it there
    /// would be a weakening dressed as a widening — the first published version
    /// of this test did exactly that: making `diff`'s checkpoint required went
    /// from red under the old rule to green under the new one, with every
    /// published count still reading *wider*. A
    /// flag-terminated line is excused an owed value, `omh s commit -m` being
    /// the point of that one.
    ///
    /// **Read this before narrowing anything here.** Twice now a rewrite of
    /// this test has replaced a rule instead of adding to one, and twice the
    /// counts said *wider* while a class left the scan in silence — first 44
    /// printed lines, then the arity of every command a session line names.
    /// Neither was caught by the suite. The exact map below exists because of
    /// that, and so does the rule that an admission is only ever added.
    #[test]
    fn the_lines_omh_prints_are_lines_omh_accepts() {
        let mut files = rust_sources(&["src"]);
        the_whole_tree(&files);
        files.extend(manifests());
        // Counted apart from the file floor above: this asks whether the scan
        // still recognises the *lines* it was written to read, which is the
        // half that goes quiet when a verb is renamed.
        // The longest command omh accepts, counting the words after `omh`
        // once holes are filled — `omh import <skill> <name> --from <harness>`.
        // Longer than any command omh prints whose words are all its own.
        //
        // This replaces a cut that called itself "the longest command omh
        // accepts", which was false twice over: `omh memory remember` with its
        // three required options is eight words, and `McpCmd::Add` takes
        // `trailing_var_arg`, so what omh accepts has no length at all. What is
        // measurable is the corpus this scan reads, and even that is only half
        // the rule — a long line is called a sentence only when its first word
        // is neither a session nor a command omh knows, so a broken `config
        // mcp addd …` stays checked however long it runs.
        //
        // What that leaves unseen: a *misspelt* verb in a line this long,
        // behind nothing but a gutter. `omh cnofig mcp add a b c` would read as
        // prose. Short misspellings — the shape that has actually occurred
        // here — are checked.
        const SENTENCE: usize = 3;
        let vocab = vocabulary();
        let mut qualifying = 0;
        let mut checked: std::collections::BTreeMap<String, usize> = Default::default();
        let mut refused: Vec<String> = Vec::new();
        for file in &files {
            // A message wide enough to wrap is written with Rust's string
            // continuation, which eats the newline and the indent that follows
            // it. Read one line at a time, `omh s commit --keep` looks like
            // ``omh s commit \``, so the scan has to join what the compiler
            // joins before it reads anything.
            let body = std::fs::read_to_string(file).unwrap();
            let mut joined = String::new();
            let mut continued = false;
            for line in body.lines() {
                // What the compiler drops: the newline, and the indent the
                // next line begins with.
                let line = if continued { line.trim_start() } else { line };
                match line.strip_suffix('\\') {
                    // `\\\\` is an escaped backslash, not a continuation, and
                    // stripping one leaves the other — which still ends in a
                    // backslash, so this asks again rather than assuming.
                    Some(head) if !head.ends_with('\\') => {
                        joined.push_str(head);
                        continued = true;
                    }
                    _ => {
                        joined.push_str(line);
                        joined.push('\n');
                        continued = false;
                    }
                }
            }
            // Stop at the test module. Everything below it is assertion text
            // and fixtures, not anything omh prints — and counting it is not
            // harmless: 24 of the lines this scan called "printed" were test
            // strings, two of them *this test's own failure messages*, and
            // three of the per-file floors below sat above the number of real
            // printed lines in their file. A floor reached by counting the
            // guard's own prose measures nothing about omh.
            let body = match joined.find("\nmod tests {") {
                Some(at) => joined[..at].to_string(),
                None => joined,
            };
            // The manifest is read for one field. `omh why` prints `remove`
            // verbatim as the way out of a feature, and nothing else in that
            // file is a line anyone is being told to type — `because` and
            // `why` are paragraphs of prose that happen to name commands.
            // Keyed on the field name, so a floor below counts what it found:
            // renaming the field must fail loudly rather than read nothing.
            //
            // Eight of the eleven `remove` fields open with a command; the
            // other three open with prose (*"nothing to uninstall"*, a
            // settings key). Those are skipped by the delimiter rule wanting
            // the field's opening quote, not by anything knowing about them.
            let manifest = file.extension().is_some_and(|e| e == "toml");
            for raw in body.lines() {
                // Comments describe old spellings on purpose — `session_prefix`
                // documents all four it replaced. The manifest needs no such
                // skip: a `#` line cannot open with `remove `, so the gate just
                // below drops it anyway, and a second rule that fires on
                // nothing reads as protection that is not there.
                if !manifest && raw.trim_start().starts_with("//") {
                    continue;
                }
                if manifest {
                    // Anchored, and that is the whole of the manifest's
                    // protection. A rename of this field is caught because the
                    // gate then matches nothing and `2026.08.toml` contributes
                    // zero checked lines, which the floor below names. Relax it
                    // to `starts_with("remov")` and rename the field and the
                    // guard goes quiet — the two together are the mutation to
                    // run, since neither alone changes anything.
                    //
                    // A separate floor counting `remove` fields stood here and
                    // was removed: every counted command comes from one, so it
                    // could not fail where the floor below passed. It agreed
                    // with everything.
                    let field = raw.trim_start();
                    if !(field.starts_with("remove ") || field.starts_with("remove=")) {
                        continue;
                    }
                }
                for (at, _) in raw.match_indices("omh ") {
                    // Where a command starts, in four shapes.
                    //
                    // omh's failures are written in the third person and open a
                    // literal with the bare word — `omh could not read {}` — so
                    // a literal's *first* word is the one place `omh` is a
                    // subject rather than a command, and the only shape that
                    // has to be excluded. Everything below is a way of saying
                    // *something precedes it*.
                    //
                    // A column laid out inside a literal was going to be the
                    // third rule here, cutting at the last `\n` escape and
                    // asking whether what precedes the command is padding.
                    // cmd::init::Measured against the tree it admitted **nothing** the two
                    // rules below did not already admit — every column entry
                    // omh prints either names a session or follows a gutter.
                    // It is not here because a clause that admits nothing,
                    // sitting under a comment describing what it admits, is the
                    // exact defect this whole test was rewritten to remove.
                    let before = &raw[..at];
                    let trimmed = before.trim_end();
                    // **Mutating one of these looks like it proves nothing.**
                    // Disable any single arm below and the suite stays green,
                    // because on a tree with no defects every line they admit
                    // is also admitted by `names_a_command` — `omh s attach` is
                    // reachable by its backticks *and* by being a real verb.
                    //
                    // That is what they are for. A positional arm earns its
                    // place only on a line the vocabulary cannot vouch for,
                    // which is to say a broken one, which is to say never in a
                    // green tree. So the mutation has to be a pair: break a
                    // printed line *and* remove the arm that reaches its
                    // position. Do that and the map below drops that file's
                    // count and fails.
                    //
                    // Two clauses were deleted from this test for admitting
                    // nothing, and the difference is worth holding onto: those
                    // could not admit anything *even paired with a defect*.
                    // These can. A green single-mutation is not evidence of
                    // dead weight here, and reading it as such is how the arm
                    // that catches the next retired spelling gets removed.
                    let delimited =
                        // Quoted in prose: *stop it with `omh {} down`*.
                        trimmed.ends_with('`')
                        // The gutter omh puts between a sentence and a command
                        // it wants typed — *review with  omh {} diff* — and
                        // between a column entry and its explanation. Two
                        // spaces, the same separator the stop list below reads
                        // from the other end.
                        || before.ends_with("  ")
                        // The em dash omh introduces a command with, in the
                        // `import` report: *{name} has 3 hooks omh can read —
                        // omh import hooks {name}*. It was already a stop, so a
                        // command was being cut *at* one while never being
                        // admitted *after* one — an asymmetry with no argument
                        // behind it, and two real hints on the wrong side of it.
                        || trimmed.ends_with('—')
                        // `remove = "omh …"`, which `omh why` prints verbatim.
                        || (manifest && trimmed.ends_with('"'));
                    let rest = &raw[at + "omh ".len()..];
                    // A printed line ends where the message resumes: a newline
                    // escape, the end of the literal, backticked prose, or the
                    // column padding that lines an explanation up beside it.
                    //
                    // A comma stops only when a space follows it. Written bare
                    // it also fired inside `omh s commit --keep 1,3-4`, cutting
                    // it to `--keep 1` and deleting the range syntax that
                    // `session_prefix`'s own notes record as the case which
                    // settled that behaviour. `, ` is prose resuming; `,3` is
                    // part of the argument.
                    //
                    // `·` went altogether: it was the nearest stop for no
                    // extraction in the tree, because the one line carrying it
                    // is cut by the gutter first. A stop can only shorten, and
                    // a shorter command parses more easily, so a stop that
                    // earns nothing is a stop that can only hide.
                    // A stop can only shorten, and a shorter command is
                    // strictly likelier to parse — `omh settings mcp rm —
                    // codegraph` would be checked as `omh settings mcp rm` and
                    // tolerated as a missing argument. The em dash is here
                    // because the manifest separates a command from its
                    // explanation with one, and it is omh's most-used
                    // punctuation, so this is the stop most worth watching.
                    let end = ["\\n", "\"", "`", "—", "  ", ", "]
                        .iter()
                        .filter_map(|stop| rest.find(stop))
                        .min()
                        .unwrap_or(rest.len());
                    let line = rest[..end].trim();
                    // Placeholders stand for an id or a name; either way a
                    // session id is the value that makes the line whole.
                    let filled = regex_lite_fill(line);
                    let words: Vec<&str> = filled.split_whitespace().collect();
                    // Naming a session is no longer a way *in*. It was, and it
                    // was the rule this whole test began as — but `s` and
                    // `sessions` are subcommands like any other, so
                    // `names_a_command` already admits every line this used to,
                    // measured both on the tree as it stands and on one where
                    // the `s` alias has been retired. A second clause admitting
                    // nothing is the defect this test keeps being rewritten to
                    // remove; it does not get to stay just because it reads
                    // reassuringly.
                    //
                    // It survives for the verdict instead, where it decides how
                    // strictly a line is judged. That is load-bearing: see the
                    // excuse table below.
                    //
                    // `s01` rather than a pattern: the fill above has already
                    // turned every hole into that id, so a line naming any
                    // session names this one.
                    //
                    // **A literal id in a printed line is a trap.** `s01` is
                    // in the vocabulary, so `omh s01 attach zed` was admitted;
                    // written `s02` it is not a command's first word, the line
                    // leaves the scan entirely, and the map below drops by one
                    // — under a message inviting the reader to update the
                    // number. Nothing in the tree writes a literal id into a
                    // printed line today, which is the only reason this is a
                    // note and not a defect.
                    let names_a_session =
                        matches!(words.first(), Some(&"s" | &"sessions" | &"s01"));
                    let names_a_command = words.first().is_some_and(|w| vocab.contains(*w));
                    if !delimited && !names_a_command {
                        continue;
                    }
                    // A source line that types a retired spelling on purpose is
                    // a needle in the guard against one, not something omh
                    // prints. Same marker the retired-verb scan honours, and
                    // read off `raw` — `line` is the extracted command, which
                    // never carries it.
                    if raw.contains("types the retired verb on purpose") {
                        continue;
                    }
                    // The gate reads the *filled* command, because every
                    // line omh actually prints writes the id as a hole —
                    // `omh {id} down`, `omh {} resume`. Reading the literal
                    // instead dropped roughly twenty real messages out of the
                    // scan, including the two this file had just rewritten,
                    // while a comment claimed the scan had been widened. It
                    // was narrowed. The false positive that prompted that —
                    // `omh {arg} {}` inside `passthrough`, whose first hole is
                    // a flag — is gone with the function.
                    // An ellipsis means *whatever you were typing* — `omh s01
                    // …` stands for a line, not a line. Nothing else is dropped
                    // on the shape of its last word.
                    //
                    // A trailing flag used to be dropped here too, justified by
                    // the five `shadow.rs` messages that say *take the files as
                    // they stand with `omh s commit -m`*, where the value the
                    // reader supplies is the point. That reasoning covers the
                    // six lines ending in an option that owes a value. It threw
                    // away twenty-two more that owe nothing and are complete as
                    // printed — `omh use --all`, `omh s down --all`, `omh sNN
                    // log --turns`, `omh s commit --skip-carried`. Each was
                    // breakable with the whole suite green.
                    //
                    // So the shape of the last word is not the question; clap
                    // is asked instead, below, and the only excuse a
                    // flag-terminated line gets is the one that means *you owe
                    // a value here*.
                    let ends_in_a_flag = words.last().is_some_and(|w| w.starts_with('-'));
                    if words.last().is_some_and(|w| *w == "…") {
                        continue;
                    }
                    // Unknown and long is a sentence, not a command.
                    //
                    // omh indents prose under a heading with the same `\n  ` it
                    // indents a column with — *"omh has no rationale for this
                    // one"* — so position alone does not separate the error
                    // voice from a line to type. What separates them is what
                    // they are: a line whose first word omh does not know, run
                    // out longer than any command omh prints.
                    //
                    // Both halves are needed. Length alone called a broken six-
                    // word `omh settings mcp addd …` a sentence, which a reviewer
                    // planted and the guard swallowed. Vocabulary alone would
                    // dismiss `omh attatch`, which is the whole point.
                    //
                    // The assertion is the valve: a line skipped as prose that
                    // omh nonetheless *accepts* is a command this cut is now
                    // wrong about, and it says so rather than going quiet. It
                    // asks the same way the real check does, through
                    // `session_prefix` — asking `Cli` alone made it vacuous for
                    // every session line, since `omh s01 diff` never parses raw.
                    let a_sentence = !names_a_command && words.len() > SENTENCE;
                    if a_sentence {
                        let (_, probe) = cli::session_prefix(
                            std::iter::once("omh")
                                .chain(words.iter().copied())
                                .map(str::to_string)
                                .collect(),
                        );
                        assert!(
                            Cli::try_parse_from(&probe).is_err(),
                            "`omh {line}` is {} words and omh accepts it, so the cut at \
                             {SENTENCE} words is now wrong",
                            words.len()
                        );
                        continue;
                    }
                    let argv: Vec<String> = std::iter::once("omh")
                        .chain(words)
                        .map(str::to_string)
                        .collect();
                    let (_, argv) = cli::session_prefix(argv);
                    checked
                        .entry(under_src(file))
                        .and_modify(|n| *n += 1)
                        .or_insert(1usize);
                    // A command named without the argument the reader supplies
                    // is a sentence, not a suggestion — *"drop it with
                    // `omh set carry_in`"*. clap already tells the two apart,
                    // so nothing here has to keep a list of which is which:
                    // every way of naming a command omh does not have lands in
                    // some other kind.
                    // **Only where the rule this replaced did not assert.**
                    // That rule read session lines and demanded `is_ok()` of
                    // them, full stop. Tolerating a missing argument on those
                    // too is a weakening dressed as a widening: make `diff`'s
                    // checkpoint required and `omh s diff {id}` — one of the
                    // three lines the paragraph at the top names — goes from
                    // red under the old rule to green under this one. Every
                    // line the tolerance is for is a sentence in prose, and no
                    // sentence in prose opens with a session, so the gate costs
                    // nothing and closes the hole.
                    let excused: &[clap::error::ErrorKind] = match (names_a_session, ends_in_a_flag)
                    {
                        // `omh s commit -m` — the option is real and the value
                        // is the reader's to write.
                        (true, true) => &[clap::error::ErrorKind::InvalidValue],
                        // The same, plus prose naming a command without its
                        // arguments: `config::Layer::Shared` renders as the
                        // label `omh set --shared`, which is what to type
                        // *into*, not a line to paste.
                        (false, true) => &[
                            clap::error::ErrorKind::InvalidValue,
                            clap::error::ErrorKind::MissingRequiredArgument,
                        ],
                        // Prose naming a command, never a session.
                        (false, false) => &[clap::error::ErrorKind::MissingRequiredArgument],
                        // What the rule this replaced demanded, unchanged.
                        (true, false) => &[],
                    };
                    if let Err(e) = Cli::try_parse_from(&argv) {
                        if !excused.contains(&e.kind()) {
                            refused.push(format!(
                                "{}: `omh {line}` — {:?}",
                                under_src(file),
                                e.kind()
                            ));
                        }
                    }
                    qualifying += 1;
                }
            }
        }
        assert!(
            refused.is_empty(),
            "{} of the {qualifying} command lines omh prints are lines omh \
             does not accept: {refused:#?}",
            refused.len()
        );
        // The whole map, exactly — not a floor per file.
        //
        // Three earlier shapes of this assertion each failed the same way. A
        // total (`qualifying >= 40`) sat green while forty-four lines left the
        // scan. Per-file floors caught an arm going dark but sat above the real
        // printed lines in three files, reaching their numbers only by counting
        // this test's own assertion strings. And a floor, by construction, only
        // ever notices a *fall*.
        //
        // An exact map notices both directions and costs one edit when a
        // message is added or removed — which is the right price, because that
        // edit is where somebody reads what the scan now sees. A file dropping
        // out is loud; a file appearing is loud; a shape quietly leaving one
        // file while another gains lines is loud, and no floor can see that.
        let expected: std::collections::BTreeMap<String, usize> = [
            ("base/2026.08.toml", 19),
            ("src/auth.rs", 2),
            ("src/base.rs", 3),
            ("src/cli.rs", 14),
            ("src/cmd/auth.rs", 3),
            ("src/cmd/catalogue.rs", 12),
            ("src/cmd/harvest.rs", 19),
            ("src/cmd/init.rs", 6),
            // The fourth is `doctor`'s TLS-inspection warning. It is printed
            // to somebody whose sandbox cannot verify anything it fetches and
            // who has, by construction, not set `ca_cert` — so the command in
            // it is the only thing standing between them and a broken
            // sandbox, and `--local` being the spelling omh accepts is what
            // this scan is for.
            ("src/cmd/inspect.rs", 4),
            ("src/cmd/memory.rs", 1),
            ("src/cmd/session.rs", 10),
            ("src/cmd/settings.rs", 10),
            ("src/config.rs", 3),
            ("src/container.rs", 4),
            ("src/doctor.rs", 1),
            // Three. One is the `# `omh set --local ca_cert`` line `ca_layer`
            // writes into the generated Dockerfile, telling whoever reads the
            // recipe where the certificate came from. **Not** either of
            // `ca_for`'s refusals: neither contains a command line, so neither
            // is what this scan sees — which is what this comment said before,
            // about a reading nobody had taken.
            //
            // The other two are `why_the_build_failed`'s, one per arm.
            // `omh set --local ca_cert …` goes to somebody whose build just
            // died and who has no reason to suspect a setting exists, so a
            // command that does not parse would send them nowhere — and
            // `omh settings set` is the spelling that looks right while
            // writing a template nothing re-reads. `omh doctor` goes to
            // somebody who set one that did not work; it carries no `--local`
            // and is the shorter claim.
            ("src/image.rs", 3),
            ("src/main.rs", 8),
            ("src/memory.rs", 2),
            ("src/memory/ingest.rs", 2),
            ("src/notice.rs", 2),
            // `omh s down`, in the refusal when a live sandbox blocks the move
            // off the pre-2026.08 repo key.
            ("src/profile.rs", 1),
            ("src/render.rs", 1),
            ("src/report.rs", 14),
            ("src/rules.rs", 1),
            ("src/selection.rs", 1),
            ("src/session.rs", 4),
            ("src/shadow.rs", 11),
            ("src/ssh.rs", 1),
            ("src/stack.rs", 1),
            ("src/why.rs", 3),
        ]
        .into_iter()
        .map(|(f, n)| (f.to_string(), n))
        .collect();
        assert_eq!(
            checked, expected,
            "the set of command lines this scan reads has changed. If you added \
             or removed a printed line, update the map. If you did not, a cut \
             stopped admitting a shape it used to — which is how this test has \
             gone quiet three times."
        );
    }

    /// `{id}`, `{}`, `<id>`: whichever a message uses, fill it.
    ///
    /// The first placeholder is the session — that is the shape of every line
    /// this scan reads, since the session goes first. A later one is a value
    /// the verb takes, and `1` is the fill because it is valid where a number
    /// is required and harmless where a word is. Filling every slot with a
    /// session id was the first version, and it turned the real hint
    /// `omh {} diff {}` into `omh s01 diff s01` — refused for a reason that was
    /// the test's own doing rather than the line's.
    fn regex_lite_fill(line: &str) -> String {
        // `sNN` is how omh writes *any session*, in fifty-odd places across
        // the tree — a hole spelled without braces. Nearly all of them sit in
        // comments the scan discards before reaching here; exactly one printed
        // line depends on this, `omh sNN sync` in `doctor.rs`, and without the
        // fill that line reads as a launch of a harness called `sNN`.
        //
        // It does *not* consume the first-hole slot. A guard against that was
        // written and then removed for admitting nothing: it would matter only
        // to a printed line carrying both an `sNN` and a `{}`, and the only
        // such string in the tree is inside a comment this scan discards. Set
        // it either way and every line reads the same.
        let mut first = true;
        let line: String = line
            .split_whitespace()
            .map(|w| if w == "sNN" { "s01" } else { w })
            .collect::<Vec<_>>()
            .join(" ");
        let line = line.as_str();
        let mut out = String::new();
        let mut rest = line;
        while let Some(open) = rest.find(['{', '<']) {
            let close = if rest.as_bytes()[open] == b'{' {
                '}'
            } else {
                '>'
            };
            let Some(shut) = rest[open..].find(close) else {
                break;
            };
            out.push_str(&rest[..open]);
            out.push_str(if first { "s01" } else { "1" });
            first = false;
            rest = &rest[open + shut + 1..];
        }
        out.push_str(rest);
        out
    }

    /// A leading session id is found wherever the globals leave it.
    ///
    /// It was `argv.get(1)` and nothing else, so the id had to be the literal
    /// first word. Every global is declared `global = true` and clap takes them
    /// anywhere — so `omh --json s01 log`, which is precisely what a script
    /// wants, got clap's `unrecognized subcommand 's01'` and the actively wrong
    /// tip *a similar subcommand exists: 's'*.
    ///
    /// Three of the five globals take a value, and that value must not be read
    /// as the verb — `-s s02` is a session named the other way, not a prefix.
    ///
    /// A unit test rather than a CLI one: `session_prefix` is pure, and this is
    /// a question about argv rather than about a session existing.
    #[test]
    fn a_leading_session_id_survives_the_flags_in_front_of_it() {
        for front in [
            vec!["--json"],
            vec!["--dry-run"],
            vec!["--color", "never"],
            vec!["--color=never"],
            vec!["--json", "--dry-run"],
            vec!["--dry-run", "--color", "always", "--json"],
        ] {
            let line: Vec<&str> = front.iter().copied().chain(["s01", "diff"]).collect();
            let want: Vec<&str> = front.iter().copied().chain(["s", "diff"]).collect();
            assert_eq!(
                cli::session_prefix(cli_argv(&line)),
                (Some("s01".to_string()), cli_argv(&want)),
                "`omh {}` names a session",
                line.join(" ")
            );
        }
    }

    /// A flag's value is not the verb, and not a session either.
    ///
    /// The reason the step over the globals has to know which of them take a
    /// value: read naively, `omh -s s02 diff` has `s02` where the prefix would
    /// be. It is the flag's argument, `the_one_session` already reconciles the
    /// two spellings, and treating it as a prefix would hand that function two
    /// names for one session on every ordinary invocation.
    #[test]
    fn a_value_belonging_to_a_flag_is_not_a_session_prefix() {
        for line in [
            vec!["-s", "s02", "diff"],
            vec!["--session", "s02", "diff"],
            vec!["-a", "s02", "info"],
            vec!["--account", "s02", "info"],
        ] {
            assert_eq!(
                cli::session_prefix(cli_argv(&line)),
                (None, cli_argv(&line)),
                "`omh {}` names its session through the flag",
                line.join(" ")
            );
        }
    }

    /// Anything that is not `sNN` is left exactly as it was.
    #[test]
    fn a_command_that_is_not_a_session_is_not_read_as_one() {
        for line in [
            vec!["s", "diff"],
            vec!["init"],
            vec!["claude"],
            vec!["sessions", "log"],
            // a harness whose name merely starts with s
            vec!["sourcegraph"],
        ] {
            assert_eq!(
                cli::session_prefix(cli_argv(&line)),
                (None, cli_argv(&line)),
                "{line:?} is not a session prefix"
            );
        }
    }

    /// Naming the session twice is a question, not something to resolve.
    ///
    /// Every spelling of the flag, because the guard this replaces scanned argv
    /// for the two whole tokens `--session` and `-s` and let `--session=s02`
    /// and `-ss02` through — and letting one through is worse than having no
    /// guard, since the line then runs against whichever id won.
    #[test]
    fn a_session_named_twice_is_refused_rather_than_picked() {
        for flag in [
            vec!["--session", "s02"],
            vec!["--session=s02"],
            vec!["-s", "s02"],
            vec!["-ss02"],
        ] {
            let line: Vec<&str> = std::iter::once("s01")
                .chain(flag.iter().copied())
                .chain(std::iter::once("diff"))
                .collect();
            let (prefix, argv) = cli::session_prefix(cli_argv(&line));
            let parsed = Cli::try_parse_from(&argv)
                .unwrap_or_else(|e| panic!("{line:?} has to reach the parser: {e}"));
            let err = cli::the_one_session(prefix, parsed.session)
                .expect_err("two names for one session is not something to guess at");
            assert!(
                err.to_string().contains("s01") && err.to_string().contains("s02"),
                "the refusal has to name both, for {line:?}: {err}"
            );
        }
    }

    /// A harness's own `-s` still is not omh naming the session twice.
    ///
    /// The pair above and this one are the two halves of one boundary, and a
    /// guard that satisfies either alone is easy to write: refusing the whole
    /// line refuses this launch, and refusing nothing lets the pair above
    /// through. Read through the parser they separate themselves — everything
    /// after a harness name is the harness's argv, so the flag never becomes
    /// omh's `session`.
    #[test]
    fn a_harness_flag_is_still_not_omh_naming_the_session_twice() {
        let (prefix, argv) =
            cli::session_prefix(cli_argv(&["s01", "resume", "claude", "--", "-s", "some"]));
        let parsed = Cli::try_parse_from(&argv).expect("a launch is a valid line");
        assert_eq!(
            cli::the_one_session(prefix, parsed.session).unwrap(),
            Some("s01".to_string()),
            "the session stays the prefix's; a flag past the separator is the harness's"
        );
    }

    /// The log reports the session it was asked about, from that session's own
    /// sandbox repository.
    ///
    /// Nothing reached this wiring before it was split out of `log_cmd`:
    /// replacing the read with an empty list left the whole suite green, and
    /// `omh sNN log` would have answered *the agent has not committed
    /// anything* for every session on earth. The shadow tests call
    /// `checkpoints` directly and the report tests build the value by hand, so
    /// both halves stayed correct while nothing joined them.
    ///
    /// Two sessions with different work in them, because with one the wiring
    /// cannot be wrong in a way this notices.
    #[test]
    fn the_log_reads_the_named_sessions_own_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .expect("git must be installed to run this test");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "root"]);
        std::fs::create_dir_all(paths.shadows()).unwrap();

        let mut sessions = Vec::new();
        for (id, subject) in [("s01", "Only in s01"), ("s02", "Only in s02")] {
            let session = Session::new(&paths.worktrees().join(id), id.to_string());
            session.ensure(&paths.repo, "main").unwrap();
            let shadow = shadow::Shadow::new(&paths.shadows(), id);
            shadow.ensure(&session.worktree, &[]).unwrap();
            std::fs::write(session.worktree.join("work.rs"), format!("// {subject}\n")).unwrap();
            // Through the sandbox's own repository, the way the agent commits.
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", subject],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
            sessions.push(session);
        }

        for (session, mine, theirs) in [
            (&sessions[0], "Only in s01", "Only in s02"),
            (&sessions[1], "Only in s02", "Only in s01"),
        ] {
            let log = cmd::harvest::log_report(&paths, session, false, &out::Ctx::plain()).unwrap();
            let subjects: Vec<&str> = log
                .read
                .commits
                .iter()
                .map(|c| c.subject.as_str())
                .collect();
            assert!(
                subjects.contains(&mine) && !subjects.contains(&theirs),
                "{}'s log is {}'s work: {subjects:?}",
                session.id,
                session.id
            );
        }
    }

    /// `--keep` binds a value only when one is given.
    ///
    /// `num_args = 0..=1` with `default_missing_value` is the clap shape most
    /// prone to silently binding the wrong thing — a following flag, or a
    /// following verb — and the three forms are one assertion each.
    #[test]
    fn keep_takes_a_selection_or_nothing_and_never_the_next_word() {
        let parse = |args: &[&str]| {
            let argv: Vec<String> = std::iter::once("omh")
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect();
            match Cli::try_parse_from(&argv).map(|cli| cli.cmd) {
                Ok(Cmd::Sessions {
                    cmd: Some(SessionsCmd::Commit { keep, edit, .. }),
                }) => Ok((keep, edit)),
                Ok(_) => panic!("{args:?} did not parse as a commit"),
                Err(e) => Err(e.to_string()),
            }
        };

        assert_eq!(parse(&["s", "commit"]).unwrap(), (None, false));
        assert_eq!(
            parse(&["s", "commit", "--keep"]).unwrap(),
            (Some(String::new()), false),
            "a bare --keep is not a missing value"
        );
        assert_eq!(
            parse(&["s", "commit", "--keep", "1,3-4"]).unwrap(),
            (Some("1,3-4".to_string()), false)
        );
        assert_eq!(
            parse(&["s", "commit", "--keep", "--edit"]).unwrap(),
            (Some(String::new()), true),
            "--keep does not swallow the flag after it"
        );
        assert!(
            parse(&["s", "commit", "--edit"])
                .unwrap_err()
                .contains("--keep"),
            "--edit is about the list --keep takes"
        );
        assert!(
            parse(&["s", "commit", "--keep", "-m", "x"])
                .unwrap_err()
                .contains("cannot be used with"),
            "and squashing is the other way to land work, not a modifier of this one"
        );
    }

    /// A commit refuses over markers a sync left behind, says where they are,
    /// and can be meant anyway.
    #[test]
    fn a_commit_will_not_land_an_unresolved_conflict_unless_it_is_meant() {
        let none: Vec<String> = vec![];
        assert!(
            cmd::harvest::may_commit("s01", &none, false).is_ok(),
            "a clean tree commits"
        );

        let one = vec!["src/tap.rs:12: leftover conflict marker".to_string()];
        let said = cmd::harvest::may_commit("s01", &one, false)
            .unwrap_err()
            .to_string();
        assert!(
            said.contains("src/tap.rs:12"),
            "it says where, so the user is not sent hunting: {said}"
        );
        assert!(
            said.contains("1 conflict marker in"),
            "and one is not `1 conflict markers`: {said}"
        );
        assert!(
            said.contains("--force"),
            "and the way past is in the refusal: {said}"
        );
        assert!(
            cmd::harvest::may_commit("s01", &one, true).is_ok(),
            "`--force` means it"
        );

        // A whole-file conflict is one line per hunk. The refusal has to stay
        // readable at that size, or the way past scrolls off the screen.
        let many: Vec<String> = (1..=40)
            .map(|n| format!("src/big.rs:{n}: leftover conflict marker"))
            .collect();
        let said = cmd::harvest::may_commit("s01", &many, false)
            .unwrap_err()
            .to_string();
        assert_eq!(
            said.matches("leftover conflict marker").count(),
            5,
            "five lines, not forty: {said}"
        );
        assert!(
            said.contains("40 conflict markers") && said.contains("…and 35 more"),
            "and the count is still the whole truth: {said}"
        );
    }

    /// omh does not act on a container question it could not answer.
    ///
    /// The decision points all read the same way — start it, enter it, stop
    /// it, sync over it — and each is worse done blind than not done. The one
    /// that made this worth changing is `sync`: it asks whether the sandbox is
    /// up to decide whether writing files would land under a live agent, and
    /// an unreachable runtime used to answer *nothing is running*.
    #[test]
    fn a_container_question_omh_could_not_answer_stops_the_command() {
        assert!(cmd::harvest::must_know(image::Running::Yes, "s01", "sync over it").unwrap());
        assert!(!cmd::harvest::must_know(image::Running::No, "s01", "sync over it").unwrap());

        let refused = cmd::harvest::must_know(
            image::Running::Unknown("daemon not reachable".into()),
            "s01",
            "sync over it",
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("sync over it"),
            "it says what it declined to do: {refused}"
        );
        assert!(
            refused.contains("daemon not reachable"),
            "and the runtime's reason, which is the only actionable part: {refused}"
        );
        // The subject is a parameter because two of the callers are `graph`,
        // which asks about a per-repo UI container and not a session. The
        // sentence used to say "the sandbox" for those, sending the reader to
        // look at sessions.
        assert!(
            refused.contains("s01") && !refused.contains("the sandbox"),
            "and names what it asked about: {refused}"
        );
        let graph = cmd::harvest::must_know(
            image::Running::Unknown("daemon not reachable".into()),
            "the graph",
            "stop it",
        )
        .unwrap_err()
        .to_string();
        assert!(graph.contains("the graph is running"), "got: {graph}");
    }

    /// The reaper's inverted safe direction, which is the one place *could not
    /// tell* must not lead to a refusal — there is nobody to refuse to.
    ///
    /// A named predicate rather than a `matches!` inside a filter, because the
    /// mutation that matters — `Unknown` becoming reapable, so a flapping
    /// daemon stops live agents' sandboxes on a guess — left the whole suite
    /// green while it was inline.
    #[test]
    fn a_session_omh_cannot_ask_about_is_never_reaped() {
        assert!(
            cmd::harvest::reapable(&image::Running::Yes),
            "a live one may be reaped"
        );
        assert!(
            !cmd::harvest::reapable(&image::Running::No),
            "one already down has nothing to reap"
        );
        assert!(
            !cmd::harvest::reapable(&image::Running::Unknown("daemon down".into())),
            "and one omh could not ask about is left alone — stopping a live \
             session on a guess costs somebody's turn"
        );
    }

    /// Turn snapshots are named when a session is removed, and are never the
    /// reason it is refused.
    ///
    /// The judgement this encodes, since it is not the only defensible one. A
    /// snapshot is a tree omh photographed at the end of a turn, and there is
    /// one for nearly every session that ever ran — so refusing over them
    /// would make this guard fire almost always, and a guard that fires almost
    /// always is answered with `--force` unread. That is how it would stop
    /// protecting the agent's own commits, which is what it was built for.
    ///
    /// They are still said, because they do go, and because the one time they
    /// matter is the one time the agent threw the tree away.
    #[test]
    fn a_removal_names_the_turn_snapshots_it_takes_without_refusing_over_them() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        // Harvested first, so nothing of the agent's own is at stake — which
        // is the branch this decision is really about, and the one `force:
        // true` would have skipped straight past.
        shadow
            .harvest(
                &paths.repo,
                &session.worktree,
                "omh/s01",
                &[],
                shadow::Keep::All,
            )
            .unwrap();

        let note =
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::Kept(12), false)
                .expect("snapshots alone never stop a removal")
                .expect("but they are said");
        assert!(
            note.contains("12 turn snapshots") && note.contains("log --turns"),
            "named, with the way to read them: {note}"
        );

        assert_eq!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
                .unwrap(),
            None,
            "and a session that has none says nothing about them"
        );

        // With the agent's own commits at stake it still refuses — that is
        // the guard this must not have weakened — and the snapshots ride along
        // in the same message rather than displacing it.
        let (paths, unkept, _shadow) = a_session_with_two_checkpoints();
        let refused =
            cmd::harvest::may_remove(&paths, &unkept, cmd::harvest::Snapshots::Kept(3), false)
                .expect_err("unharvested commits still refuse")
                .to_string();
        assert!(
            refused.contains("commit --keep"),
            "the refusal is unchanged: {refused}"
        );
        assert!(
            refused.contains("and 3 turn snapshots"),
            "and says what else goes: {refused}"
        );
        // The rule the rest of this message keeps: everything under the first
        // line is a command, so the snapshots are mentioned in the sentence
        // and the way to read them is offered as one.
        assert!(
            refused.contains("omh s01 log --turns"),
            "with a command for it: {refused}"
        );
        for line in refused
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            assert!(
                line.starts_with("omh s01 "),
                "a line the user cannot paste: {line}"
            );
        }
    }

    /// A sync that cannot leave its note is still a sync, and says which.
    ///
    /// The design decision the call site argues for, asserted rather than
    /// commented: the merge has landed, the baseline has moved and the shadow
    /// has its commit by the time the note is written, so failing here would
    /// report finished work as a failed command. The other half — that the
    /// user hears about it — is what carrying it on the report rather than
    /// printing it inside makes checkable at all.
    #[test]
    fn a_sync_whose_note_cannot_be_written_still_succeeds_and_says_so() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let repo_git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        repo_git(&["checkout", "-q", "main"]);
        std::fs::write(paths.repo.join("from-trunk.rs"), "fn trunk() {}\n").unwrap();
        repo_git(&["add", "-A"]);
        repo_git(&["commit", "-qm", "trunk moved"]);

        // A directory where the note goes. Contrived, and the reachable
        // versions — a full disk, a permission the host process does not have
        // — are not things a test can arrange on demand.
        std::fs::create_dir(shadow::note_file(&shadow.gitdir)).unwrap();

        let synced =
            cmd::harvest::sync_session(&paths, &session, "main").expect("the sync itself is fine");
        assert_eq!(synced.moved, 1, "the work still arrived");
        assert!(
            session.worktree.join("from-trunk.rs").exists(),
            "and is on disk, which is what a failed command would deny"
        );
        let why = synced.note.expect("the failure is carried, not swallowed");
        assert!(
            why.contains("omh-note"),
            "naming the file it could not write: {why}"
        );
    }

    /// A sync brings trunk over, explains itself in the sandbox, and leaves
    /// the agent's work where a harvest can still take it.
    ///
    /// The whole mechanism end to end, because every part of it is only worth
    /// anything in combination: a merged file the agent never asked for, a
    /// commit that says what moved, a baseline that makes `diff` mean the
    /// agent's work again — and the checkpoint that makes all of it
    /// recoverable.
    ///
    /// The last assertion is the one that decides a design question. The spec
    /// says to advance the replay point past the commit omh writes, so a
    /// harvest never replays trunk's changes as the agent's. That cannot be
    /// done with a single ancestor pointer without also marking the checkpoint
    /// *below* it as handed over — and that checkpoint is the agent's own
    /// uncommitted work. So the replay point stays where it was, and this
    /// asserts what the spec was protecting: after a sync, `--keep` puts the
    /// agent's work on the branch and does not land trunk's twice.
    #[test]
    fn a_sync_brings_trunk_over_and_leaves_the_agents_work_harvestable() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let repo_git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        // The agent leaves something uncommitted, so the checkpoint has work.
        std::fs::write(session.worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();

        // Trunk moves: one file the session never touched.
        let on_session = repo_git(&["rev-parse", "HEAD"]);
        repo_git(&["checkout", "-q", "main"]);
        std::fs::write(paths.repo.join("from-trunk.rs"), "fn trunk() {}\n").unwrap();
        repo_git(&["add", "-A"]);
        repo_git(&["commit", "-qm", "trunk moved"]);
        let onto = repo_git(&["rev-parse", "HEAD"]);
        let _ = on_session;

        let synced = cmd::harvest::sync_session(&paths, &session, "main").unwrap();
        assert_eq!(synced.moved, 1, "one commit arrived");
        assert!(synced.conflicted.is_empty(), "and it merged cleanly");
        assert!(synced.checkpoint, "the uncommitted work was checkpointed");

        // 1. The file trunk added is in the session now.
        assert!(
            session.worktree.join("from-trunk.rs").exists(),
            "trunk's file reached the session"
        );
        // 2. The branch sits on the new base.
        assert_eq!(
            repo_git(&["rev-parse", "omh/s01"]),
            onto,
            "the baseline moved, so `diff` measures the agent's work and not trunk's"
        );
        // 3. The sandbox can read what happened.
        let sandbox_log = shadow.checkpoints(&session.worktree).unwrap();
        let subjects: Vec<&str> = sandbox_log
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert!(
            subjects.iter().any(|s| s.starts_with("base moved to")),
            "a commit the agent can read: {subjects:?}"
        );
        assert!(
            subjects.iter().any(|s| s.contains("Before omh brought")),
            "and the point it can be undone from: {subjects:?}"
        );

        // 4. The sentence the agent is given when it starts again — at the
        //    host path that is the mount's other end, since a note written
        //    anywhere else is delivered to nobody, in silence, forever.
        let note = std::fs::read_to_string(shadow::note_file(&shadow.gitdir))
            .expect("a note was left where the hook reads");
        assert!(
            note.contains("main moved 1 commit") && note.contains("git show HEAD"),
            "what moved, and where to read it: {note}"
        );
        assert_eq!(
            synced.note, None,
            "and nothing to report about leaving it: {synced:?}"
        );

        // 5. …and the agent's work is still there to be taken.
        let landed = shadow
            .harvest(
                &paths.repo,
                &session.worktree,
                "omh/s01",
                &[],
                shadow::Keep::All,
            )
            .unwrap()
            .landed;
        let on_branch = repo_git(&["log", "--format=%s", &format!("{onto}..omh/s01")]);
        assert!(landed > 0, "the harvest took something: {on_branch}");
        assert!(
            on_branch.contains("one") && on_branch.contains("two"),
            "the agent's own commits reached the branch: {on_branch}"
        );
        assert_eq!(
            on_branch.matches("base moved to").count(),
            0,
            "and trunk's changes did not arrive a second time as the agent's: {on_branch}"
        );
    }

    /// A session holding work no branch has is not removed by accident.
    ///
    /// The last piece of [risks](../docs/design/risks.md) 2c. The branch
    /// survives a removal and the worktree's files were on disk until it ran,
    /// but the agent's own commits live only in the sandbox's repository and
    /// `reap` deletes it. omh could not ask this until `log` learned to count.
    #[test]
    fn a_session_holding_unkept_work_is_not_removed_without_being_asked() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        let err = cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
            .expect_err("two commits are on no branch anywhere");
        let said = err.to_string();
        assert!(
            said.contains("s01 has 2 commits that no branch has"),
            "it says how much is at stake, and agrees with itself about the number: {said}"
        );
        // Every line offered is a command, and every one names this session —
        // a refusal that walks the user to `--force` by elimination is worse
        // than no refusal.
        for line in said
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            assert!(
                line.starts_with("omh s01 "),
                "a line the user cannot paste: {line}"
            );
        }
        assert!(
            said.contains("--keep") && said.contains("commit -m") && said.contains("--force"),
            "put it on the branch, take the files as they stand, or mean it: {said}"
        );
        assert!(
            said.contains("omh/s01"),
            "and names the branch it would go on: {said}"
        );
        assert!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, true).is_ok(),
            "`--force` means it"
        );

        // Everything handed over: nothing to warn about.
        let head = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.gitdir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        std::fs::write(
            &shadow.landed_record,
            String::from_utf8_lossy(&head.stdout).trim(),
        )
        .unwrap();
        assert!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
                .is_ok(),
            "a session whose work is all on the branch removes quietly"
        );
    }

    /// Work the agent threw away is still work only this repository has.
    ///
    /// `reset --hard` is one of the four commands the sandbox's own git exists
    /// to give back, and the first version of this guard was blind to it:
    /// `seed..HEAD` counts 0 afterwards, so `rm` removed three commits without
    /// a word — the exact scenario `risks.md` cites as the reason the guard
    /// exists. cmd::init::Measured: `--all --reflog` still finds them.
    ///
    /// The same read covers a side branch the agent wandered off, which
    /// `preflight` refuses a *harvest* over while `rm` was dropping it for
    /// good.
    #[test]
    fn work_the_sandbox_can_still_reach_counts_however_the_agent_left_it() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let sandbox_git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let seed = shadow.seed().unwrap();

        // The agent throws its own work away.
        sandbox_git(&["reset", "-q", "--hard", &seed]);
        assert!(
            shadow
                .checkpoints(&session.worktree)
                .unwrap()
                .commits
                .is_empty(),
            "the numbered list cannot see them — that is why this guard reads wider"
        );
        let err = cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
            .expect_err("two commits are still in there and on no branch");
        assert!(err.to_string().contains("2 commits"), "{err}");

        // …and when the replay point no longer reaches, the count widens back
        // to the seed rather than trusting a record the history has left
        // behind. Narrower would mean counting from a commit this repository
        // cannot place, which is how work goes missing from a number someone
        // is about to act on.
        std::fs::write(&shadow.landed_record, "0".repeat(40)).unwrap();
        let err = cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
            .expect_err("a record naming nothing this repository has is not an answer");
        assert!(
            err.to_string().contains("cannot say what that removes"),
            "omh says it cannot tell rather than counting from a point it cannot place: {err}"
        );
        std::fs::remove_file(&shadow.landed_record).unwrap();

        // …and the same for a branch it wandered off.
        sandbox_git(&["checkout", "-q", "-b", "spike"]);
        std::fs::write(session.worktree.join("spike.rs"), "fn spike() {}\n").unwrap();
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "a spike"]);
        sandbox_git(&["checkout", "-q", "-"]);
        let err = cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
            .expect_err("three now");
        assert!(err.to_string().contains("3 commits"), "{err}");
    }

    /// What the seed record alone settles — including the arm a filesystem
    /// test cannot reach.
    ///
    /// `chmod 000` does not stop uid 0, so a permissions arm asserted through
    /// a real file passes vacuously wherever the suite runs as root. It is
    /// also the arm that silently deleted a sandbox: `Path::exists` answers
    /// `false` for a permissions error exactly as it does for absence.
    #[test]
    fn what_the_seed_record_settles_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let shadow = shadow::Shadow::new(dir.path(), "s01");
        let missing = || std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");

        assert!(
            cmd::harvest::from_the_seed_record(Ok(()), true, &shadow).is_none(),
            "a record omh can read settles nothing on its own — the repository decides"
        );
        assert!(
            matches!(
                cmd::harvest::from_the_seed_record(Err(missing()), false, &shadow),
                Some(cmd::harvest::AtStake::Nothing)
            ),
            "no record and no repository: nothing ever ran here"
        );
        // What `reap` leaves when `remove_dir_all` fails on a live mount and
        // the seed file goes anyway. `log` refuses to show this one.
        assert!(
            matches!(
                cmd::harvest::from_the_seed_record(Err(missing()), true, &shadow),
                Some(cmd::harvest::AtStake::Unknown(_))
            ),
            "a repository with no record of its start is not an empty session"
        );
        // The arm that mattered.
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        match cmd::harvest::from_the_seed_record(Err(denied), true, &shadow) {
            Some(cmd::harvest::AtStake::Unknown(why)) => assert!(
                why.contains("denied"),
                "the reason reaches the user rather than being read as absence: {why}"
            ),
            other => panic!("a record omh could not read is not an empty session: {other:?}"),
        }
    }

    /// A sandbox omh cannot read is asked about, not assumed empty.
    ///
    /// Three states, each of which the first version deleted in silence. The
    /// test it replaces asserted the opposite — it took the one harmless
    /// member of the class (the repository is *gone*, so nothing is left to
    /// lose) and generalised it to the whole class, which locked the collapse
    /// in.
    #[test]
    fn a_sandbox_omh_cannot_read_is_asked_about_rather_than_assumed_empty() {
        // A truncated replay record. `landed` bails on it deliberately: the
        // write truncates before it writes, so a process killed in that window
        // leaves zero bytes — and the session has demonstrably been harvested,
        // so the repository demonstrably holds commits.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::write(&shadow.landed_record, "").unwrap();
        let err = cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
            .expect_err("omh cannot tell what landed — that is a reason to ask");
        assert!(
            err.to_string().contains("cannot say what that removes"),
            "it says it cannot tell, rather than naming a count it does not have: {err}"
        );
        assert!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, true).is_ok(),
            "and `--force` is still the way past, so nobody is trapped"
        );

        // A repository with no record of where it started — what `reap` leaves
        // when `remove_dir_all` fails on a live mount and the seed file goes
        // anyway. `log` refuses to *show* this one; `rm` would have deleted it.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        assert!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
                .is_err(),
            "a repository omh cannot place is not an empty session"
        );

        // Gone entirely: nothing is left to lose, and `rm` must not stand in
        // the way of clearing up.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::remove_dir_all(&shadow.gitdir).unwrap();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        assert!(
            cmd::harvest::may_remove(&paths, &session, cmd::harvest::Snapshots::None, false)
                .is_ok(),
            "nothing there is nothing to lose"
        );

        // And a session whose sandbox never ran at all.
        let never_ran = Session::new(&paths.worktrees().join("s02"), "s02".to_string());
        assert!(
            cmd::harvest::may_remove(&paths, &never_ran, cmd::harvest::Snapshots::None, false)
                .is_ok()
        );
    }

    /// The count says what it counted, and says it in the singular when there
    /// is one of it.
    ///
    /// Asserted on the value rather than the sentence: `contains("1 commit")`
    /// is satisfied by `"1 commits"`, so a hardcoded plural survives every
    /// string assertion in this file.
    #[test]
    fn one_of_a_thing_is_said_in_the_singular() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let landed = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.gitdir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        std::fs::write(
            &shadow.landed_record,
            String::from_utf8_lossy(&landed.stdout).trim(),
        )
        .unwrap();

        std::fs::write(session.worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();
        assert!(
            matches!(cmd::harvest::at_stake(&paths, &session), cmd::harvest::AtStake::Work(what) if what == "1 uncommitted path"),
            "one path, said once"
        );

        let sandbox_git = |args: &[&str]| {
            Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap()
        };
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "one more"]);
        assert!(
            matches!(cmd::harvest::at_stake(&paths, &session), cmd::harvest::AtStake::Work(what) if what == "1 commit"),
            "and one commit, said once"
        );
    }

    /// Naming checkpoints needs a git that can drop an already-applied
    /// commit, and says so rather than handing over a usage dump.
    ///
    /// Injected, because this machine's git *has* the option: probed inline,
    /// deleting the guard changed nothing any test could see.
    #[test]
    fn a_selection_on_a_git_that_cannot_do_it_says_which_command_still_works() {
        let (_paths, session, shadow) = a_session_with_two_checkpoints();

        let err = cmd::harvest::what_to_keep(&shadow, &session, "1", false, false, &|| Ok(false))
            .expect_err("this git cannot take a selection");
        assert!(
            err.to_string().contains("--empty"),
            "the refusal names what git is missing: {err}"
        );
        assert!(
            err.to_string().contains("--keep"),
            "and what still works without it: {err}"
        );
        assert!(
            cmd::harvest::what_to_keep(&shadow, &session, "1", false, false, &|| Ok(true)).is_ok(),
            "and a git that can, does"
        );
        // `--keep` on its own — and with `--edit` — asks nothing of git that
        // omh has not always asked, so neither may be refused for this. They
        // must not even ask: the probe forks a process, and returning `Err`
        // from it here proves it was never called.
        let never = || -> Result<bool> { panic!("`--keep` asked git a question it does not need") };
        assert!(cmd::harvest::what_to_keep(&shadow, &session, "", false, false, &never).is_ok());
        assert!(cmd::harvest::what_to_keep(&shadow, &session, "", true, true, &never).is_ok());

        // The question comes before the list is read, so a number that is also
        // wrong reports the git first — there is no point telling someone
        // which checkpoint they meant on a git that cannot take any.
        let err = cmd::harvest::what_to_keep(&shadow, &session, "9", false, false, &|| Ok(false))
            .expect_err("this git cannot take a selection");
        assert!(
            err.to_string().contains("--empty"),
            "the git is the answer, not the number: {err}"
        );

        // And *could not ask* is neither yes nor no.
        let err = cmd::harvest::what_to_keep(&shadow, &session, "1", false, false, &|| {
            Err(anyhow::anyhow!("no git on PATH"))
        })
        .expect_err("omh could not tell");
        // Through `problem`, because that is how an error reaches a person:
        // omh's sentence is the headline and git's reason is the cause under
        // it. `to_string()` alone shows only the outer half and would pass for
        // an error that had thrown git's words away.
        let printed = out::problem(&out::Palette::plain(), &err);
        assert!(
            printed.contains("no git on PATH"),
            "git's own reason reaches the user: {printed}"
        );
        assert!(
            !printed.contains("newer git"),
            "and omh does not invent a diagnosis it cannot support: {printed}"
        );
    }

    /// The host's answers come after the sandbox's, so an empty probe is still
    /// an empty probe.
    ///
    /// The host side is not a parameter, which is the real guard — as two
    /// arguments, swapping them silenced the emptiness check and passing an
    /// empty list dropped git from the report, and neither mistake could be
    /// reached by a test, because `doctor_cmd` needs a container. Now neither
    /// compiles. What is left to assert is the ordering and that the host's
    /// row is actually there.
    #[test]
    fn host_checks_never_stand_in_for_a_probe_that_did_not_run() {
        let sandbox = vec![doctor::Outcome {
            name: "rules".into(),
            ok: true,
            detail: "reads".into(),
        }];

        let err = cmd::harvest::every_check(Vec::new())
            .expect_err("a sandbox that ran nothing is not a pass");
        assert!(err.to_string().contains("did not run it"), "{err}");

        let both = cmd::harvest::every_check(sandbox).unwrap();
        assert_eq!(
            both.first().map(|o| o.name.as_str()),
            Some("rules"),
            "the sandbox's answers first: {both:?}"
        );
        assert!(
            both.iter().any(|o| o.name == "git on the host"),
            "and the host's are appended: {both:?}"
        );
    }

    /// What `--keep` and `--edit` mean, as a table.
    ///
    /// The headline of this change is that `--keep` alone no longer opens an
    /// editor, and nothing could prove it while `is_terminal` was consulted
    /// inline: no test process has a terminal, so every test took the same arm
    /// and a mutation that reopened the editor for every real user left the
    /// suite green. With the answer passed in, the decision is a table.
    #[test]
    fn what_keep_and_edit_mean_together() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        let keep = |selection: &str, edit: bool, terminal: bool| {
            cmd::harvest::what_to_keep(&shadow, &session, selection, edit, terminal, &|| Ok(true))
        };
        let _ = &paths;

        assert_eq!(
            keep("", false, true).unwrap(),
            shadow::Keep::All,
            "a bare --keep takes everything and opens nothing, terminal or not"
        );
        assert_eq!(keep("", false, false).unwrap(), shadow::Keep::All);
        assert_eq!(
            keep("", true, true).unwrap(),
            shadow::Keep::Edit,
            "--edit is what asks for the list"
        );
        assert!(
            keep("", true, false)
                .unwrap_err()
                .to_string()
                .contains("no terminal"),
            "and it needs somewhere to draw"
        );
        // Reachable at last: this refusal used to sit behind the terminal
        // check, so a script naming both was told about a terminal instead.
        assert!(
            keep("1", true, true)
                .unwrap_err()
                .to_string()
                .contains("twice"),
            "a selection and --edit name what to take twice"
        );
    }

    /// A merge cannot be picked one commit at a time, and is turned away
    /// before the fetch rather than by git afterwards.
    ///
    /// `log` renders a merge as `merge`, so `--keep 3` naming one is an
    /// ordinary thing to type. git's own answer is *"is a merge but no -m
    /// option was given"* — advice about a flag `--keep` forbids, and one that
    /// means something else entirely in omh.
    #[test]
    fn a_selection_naming_a_merge_is_refused_before_anything_moves() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let sandbox_git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        let seed = shadow.seed().unwrap();
        sandbox_git(&["checkout", "-q", "-b", "side", &seed]);
        std::fs::write(session.worktree.join("side.rs"), "fn side() {}\n").unwrap();
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "on the side"]);
        sandbox_git(&["checkout", "-q", "-"]);
        sandbox_git(&["merge", "-q", "--no-ff", "side", "-m", "Merge the side"]);

        let read = shadow.checkpoints(&session.worktree).unwrap();
        let merge = read
            .commits
            .iter()
            .find(|c| c.touched.is_none())
            .expect("the merge is a checkpoint");
        let err = cmd::harvest::what_to_keep(
            &shadow,
            &session,
            &merge.number.to_string(),
            false,
            false,
            &|| Ok(true),
        )
        .expect_err("a merge cannot be picked on its own");
        assert!(
            err.to_string().contains("merge") && !err.to_string().contains("-m option"),
            "refused in omh's words, not git's: {err}"
        );
        assert!(
            !paths.repo.join(".git/omh-harvest-s01-scratch").exists(),
            "and nothing was built to find that out"
        );
    }

    /// A number the session does not have is refused against the session's own
    /// list.
    ///
    /// The CLI test that claimed this could not reach it: `sb.session()`
    /// builds a worktree and no sandbox repository, so every spec — `9`, `0`,
    /// `two`, `4-2` — died identically inside `seed()`, about a record the
    /// user has never heard of. Replacing `chosen`'s body with a parser that
    /// ignored every rule left that test green.
    #[test]
    fn a_number_the_session_does_not_have_is_refused_with_the_range() {
        let (_paths, session, shadow) = a_session_with_two_checkpoints();
        for spec in ["9", "0", "two", "4-2", "1,1"] {
            let err =
                cmd::harvest::what_to_keep(&shadow, &session, spec, false, false, &|| Ok(true))
                    .unwrap_err()
                    .to_string();
            assert!(
                err.contains("1 to 2") || err.contains("twice") || err.contains("backwards"),
                "`{spec}` is refused against the session's own list: {err}"
            );
        }
    }

    /// A repository, a session, and a sandbox holding two checkpoints.
    fn a_session_with_two_checkpoints() -> (Paths, Session, shadow::Shadow) {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();
        for name in ["one", "two"] {
            std::fs::write(
                session.worktree.join(format!("{name}.rs")),
                format!("fn {name}() {{}}\n"),
            )
            .unwrap();
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", name],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
        }
        (paths, session, shadow)
    }

    /// Work the branch already has cannot be handed over twice.
    ///
    /// `omh sNN log` numbers every checkpoint, including the ones below the
    /// divider, so `--keep 1` is a reasonable thing to type about work that
    /// has already landed — and replaying it applies the same patch a second
    /// time. Refused by name rather than skipped, because silently dropping a
    /// number the user typed lands a different set than the one they asked
    /// for.
    #[test]
    fn a_selection_naming_work_the_branch_already_has_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();

        let mut ids = Vec::new();
        for name in ["one", "two"] {
            std::fs::write(
                session.worktree.join(format!("{name}.rs")),
                format!("fn {name}() {{}}\n"),
            )
            .unwrap();
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", name],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
            let head = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            ids.push(String::from_utf8_lossy(&head.stdout).trim().to_string());
        }
        // The first was handed over; the second was not.
        std::fs::write(&shadow.landed_record, format!("{}\n", ids[0])).unwrap();

        let err = cmd::harvest::what_to_keep(&shadow, &session, "1", false, false, &|| Ok(true))
            .expect_err("checkpoint 1 is already on the branch");
        assert!(
            err.to_string().contains('1') && err.to_string().contains("already"),
            "the refusal names the number and says why: {err}"
        );
        assert!(
            cmd::harvest::what_to_keep(&shadow, &session, "2", false, false, &|| Ok(true)).is_ok(),
            "and the one that has not landed is still keepable"
        );
    }

    /// A session whose sandbox never ran is *nothing yet*; one whose
    /// repository is there without a record of where it started is not.
    ///
    /// `reap` leaves the second behind when `remove_dir_all` fails on a live
    /// mount and the seed file goes anyway — a repository holding every
    /// checkpoint the agent made, and no way to say where they began. Reported
    /// as an empty session, the user's next move is `rm`.
    #[test]
    fn a_sandbox_repository_with_no_record_of_its_start_is_not_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();

        // Never launched: no repository, no record.
        let log = cmd::harvest::log_report(&paths, &session, false, &out::Ctx::plain()).unwrap();
        assert!(
            log.read.commits.is_empty(),
            "asking before the agent has run is an ordinary thing to do"
        );

        // Launched, then half-reaped.
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        let err = cmd::harvest::log_report(&paths, &session, false, &out::Ctx::plain())
            .expect_err("a repository omh cannot place is not an empty session");
        assert!(
            err.to_string().contains("rm"),
            "and the refusal warns about the move that would destroy it: {err}"
        );
    }

    /// `resolved` is the wiring between the manifest and every launch, and
    /// nothing reached it: replacing its body with a pair of defaults — omh
    /// contributing no hooks, no rules sections, nothing — left the whole suite
    /// green.
    ///
    /// That is the failure `tests/cli.rs` says in its own module doc it exists
    /// to notice: a guard correct while the wiring that reaches it is missing.
    /// `container` and `doctor` each build the pair in their fixtures, so they
    /// prove a plan handles one and say nothing about whether one arrives.
    #[test]
    fn resolved_reads_the_manifest_and_this_repos_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let write = |p: std::path::PathBuf, body: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        cmd::init::install_bundled(&paths.base(), bundled::Shipped::Base, &out::Ctx::plain())
            .unwrap();
        // Which servers the catalogue declares decides whether a feature was
        // *removed* rather than merely switched off, so the fixture has to
        // declare them — in the catalogue, which is the only place a server
        // lives. Seeded into `.omh/profile/mcp.json` this asserted nothing:
        // `installed` came back empty, every feature was already `gone` by the
        // removed-server path, and the `[omh]` half below was asserting an
        // absence that was there before the setting was written.
        write(
            paths.root.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"},"memory":{"command":"omh"}}}"#,
        );

        let (own, _) = cmd::session::resolved(&paths).unwrap();
        assert!(
            !own.hooks.is_empty() && !own.sections.is_empty(),
            "a launch must be given what the manifest ships"
        );
        assert!(
            own.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "with every feature on, the graph hooks are what `[omh]` below removes: {:?}",
            own.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );

        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n\n[mcp.memory.env]\nOMH_TEST = \"seen\"\n",
        );
        let (off, policy) = cmd::session::resolved(&paths).unwrap();
        assert!(
            !off.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "and `[omh]` in this repo has to reach it: {:?}",
            off.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );
        assert!(
            off.sections.iter().any(|s| s.name == "git-rules"),
            "without taking a different feature with it"
        );
        // The other half of the same wire. `settings::resolve` produces this
        // and `render::document` applies it, both tested — and the assignment
        // between them was asserted nowhere, so deleting it left the suite
        // green and a token reached no server.
        assert_eq!(
            policy.mcp_env["memory"]["OMH_TEST"], "seen",
            "a per-repo MCP environment has to reach the plan too"
        );
        // And the half that used to hang off `Own`: switching a feature off is
        // what drops its server from the document, and it is a fact about this
        // repo rather than about what omh generates.
        assert!(
            policy.disabled_servers.contains("codegraph"),
            "the feature's server travels with the feature: {:?}",
            policy.disabled_servers
        );
    }

    // ── the provisioning resolution ─────────────────────────────────────────

    fn outcome(name: &str, ok: bool, detail: &str) -> doctor::Outcome {
        doctor::Outcome {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }

    /// A probe that reported nothing means the container never ran it, and
    /// recording that as "nothing applies" is **destructive**: `reconcile` drops
    /// every `true` it is not told about, so an empty answer would erase the
    /// resolution and, on the next launch, the repo would provision nothing.
    ///
    /// Silence is cannot-tell, and cannot-tell writes nothing at all — the same
    /// asymmetry `detect::program` and `facts::Facts` are built on, at the one
    /// point where acting on it would delete somebody's file contents.
    #[test]
    fn a_resolution_nobody_measured_is_never_recorded() {
        assert_eq!(
            cmd::init::fired_from(3, &[]),
            None,
            "three provides asked, none answered — the container never ran"
        );
    }

    /// A partial report is not an answer either, and here that distinction
    /// deletes from a committed file.
    ///
    /// The protocol prints one line per provide, so fewer lines than provides
    /// means the container died part-way — OOM, a torn pipe, a runtime that
    /// truncates. Accepting the prefix as the whole answer makes
    /// `stack::reconcile` drop every `true` it was not told about, and
    /// `config::write_provision` then rewrites `.omh/settings.toml` without
    /// them. A rust repo loses `rust/linker = true`, the next layer is built
    /// with no `gcc`, and `cargo test` fails at link — the exact failure this
    /// design opens by describing.
    ///
    /// The now-deleted `[toolchain]` question had to be fixed for this same
    /// defect, where it only cost a spurious question. Here it edits a file
    /// under version control, which is why the guard outlived the question.
    #[test]
    fn a_partial_report_is_not_a_resolution() {
        let truncated = [outcome("rust/toolchain", true, "applies")];
        assert_eq!(
            cmd::init::fired_from(2, &truncated),
            None,
            "two provides asked, one answered — the container died mid-script"
        );
    }

    /// Nothing to ask is an **answer**, not silence, and the difference decides
    /// whether a stale resolution is ever cleared.
    ///
    /// A repo that stops being a stack — `Cargo.toml` deleted, the crate moved
    /// into a subdirectory — has no candidates, so no container runs. Treating
    /// that like an unanswered probe leaves `[provision]` asserting
    /// `rust/toolchain = true` for ever: the stack layer keeps installing a
    /// toolchain nothing uses, and the committed file describes a repo that no
    /// longer exists.
    #[test]
    fn nothing_to_ask_is_an_answer_and_clears_a_stale_resolution() {
        assert_eq!(
            cmd::init::fired_from(0, &[]),
            Some(std::collections::BTreeSet::new()),
            "no candidates is a measured 'nothing applies', not a failure to measure"
        );
    }

    /// And a probe that *did* answer is taken at its word — only the provides
    /// that applied. A provide that could not answer is simply absent, which is
    /// the safe direction: it does not get installed, its `needs` then fails to
    /// resolve, and that is reported. Installing on a coin-flip would be silent.
    #[test]
    fn only_the_provides_that_applied_are_recorded() {
        let answered = [
            outcome("rust/toolchain", true, "applies"),
            outcome("node/pnpm", false, "1 does not apply"),
            outcome("node/bun", false, "2 could not answer"),
        ];
        assert_eq!(
            cmd::init::fired_from(3, &answered),
            Some(std::collections::BTreeSet::from([
                "rust/toolchain".to_string()
            ]))
        );
    }

    /// Installs run in file order, and only for provides the resolution
    /// recorded. A provide that asserts something the base image already
    /// ships — node's `runtime` — contributes no `RUN` and must not move the
    /// tag.
    #[test]
    fn installs_are_the_recorded_recipes_in_file_order() {
        let defs = stack::load_dir(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/stacks"
        )))
        .unwrap();
        let node = defs.iter().find(|d| d.name == "node").expect("node ships");

        // Everything applies, so only `install`-carrying provides may appear,
        // in the order the file gives them.
        let all: BTreeMap<String, bool> = node
            .provides
            .iter()
            .map(|p| (stack::key(&node.name, &p.name), true))
            .collect();
        let got = cmd::init::installs_for(&[node], &all);

        let expected: Vec<&str> = node
            .provides
            .iter()
            .filter_map(|p| p.install.as_deref())
            .collect();
        assert_eq!(got, expected, "order or filtering changed");
        assert!(
            !got.is_empty() && got.len() < node.provides.len(),
            "node must have both kinds of provide for this to prove anything: {got:?}"
        );
    }

    /// Only the recipes that fired, and the **file** decides their order.
    ///
    /// The test above records every provide, so it exercises the filter only in
    /// the case where the filter does nothing, and it draws its fixture from
    /// `stacks/node.toml`, whose recipes happen to already be in alphabetical
    /// order. Two wrong implementations pass it: one that ignores the
    /// resolution entirely, and one that sorts.
    ///
    /// Both are real failures. Ignoring the resolution installs *every* package
    /// manager into a node repo — the outcome `stacks/node.toml` opens by
    /// forbidding, since a repo with a `pnpm-lock.yaml` must not also get yarn
    /// and bun. Sorting puts `corepack enable pnpm` ahead of the node provide
    /// it needs, and the image build fails on a stack file that is correct.
    ///
    /// So the fixture is hostile on both axes at once: what was recorded is a
    /// strict subset, and file order is not sorted order.
    #[test]
    fn only_the_recorded_recipes_run_and_the_file_decides_their_order() {
        fn provide(name: &str, install: Option<&str>) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![name.into()],
                when: None,
                install: install.map(str::to_string),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "fixture".into(),
            marker: "fixture.toml".into(),
            provides: vec![
                provide("zulu", Some("install zulu")),
                provide("alpha", Some("install alpha")),
                provide("asserted", None),
                provide("mike", Some("install mike")),
            ],
        };
        // `mike` was never recorded; `asserted` applies and has no recipe.
        let resolved: BTreeMap<String, bool> =
            ["fixture/zulu", "fixture/alpha", "fixture/asserted"]
                .iter()
                .map(|k| ((*k).to_string(), true))
                .collect();

        assert_eq!(
            cmd::init::installs_for(&[&def], &resolved),
            vec!["install zulu", "install alpha"],
            "a provide the resolution does not name must contribute no recipe, \
             and sorted order is not file order"
        );
    }

    /// An opt-out keeps the recipe out of the image, which is the only thing an
    /// opt-out could mean.
    ///
    /// `[provision] "rust/linker" = false` is how somebody says *do not install
    /// this* — because it costs 124 MB they do not want, or because their base
    /// image already has it. `reconcile` preserves that `false` faithfully and
    /// `settings::resolve` reads it back, and there was a version where both
    /// were ceremony: the install set was built from what *fired*, so the
    /// recipe ran anyway. The file said one thing, the image was another, and
    /// `omh why` would cite the file.
    ///
    /// Kept as its own case now that `installs_for` reads only `true`, because
    /// what it guards is not the filter's spelling but the outcome: a `false`
    /// and a key nobody recorded must reach the image identically, and a
    /// future `unwrap_or(true)` would break exactly this and nothing else.
    #[test]
    fn a_provide_somebody_opted_out_of_is_not_installed() {
        fn provide(name: &str, install: &str) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![name.into()],
                when: None,
                install: Some(install.into()),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: vec![
                provide("toolchain", "install rustup"),
                provide("linker", "apt-get install -y gcc"),
            ],
        };
        let resolved = BTreeMap::from([
            ("rust/toolchain".to_string(), true),
            ("rust/linker".to_string(), false),
        ]);

        assert_eq!(
            cmd::init::installs_for(&[&def], &resolved),
            vec!["install rustup"],
            "the predicate said the linker applies; a person said not here, and \
             a person outranks a predicate"
        );
    }
    /// **A repo that provisions runs a different image from one that does
    /// not**, and two repos provisioning different things do not share one
    /// either.
    ///
    /// This is what the whole design comes down to, and it is the check the
    /// plan reserved for a machine with docker: same stack, same marker,
    /// different lockfiles, *different images*. The half that needs no
    /// container is the arithmetic — which tag a repo resolves to — and that is
    /// what is asserted here. Whether the image then contains a working pnpm
    /// is `omh doctor`'s question and no green test can answer it.
    ///
    /// Read through `sandbox`, the same function every launch and `init` call,
    /// because the bug this replaces was not a wrong tag: it was two places
    /// computing one.
    #[test]
    fn a_repo_that_provisions_runs_a_different_image() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.repo.join("package.json"), "{}").unwrap();
        std::fs::copy(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/stacks/node.toml")),
            paths.stacks().join("node.toml"),
        )
        .unwrap();
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();

        let with = |keys: &[&str]| {
            let mut repo = settings::RepoPolicy::default();
            for k in keys {
                repo.provision.insert((*k).to_string(), true);
            }
            cmd::init::sandbox(&paths, &adapter, &repo, image::ca_for(&paths).unwrap())
                .unwrap()
                .tag
        };

        let nothing = with(&[]);
        let pnpm = with(&["node/pnpm"]);
        let yarn = with(&["node/yarn"]);

        assert_eq!(
            nothing,
            image::tag_for(&adapter, None),
            "a repo that provisions nothing runs the harness image, not an \
             empty layer on top of it"
        );
        assert_ne!(pnpm, nothing, "provisioning changes the image");
        assert_ne!(
            pnpm, yarn,
            "same stack, same marker, different lockfile — and a shared image \
             would hand the yarn repo pnpm and nothing else"
        );
        assert_eq!(
            pnpm,
            with(&["node/pnpm"]),
            "and it is stable, or every launch rebuilds"
        );
    }

    /// A probe that **ran and failed** is not a measurement of nothing, and the
    /// difference is a sentence on the user's terminal.
    ///
    /// `docker run` against a missing image, a refusing daemon or a full disk
    /// exits non-zero with empty stdout. Parsed anyway that is an empty outcome
    /// list — indistinguishable from a sandbox that answered and had nothing,
    /// except that one of the two is a broken machine. Both leave every hook
    /// unsuppressed, which is the safe direction; only one of them deserves
    /// silence, and it is not this one.
    ///
    /// This is the same defect `init`'s predicate call was fixed for, in a
    /// function written afterwards — which is why the guard is a value here
    /// rather than a `println!` no test can watch vanish.
    #[test]
    fn a_probe_that_ran_and_failed_is_a_reason_not_a_measurement() {
        let failed = cmd::init::measured_or_reason(false, "", "Error: No such image: omh/x:abc\n");
        let Err(reason) = failed else {
            panic!("a failed container was read as a sandbox with nothing in it");
        };
        assert!(
            reason.contains("could not ask the sandbox"),
            "the reason has to say nobody was asked: {reason}"
        );
        assert!(
            reason.contains("No such image"),
            "and carry what the runtime said, or it names no cause: {reason}"
        );

        // A probe that succeeded is taken at its word, protocol and all.
        assert_eq!(
            cmd::init::measured_or_reason(true, "ok\tcargo\tresolves\n", "")
                .expect("a successful probe is an answer")
                .len(),
            1
        );
        // Including when it honestly measured nothing — an empty *successful*
        // report is a report, and must not be dressed as a failure.
        assert_eq!(cmd::init::measured_or_reason(true, "", ""), Ok(Vec::new()));
    }

    /// Stderr is carried, but not all of it.
    ///
    /// A runtime failing to pull or mount can produce a page of output, and a
    /// diagnostic that buries the line above it under its own noise is one
    /// people learn to scroll past — the same reason `init`'s predicate report
    /// takes three lines.
    #[test]
    fn the_reason_carries_a_few_lines_of_evidence_not_a_page() {
        let noisy: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let Err(reason) = cmd::init::measured_or_reason(false, "", &noisy) else {
            panic!("must be a reason");
        };
        assert_eq!(
            reason.lines().count(),
            4,
            "one reason and three lines of evidence: {reason}"
        );
    }

    // ── the questions of last resort ────────────────────────────────────────

    fn unclaimed(stacks: &[&str]) -> Vec<stack::Marker> {
        stacks
            .iter()
            .map(|s| stack::Marker {
                file: format!("{s}.manifest"),
                stack: (*s).to_string(),
            })
            .collect()
    }

    fn exchange(
        markers: &[stack::Marker],
        has_test: bool,
        typed: &str,
    ) -> (usize, Vec<ask::Answer>) {
        let refs: Vec<&stack::Marker> = markers.iter().collect();
        let mut out = Vec::new();
        cmd::init::ask_all(
            &refs,
            has_test,
            &mut std::io::BufReader::new(typed.as_bytes()),
            &mut out,
        )
        .unwrap()
    }

    /// **A decline stops the remaining marker questions**, rather than putting
    /// every one of them into the same void.
    ///
    /// A decline and a closed pipe are indistinguishable at this level, and the
    /// one that matters is the pipe: a polyglot repo with three unclaimed
    /// markers would otherwise print three questions nobody can see. One "no"
    /// is answer enough to stop asking — the scar the deleted `[toolchain]`
    /// question earned, carried over rather than re-learned.
    #[test]
    fn declining_one_question_stops_the_rest() {
        let three = unclaimed(&["elixir", "ruby", "php"]);
        let (asked, answers) = exchange(&three, true, "\n");
        assert_eq!(asked, 1, "one question put, and no more after the decline");
        assert!(answers.is_empty());

        // A closed pipe reaches the same place without a prompt being answered
        // at all.
        assert_eq!(exchange(&three, true, ""), (1, Vec::new()));
    }

    /// **A question declined is a question asked.** The headline counts what
    /// was put on screen, not what came back — claiming "asked nothing" after
    /// interrogating somebody is the promise the tagline sells, broken while
    /// they watch.
    #[test]
    fn the_count_is_what_was_put_not_what_was_answered() {
        let one = unclaimed(&["elixir"]);
        let (asked, answers) = exchange(&one, true, "\n");
        assert_eq!((asked, answers.len()), (1, 0));

        let (asked, answers) = exchange(&one, true, "apt-get install -y elixir\nmix\n");
        assert_eq!((asked, answers.len()), (1, 1));
        assert_eq!(answers[0].path, std::path::Path::new("stacks/elixir.toml"));
    }

    /// Neither question is put where nothing is unknown — which is most repos,
    /// most of the time, and is what keeps this from being a wizard.
    #[test]
    fn a_repo_with_nothing_unknown_is_asked_nothing() {
        assert_eq!(exchange(&[], true, "mix test\n"), (0, Vec::new()));
    }

    /// The test question stands alone: a repo omh understands entirely, that
    /// still has no way to check its own work.
    #[test]
    fn a_project_with_no_way_to_test_itself_is_asked_about_that_alone() {
        let (asked, answers) = exchange(&[], false, "mix test\n");
        assert_eq!(asked, 1);
        assert_eq!(answers[0].path, std::path::Path::new("hooks/test.json"));
    }

    /// **Covered means covered *here*.** A catalogue hook for an ecosystem this
    /// repo is not speaks for nothing in it.
    ///
    /// Without the intersection this answers `{rust, go, python}` in every repo
    /// — that is simply what omh ships — and `derive::hooks` reads a non-empty
    /// `covered` as *some ecosystem hook already runs this project's tests*. So
    /// runner derivation, the whole hand-rolled `Makefile`/`justfile`/`Taskfile`
    /// scanner, could not fire for anybody, and a C project with a working
    /// `make test` was then told by `omh init` that omh had found **no runner**.
    ///
    /// Every unit test of `derive::hooks` passes a hand-built `covered`, so none
    /// of them could see this: the defect was in what the caller computed, and
    /// nothing tested the caller. That is why this is a function.
    #[test]
    fn what_the_catalogue_covers_elsewhere_covers_nothing_here() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("rust-test.json"),
            r#"{"on":"turn-end","stack":"rust","run":"cargo test"}"#,
        )
        .unwrap();
        std::fs::write(
            hooks.join("shellcheck.json"),
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        )
        .unwrap();
        let dirs = [hooks];

        let rust = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: Vec::new(),
        };

        assert_eq!(
            cmd::catalogue::covered_here(&dirs, &[]).unwrap(),
            BTreeSet::new(),
            "a repo that is no ecosystem omh ships a hook for is covered by \
             none of them — this is the C project with a Makefile, and the \
             whole runner path depends on it"
        );
        assert_eq!(
            cmd::catalogue::covered_here(&dirs, &[&rust]).unwrap(),
            ["rust".to_string()].into_iter().collect(),
            "and a rust repo is covered, so its Makefile earns no second hook"
        );
    }

    /// A hook belonging to an ecosystem this repo is not could never have been
    /// taken here, so it is not offered and not reported as unselected.
    ///
    /// This is **applicability, not selection**, and the distinction is the
    /// whole of it. `[use]` is what you chose from what you could have chosen;
    /// once omh ships a hook per ecosystem, a rust repo's catalogue holds
    /// `go-test` and `python-format` too, and listing them as "available but
    /// not selected" would turn a real report — *here is what you are not
    /// using* — into a page of things nobody could ever use. The launcher's
    /// unselected line exists to be read, and a report nobody reads is one that
    /// stops catching the entry you did mean to take.
    ///
    /// A hook naming **no** stack belongs everywhere: `graph-refresh`, or
    /// somebody's `shellcheck`. Those are never filtered, and the asymmetry
    /// matters — filtering by "names a detected stack" rather than "does not
    /// name an undetected one" would hide every general hook in the catalogue.
    #[test]
    fn a_hook_for_an_ecosystem_this_repo_is_not_is_not_offered() {
        let declared = BTreeMap::from([
            ("rust-test".to_string(), Some("rust".to_string())),
            ("go-test".to_string(), Some("go".to_string())),
            ("shellcheck".to_string(), None),
        ]);
        let names = vec![
            "rust-test".to_string(),
            "go-test".to_string(),
            "shellcheck".to_string(),
            // A name in the list that no file declares — the repo's own hook
            // directory is read separately, and a name omh knows nothing about
            // is not a name omh may drop.
            "mine".to_string(),
        ];
        let detected: BTreeSet<String> = ["rust".to_string()].into_iter().collect();

        assert_eq!(
            cmd::catalogue::applicable_hooks(names.clone(), &declared, &detected),
            vec![
                "rust-test".to_string(),
                "shellcheck".to_string(),
                "mine".to_string()
            ],
            "only the hook naming an ecosystem this repo is not comes out"
        );

        // A repo omh detects nothing for keeps everything that claims nothing.
        assert_eq!(
            cmd::catalogue::applicable_hooks(names, &declared, &BTreeSet::new()),
            vec!["shellcheck".to_string(), "mine".to_string()]
        );
    }

    /// What the sandbox is asked about is the **union**, and neither half
    /// contains the other.
    ///
    /// `needs` is what a *stack* promised: it catches rustup installing a
    /// `cargo` that then cannot link, which is a provisioning failure and
    /// belongs in `init`'s report. A hook's program is what will be handed to a
    /// shell: a hand-written `shellcheck` hook is in no `needs` list anywhere,
    /// and asking only about `needs` ships it into a sandbox that cannot run
    /// it — `cargo: not found` with a different program in it.
    ///
    /// Both mutations are one line and neither is implausible, which is why
    /// this asserts the whole set rather than two `contains`.
    #[test]
    fn the_sandbox_is_asked_about_both_what_stacks_promised_and_what_hooks_run() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("lint.json"),
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        )
        .unwrap();

        let def = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: vec![stack::Provide {
                name: "toolchain".into(),
                needs: vec!["cargo".into(), "rustc".into()],
                when: None,
                install: Some("install rustup".into()),
                because: "a fixture".into(),
                measured: Vec::new(),
            }],
        };
        let mut repo = settings::RepoPolicy::default();
        repo.provision.insert("rust/toolchain".to_string(), true);

        // Through `needs_of`, because that is what `sandbox` puts in `owed` —
        // asserting the union over a hand-written set would let the two halves
        // agree here and disagree in the one place it matters.
        let owed = cmd::init::needs_of(&[&def], &repo.provision);
        let asked = cmd::init::probe_targets(&[hooks], &Default::default(), &repo, &owed).unwrap();

        assert_eq!(
            asked,
            BTreeSet::from([
                "cargo".to_string(),
                "rustc".to_string(),
                "shellcheck".to_string()
            ]),
            "asking about only one of the two lists leaves the other unmeasured"
        );
    }

    /// A provide nobody recorded, and a provide somebody opted out of, owe
    /// nothing — so neither can be reported as a provisioning failure.
    ///
    /// `init` prints "did not resolve after installing" for these, and that
    /// sentence has to be true. A provide that was never installed did not
    /// fail to install; saying so is a gap omh invented, and it would print on
    /// every `init` for anybody who opted out of the 124 MB linker on purpose.
    ///
    /// The consequence of the opt-out is not silenced by this. If a hook names
    /// the program, `probe_targets` asks about it through the hooks half, and a
    /// hook that cannot run is dropped by name.
    #[test]
    fn only_what_was_provisioned_owes_a_program() {
        fn provide(name: &str, need: &str, install: Option<&str>) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![need.into()],
                when: None,
                install: install.map(str::to_string),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "node".into(),
            marker: "package.json".into(),
            provides: vec![
                // No `install`: an assertion that the base image already ships
                // this, which is worth writing down only if something checks it.
                provide("runtime", "node", None),
                provide("pnpm", "pnpm", Some("corepack enable pnpm")),
                provide("yarn", "yarn", Some("corepack enable yarn")),
                provide("bun", "bun", Some("npm install -g bun")),
            ],
        };
        let resolved = BTreeMap::from([
            ("node/runtime".to_string(), true),
            ("node/pnpm".to_string(), true),
            ("node/yarn".to_string(), false),
            // `node/bun` was never recorded at all.
        ]);

        assert_eq!(
            cmd::init::needs_of(&[&def], &resolved),
            BTreeSet::from(["node".to_string(), "pnpm".to_string()]),
            "an assertion with no recipe is still owed; an opt-out and an \
             absence are not"
        );
    }

    /// A stacks directory omh cannot read is **said**, and it withdraws the
    /// drift report rather than filing a wrong one.
    ///
    /// `notice::hooks` decides which hooks name a stack this repo is not, so
    /// with no definitions it concludes that every stack-named hook belongs to
    /// nothing. Swallowing the error into an empty list therefore does not
    /// degrade the report — it inverts it, and prints the inversion in omh's
    /// own voice. The neighbouring branch already reports its error and returns
    /// `None`; this one used `unwrap_or_default` and said nothing at all.
    #[test]
    fn a_stacks_directory_omh_cannot_read_is_reported_rather_than_read_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.stacks().join("rust.toml"), "this is not toml {{{").unwrap();

        assert!(
            cmd::session::say_hooks(&paths, &out::Ctx::plain()).is_none(),
            "a report built on stacks that would not load is a wrong report"
        );
    }

    /// The resolution is read from and written to the **committed** layer, and
    /// nothing else guarded which layer that is.
    ///
    /// `config`'s own tests prove the reader answers for one layer, and prove
    /// it for a good reason — but a correct reader called with the wrong
    /// argument is the same bug with a passing guard in front of it. Both
    /// mutations survived the suite: reading `Local` exports one laptop's
    /// `false` into a file everybody clones, and writing `Local` means the
    /// resolution never reaches a teammate, which is the entire case for the
    /// table living in committed settings.
    #[test]
    fn the_resolution_is_read_and_written_in_the_committed_layer() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let write = |layer: config::Layer, body: &str| {
            let f = layer.file(&paths);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        write(
            config::Layer::Shared,
            "[provision]\n\"rust/toolchain\" = true\n",
        );
        let local_before = "[provision]\n\"node/pnpm\" = false\n";
        write(config::Layer::Local, local_before);

        let fired: BTreeSet<String> = ["rust/toolchain", "node/pnpm"]
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        cmd::init::record_resolution(&paths, &fired).unwrap();

        let shared = std::fs::read_to_string(config::Layer::Shared.file(&paths)).unwrap();
        let parsed: toml::Table = toml::from_str(&shared).expect("still TOML");
        assert_eq!(
            parsed["provision"]["rust/toolchain"].as_bool(),
            Some(true),
            "the resolution must land in the committed file: {shared}"
        );
        assert_eq!(
            parsed["provision"]["node/pnpm"].as_bool(),
            Some(true),
            "a laptop's opt-out must not be read back as the team's: {shared}"
        );
        assert_eq!(
            std::fs::read_to_string(config::Layer::Local.file(&paths)).unwrap(),
            local_before,
            "the local layer is somebody's own file and init does not edit it"
        );
    }

    // ── proposed guards ─────────────────────────────────────────────────────

    /// A stand-in for the container runtime: a script that records every call
    /// and answers the probe protocol.
    ///
    /// `measure` and `top_up` take the runtime's program name as an argument,
    /// so the whole measurement path is reachable without a container. What
    /// this cannot prove is what a *real* image contains — that is `omh
    /// doctor`'s, and no green test crosses that line. What it does prove is
    /// omh's own arithmetic: which questions get asked, of which image, and
    /// what is done with the answers.
    fn fake_runtime(dir: &std::path::Path, present: &[&str], absent: &[&str]) -> String {
        let log = dir.join("calls.log");
        let mut body = String::from("#!/bin/sh\n");
        body.push_str(&format!(
            "printf 'CALL %s\\n' \"$*\" | tr -d '\\n' >> {}; printf '\\n' >> {}\n",
            log.display(),
            log.display()
        ));
        for p in present {
            body.push_str(&format!("printf 'ok\\t{p}\\tresolves\\n'\n"));
        }
        for p in absent {
            body.push_str(&format!("printf 'fail\\t{p}\\tnot installed\\n'\n"));
        }
        let bin = dir.join("fake-runtime");
        std::fs::write(&bin, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin.to_string_lossy().to_string()
    }

    /// Every call the fake was given, and the probes among them — a probe is
    /// the one that runs the image rather than inspecting or building it.
    fn calls(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn probes(dir: &std::path::Path) -> Vec<String> {
        calls(dir)
            .into_iter()
            .filter(|c| c.contains("--pull=never"))
            .collect()
    }

    fn a_sandbox(tag: &str, owed: &[&str]) -> cmd::init::Sandbox {
        cmd::init::Sandbox {
            installs: Vec::new(),
            tag: tag.to_string(),
            resolves: BTreeMap::new(),
            owed: owed.iter().map(|s| (*s).to_string()).collect(),
            unmeasured: Some("not asked yet".into()),
            ca: None,
        }
    }

    fn measurement_fixture(dir: &std::path::Path) -> (Paths, Adapter) {
        let paths = Paths {
            root: dir.join("home"),
            repo: dir.join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        (paths, adapter)
    }

    /// **A second launch asks the image nothing**, and that is the only reason
    /// a launch is not a container run.
    ///
    /// Everything the cache is for lives in one function nothing reached:
    /// skipping `unseen`, dropping the `save`, or throwing the answer away
    /// instead of storing it all left the suite green. The first two spend a
    /// container run on every launch of every repo forever; the third ships
    /// every hook into a sandbox that may not have its program, which is the
    /// `cargo: not found` failure this milestone exists to remove.
    #[cfg(unix)]
    #[test]
    fn a_second_launch_asks_the_image_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &["cc"]);
        let own = base::Own::default();
        let repo = settings::RepoPolicy::default();

        let mut first = a_sandbox("omh/claude:abc123", &["cargo", "cc"]);
        first
            .top_up(
                &paths,
                &runtime,
                &adapter,
                &[],
                &own,
                &repo,
                &out::Ctx::plain(),
            )
            .unwrap();

        assert_eq!(probes(dir.path()).len(), 1, "the first launch must ask");
        assert_eq!(
            first.resolves.get("cargo"),
            Some(&true),
            "and keep what it was told: {:?}",
            first.resolves
        );
        assert_eq!(first.resolves.get("cc"), Some(&false));
        assert!(
            paths.facts().exists(),
            "and write it down, or the next launch asks again"
        );

        let mut second = a_sandbox("omh/claude:abc123", &["cargo", "cc"]);
        second
            .top_up(
                &paths,
                &runtime,
                &adapter,
                &[],
                &own,
                &repo,
                &out::Ctx::plain(),
            )
            .unwrap();
        assert_eq!(
            probes(dir.path()).len(),
            1,
            "a repo whose hooks and stacks have not changed must start no container"
        );
        assert_eq!(
            second.resolves.get("cc"),
            Some(&false),
            "and still know what was measured before: {:?}",
            second.resolves
        );
    }

    /// The probe runs in **this** image and asks about what this repo owes —
    /// and the answers are filed under the same tag.
    ///
    /// Probing or filing under any other tag caches an answer about image A
    /// against image B. Both directions are silent: a hook suppressed in a
    /// sandbox that has its program, or shipped into one that does not.
    #[cfg(unix)]
    #[test]
    fn the_probe_asks_this_image_about_what_it_owes() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &[]);

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        sb.top_up(
            &paths,
            &runtime,
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        )
        .unwrap();

        let probe = probes(dir.path()).join("\n");
        assert!(
            probe.contains("omh/claude:abc123"),
            "the probe must run in the image this session will run: {probe}"
        );
        assert!(
            probe.contains("cargo"),
            "and ask about what the stacks promised: {probe}"
        );

        let raw = std::fs::read_to_string(paths.facts()).unwrap();
        assert!(
            raw.contains("omh/claude:abc123"),
            "and file the answer under that image's tag: {raw}"
        );
    }

    /// **There is an image before there is a question about it.**
    ///
    /// All three launch paths measured first and built inside `session_up`, so
    /// the first launch after a recipe changed probed a tag with no image
    /// behind it, learned nothing, and shipped every hook unsuppressed. It
    /// healed on the second launch, which is exactly the broken first turn this
    /// design removes. `top_up` now builds first, and nothing else says so.
    #[cfg(unix)]
    #[test]
    fn an_image_is_made_sure_of_before_it_is_asked_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &[]);

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        sb.top_up(
            &paths,
            &runtime,
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        )
        .unwrap();

        let all = calls(dir.path());
        let built = all
            .iter()
            .position(|c| !c.contains("--pull=never"))
            .expect("the image has to be made sure of at all");
        let asked = all
            .iter()
            .position(|c| c.contains("--pull=never"))
            .expect("and then asked");
        assert!(
            built < asked,
            "a probe against an image nobody built learns nothing: {all:?}"
        );
    }

    /// A runtime that cannot answer is **cannot-tell**, never a sandbox with
    /// nothing in it.
    ///
    /// Suppression acts on a measured `false`. A probe that could not run must
    /// leave the facts as they were, or one broken docker turns every hook in
    /// every repo off in a session that otherwise looks normal.
    #[cfg(unix)]
    #[test]
    fn a_probe_that_cannot_run_suppresses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        // Exits non-zero for everything, so the image "exists" is false and the
        // probe fails — the shape of a daemon that is refusing.
        let bin = dir.path().join("failing-runtime");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        let _ = sb.top_up(
            &paths,
            &bin.to_string_lossy(),
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        );

        assert_eq!(
            sb.resolves.get("cargo"),
            None,
            "silence is cannot-tell, and cannot-tell is never a measured \
             absence: {:?}",
            sb.resolves
        );
    }

    /// A stack whose recipes are neither sorted nor all-applying — hostile on
    /// both axes, so a recipe that sorts and an `owed` built from the
    /// definitions rather than the resolution both come out wrong here.
    fn provisioned_fixture(paths: &Paths) {
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.repo.join("fixture.toml"), "").unwrap();
        std::fs::write(
            paths.stacks().join("fixture.toml"),
            r#"
name   = "fixture"
marker = "fixture.toml"

[[provide]]
name    = "zulu"
needs   = ["zulu"]
install = "install zulu"
because = "a fixture"

[[provide]]
name    = "alpha"
needs   = ["alpha"]
install = "install alpha"
because = "a fixture"

[[provide]]
name    = "declined"
needs   = ["declined"]
install = "install declined"
because = "a fixture"
"#,
        )
        .unwrap();
    }

    fn fixture_policy() -> settings::RepoPolicy {
        let mut repo = settings::RepoPolicy::default();
        repo.provision.insert("fixture/zulu".to_string(), true);
        repo.provision.insert("fixture/alpha".to_string(), true);
        repo.provision.insert("fixture/declined".to_string(), false);
        repo
    }

    /// **The recipe a launch builds must produce the tag that launch runs.**
    ///
    /// `session_up` takes the two as separate arguments — `opts.image` and
    /// `recipe` — so nothing stops them describing different images, which is
    /// the milestone-one bug in a new place. Asserted as agreement rather than
    /// against a literal, because the failure is divergence.
    #[test]
    fn the_layer_a_sandbox_names_is_the_layer_its_recipe_builds() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        provisioned_fixture(&paths);
        // **With a certificate set**, because that is the dimension this guard
        // was blind to. It hardcoded `None` on both sides, so reverting either
        // `sandbox()` or `session_up` to pass `None` left the whole suite
        // green — and what that ships is a stack layer built without the
        // corporate root while `plan.image` names the tag that has it.
        let pem = dir.path().join("corp.pem");
        std::fs::write(
            &pem,
            "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(
            paths.repo.join(".omh/settings.toml"),
            format!("ca_cert = \"{}\"\n", pem.display()),
        )
        .unwrap();
        let ca = image::ca_for(&paths).unwrap();
        assert!(ca.is_some(), "the fixture must actually set one");

        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        let sb = cmd::init::sandbox(&paths, &adapter, &fixture_policy(), ca.clone()).unwrap();

        assert_eq!(
            sb.recipe(),
            vec!["install zulu", "install alpha"],
            "file order is install order, and an opt-out contributes no recipe"
        );
        assert_ne!(
            sb.tag,
            image::tag_for(&adapter, ca.as_deref()),
            "this fixture must provision something or it proves nothing"
        );
        assert_eq!(
            image::stack_tag(&adapter, &sb.recipe(), ca.as_deref()),
            sb.tag,
            "the recipe handed to `ensure_stack` must build the tag `plan` runs, \
             or a session runs an image nothing built"
        );
        assert_ne!(
            sb.tag,
            image::stack_tag(&adapter, &sb.recipe(), None),
            "and the certificate must be part of what the tag names, or this \
             fixture is back to proving nothing about it"
        );
    }

    /// What `sandbox` hands on is the **resolution's** list and the **tag's**
    /// measurements, and neither is re-derived from anything else.
    #[test]
    fn a_sandbox_carries_what_it_owes_and_what_is_already_known() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        provisioned_fixture(&paths);
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        let repo = fixture_policy();

        let first =
            cmd::init::sandbox(&paths, &adapter, &repo, image::ca_for(&paths).unwrap()).unwrap();
        assert_eq!(
            first.owed,
            BTreeSet::from(["zulu".to_string(), "alpha".to_string()]),
            "a provide somebody opted out of was never installed and owes \
             nothing: {:?}",
            first.owed
        );
        assert!(
            first.resolves.is_empty(),
            "and nothing has been measured about this image yet"
        );

        let mut facts = facts::Facts::default();
        facts.learn(
            &first.tag,
            &[doctor::Outcome {
                name: "alpha".into(),
                ok: false,
                detail: "not installed in the sandbox".into(),
            }],
        );
        facts.save(&paths).unwrap();

        let second =
            cmd::init::sandbox(&paths, &adapter, &repo, image::ca_for(&paths).unwrap()).unwrap();
        assert_eq!(
            second.resolves.get("alpha"),
            Some(&false),
            "a sandbox must arrive knowing what was measured about its own \
             tag: {:?}",
            second.resolves
        );
    }

    // ── the shipped hooks ───────────────────────────────────────────────────

    /// `write_if_absent` says it wrote only when it wrote.
    ///
    /// `init` reports the settings seed off this answer, and the report is a
    /// claim about a **committed** file: saying a repo was seeded from your
    /// template when its `settings.toml` was already there describes an effect
    /// the template did not have.
    #[test]
    fn write_if_absent_reports_only_a_write_it_made() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("settings.toml");

        assert!(
            cmd::init::write_if_absent(&f, "first").unwrap(),
            "an absent file is written, and saying so is the whole return value"
        );
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "first");

        assert!(
            !cmd::init::write_if_absent(&f, "second").unwrap(),
            "a file that was already there was not written by this call"
        );
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "first",
            "and it is left alone — the name says absent, not overwrite"
        );
    }

    /// The template hands on what omh reads, and refuses what it must not.
    ///
    /// `init` builds an image, so the end-to-end half of this runs only where a
    /// container runtime does. The rule itself does not need one — and the rule
    /// is the part with a security claim in it, so it is tested where every run
    /// sees it.
    #[test]
    fn seed_settings_takes_what_omh_reads_and_refuses_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home").join(".omh"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();
        let template = config::Layer::Personal.file(&paths);

        std::fs::write(
            &template,
            "idle_timeout = \"45m\"\nrubbish = \"ignored\"\n\n\
             [use]\nskills = [\"tdd\"]\n\n[omh]\ncodegraph = false\n",
        )
        .unwrap();
        let (body, took) = cmd::init::seed_settings(&paths).unwrap();

        assert!(body.contains("45m"), "a key omh reads travels: {body}");
        assert!(
            !body.contains("rubbish"),
            "and one it does not is left behind rather than propagated into \
             every repo you ever start: {body}"
        );
        assert!(
            body.contains("[use]") && body.contains("tdd"),
            "the selection travels: {body}"
        );
        // Parsed, not grepped. The old assertion was satisfied by the
        // commented-out `# [omh]` placeholder the header appends when no
        // switches travel, so dropping `[omh]` from the seed passed.
        let parsed: toml::Table =
            toml::from_str(&body).expect("what init writes has to be a settings file");
        assert_eq!(
            parsed["omh"]["codegraph"].as_bool(),
            Some(false),
            "the feature switches travel, as a switch: {body}"
        );
        assert_eq!(
            parsed["idle_timeout"].as_str(),
            Some("45m"),
            "and the key is a value, not a line that happens to contain it"
        );
        assert!(
            took.contains(&"[omh]".to_string()),
            "and what travelled is reported: {took:?}"
        );
        assert!(
            took.contains(&"idle_timeout".to_string()) && took.contains(&"[use]".to_string()),
            "what travelled is reported, or the seed is indistinguishable from \
             a default: {took:?}"
        );

        // A server's environment can be a token and the seeded file is
        // committed. Refused, not skipped: dropping it silently would leave
        // somebody believing a token is in force.
        std::fs::write(&template, "[mcp.linear.env]\nTOKEN = \"secret\"\n").unwrap();
        let refused = cmd::init::seed_settings(&paths).unwrap_err().to_string();
        assert!(
            refused.contains("omh settings mcp add"),
            "the refusal names where a server's environment belongs: {refused}"
        );
    }

    /// A feature named after its own entry resolves as the feature.
    ///
    /// The shipped manifest does this deliberately — `codegraph` is an mcp
    /// entry *and* the feature containing it — so the two manifest lookups in
    /// `names` are ordered, not disjoint. Reversing them refuses
    /// `omh set codegraph off` with a sentence naming `codegraph` as part of
    /// `codegraph`, and the feature keeps no spelling at all.
    ///
    /// The end-to-end test that would catch a swap today does so only as a
    /// side effect of asserting a happy path, and would stop covering it the
    /// moment somebody rewrote that assertion.
    #[test]
    fn a_feature_named_after_its_own_entry_resolves_as_the_feature() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home").join(".omh"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.base()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        for f in bundled::Shipped::Base.files() {
            std::fs::write(paths.base().join(f.name), f.contents).unwrap();
        }

        let manifest = base::Manifest::load_dir(&paths.base()).unwrap();
        let both: Vec<&str> = manifest
            .entries
            .iter()
            .filter(|e| e.name == e.feature)
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            !both.is_empty(),
            "no feature is named after its own entry, so this test asserts \
             nothing — if that is deliberate, the ordering comment in `names` \
             is what needs changing"
        );
        for name in both {
            assert!(
                matches!(
                    cmd::settings::names(&paths, name, &out::Ctx::plain()),
                    cmd::settings::Names::AFeature
                ),
                "`{name}` is both a feature and an entry, and only the feature \
                 reading leaves it a spelling"
            );
        }
    }

    /// If the two vocabularies ever did collide, the setting wins.
    ///
    /// Defence in depth for the one thing the guard below forbids. The order
    /// in `names` looks arbitrary while nothing collides — the two arms share
    /// a dispatch branch, so a mutation removing the key check passes every
    /// end-to-end test. It is not arbitrary: a colliding name resolving as a
    /// *feature* would take `carry_in` out of `key::KEYS`' reach and strip the
    /// credential classification off it, and this is the direction that fails
    /// safe.
    ///
    /// Reachable only by handing `names` a manifest the guard would reject,
    /// which is exactly what this does.
    #[test]
    fn a_colliding_name_resolves_as_the_setting_not_the_feature() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home").join(".omh"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.base()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        // A manifest that names a feature after a credential-bearing setting.
        std::fs::write(
            paths.base().join("2026.08.toml"),
            "version = \"2026.08\"\n\n\
             [[entry]]\n\
             name    = \"collide\"\n\
             kind    = \"rules\"\n\
             feature = \"carry_in\"\n\
             since   = \"2026.08\"\n\
             because = \"a fixture\"\n\
             remove  = \"a fixture\"\n",
        )
        .unwrap();

        // The fixture has to actually collide, or this is green for the wrong
        // reason: `names` answers `ASetting` from `key::describes` before it
        // loads any manifest, so a fixture that stopped being a collision is
        // indistinguishable from one that is.
        assert!(
            matches!(
                cmd::settings::names(&paths, "collide", &out::Ctx::plain()),
                cmd::settings::Names::AnEntryOf(ref f) if f == "carry_in"
            ),
            "the fixture no longer names a feature after a settings key, so \
             the assertion below proves nothing"
        );
        assert!(
            matches!(
                cmd::settings::names(&paths, "carry_in", &out::Ctx::plain()),
                cmd::settings::Names::ASetting
            ),
            "a name in both vocabularies has to keep its credential \
             classification — resolving it as a feature is how `carry_in` \
             stops being one"
        );
    }

    /// No name is both a setting and one of omh's features.
    ///
    /// `omh set <name>` forks on this, and the fork writes two different
    /// shapes into the same file — a bare key with the value you typed, or a
    /// boolean in `[omh]`. A name in both vocabularies makes the command
    /// ambiguous in a way no error message could resolve, because both
    /// readings are things a person could reasonably have meant.
    ///
    /// It reads the **bundled** manifest rather than the repo's `base/`
    /// directory, because what ships is what a user's `omh set` will fork on.
    /// The two are held equal by `bundled`'s own guard, so this asserts the
    /// property one step closer to where it bites.
    #[test]
    fn no_name_is_both_a_setting_and_a_feature() {
        // **Every** shipped manifest, not the first. This took `.next()` off a
        // path-sorted list while `Manifest::load_dir` picks the newest by
        // declared version — one file today, so they coincide, and the day
        // `2027.01.toml` lands this would have gone on asserting the property
        // about the retired one while `names` forked on the new. Iterating all
        // of them needs no version parsing and is the stronger claim anyway.
        let manifests: Vec<base::Manifest> = bundled::Shipped::Base
            .files()
            .iter()
            .map(|f| toml::from_str(f.contents).expect("every shipped manifest parses"))
            .collect();
        assert!(!manifests.is_empty(), "omh ships a base manifest");

        let features: std::collections::BTreeSet<&str> = manifests
            .iter()
            .flat_map(|m| m.entries.iter())
            .map(|e| e.feature.as_str())
            .collect();
        assert!(
            !features.is_empty(),
            "no features at all, and this test asserts nothing"
        );
        assert!(
            !key::KEYS.is_empty(),
            "no settings keys at all, and this test asserts nothing"
        );

        for k in key::KEYS {
            assert!(
                !features.contains(k.name),
                "`{}` is both a setting omh reads and one of its features, so \
                 `omh set {} …` cannot be resolved",
                k.name,
                k.name
            );
        }

        // And the entry names, for the same reason one layer down: an entry
        // that shared a key's name would be refused as *part of a feature*
        // instead of being written as the setting it is.
        for k in key::KEYS {
            assert!(
                manifests.iter().all(|m| m.entry(k.name).is_none()),
                "`{}` is both a setting omh reads and a base-set entry",
                k.name
            );
        }
    }

    /// The rule never sends a credential-bearing key to a file git carries.
    ///
    /// This reads `rule` — the function `omh set`, `omh unset`, `omh use` and
    /// `omh unuse` all reach through — rather than `key_layer` underneath it.
    /// That distinction cost a review: the guard used to assert the table
    /// lookup while the command called something else, so the one step where a
    /// classified key can be sent somewhere other than the table said was the
    /// step no test read.
    ///
    /// Every combination of "already held here" is enumerated, because the
    /// held-layers branch is what would carry a secret into git: a repo whose
    /// committed file happens to hold `carry_in` must not thereby make the
    /// committed file the answer for an unadorned write.
    #[test]
    fn no_unadorned_write_sends_a_credential_key_into_git() {
        let secret: Vec<&str> = key::KEYS
            .iter()
            .filter(|k| k.secret == key::Secret::Yes)
            .map(|k| k.name)
            .collect();
        assert!(
            !secret.is_empty(),
            "with no credential-bearing key the loop below asserts nothing"
        );

        let every_shape = [
            vec![],
            vec![config::Layer::Shared],
            vec![config::Layer::Local],
            vec![config::Layer::Shared, config::Layer::Local],
        ];
        for name in &secret {
            for held in &every_shape {
                let reached = cmd::settings::rule(held.clone(), name, false, false);
                if held.contains(&config::Layer::Shared) {
                    // Already committed by hand. The rule joins it rather than
                    // splitting the value across two files, and `set` says so
                    // through the sharp warning — but it must not *invent* that
                    // destination, which the next case checks.
                    continue;
                }
                assert!(
                    !reached.committed(),
                    "`{name}` can name a credential, and an unadorned write \
                     with {held:?} already holding it reached a file git carries"
                );
            }
        }

        // And the flags still mean what they say, or the loop above passes by
        // reading `committed()` as false everywhere.
        assert!(
            cmd::settings::rule(vec![], "carry_in", false, true).committed(),
            "--save is how you say you meant it"
        );
        assert!(
            !cmd::settings::rule(vec![], "idle_timeout", true, false).committed(),
            "--local is the other half"
        );
    }

    /// The unclassified fallback, and the legacy commands' defaults.
    ///
    /// `no_unadorned_write_sends_a_credential_key_into_git` covers the keys
    /// the table knows. This covers the two things it cannot: a key the table
    /// has never seen, which is committed on purpose, and the two older
    /// spellings that still write settings while they are being retired.
    #[test]
    fn no_unqualified_write_can_reach_version_control() {
        // A key the table never saw is committed deliberately — flipping it to
        // the gitignored file would hide a typo rather than report it — and
        // that is only safe because
        // `every_setting_omh_reads_is_a_key_omh_can_classify` stops omh's own
        // keys from ever taking this path.
        assert!(
            key::describes("no-such-key-is-in-the-table").is_none(),
            "the assertion below stops meaning anything if the table gains this"
        );
        assert!(
            cmd::settings::rule(vec![], "no-such-key-is-in-the-table", false, false).committed(),
            "an unclassified key is committed on purpose"
        );

        // The template is not a file git carries, so `omh settings set` cannot
        // put a secret in one however the key is classified. The two other
        // clauses here were about the commands `omh set` replaced; both were
        // deleted in 0.7.0, and an assertion about a spelling that no longer
        // exists is not evidence of anything.
        assert!(
            !config::Layer::Personal.is_committed(),
            "omh settings set writes your own file"
        );
    }

    /// Whatever `init` says to run next is a command omh accepts.
    ///
    /// It named the bare harness from the day that stopped being a launch,
    /// and nothing noticed. The line is composed at runtime, so the scan that
    /// reads printed `omh …` literals cannot see it — and the doc transcripts
    /// were swept to `omh new claude`, which made the docs show output the
    /// binary does not produce and hid the break behind a green sweep.
    ///
    /// **Every line, now that there are three.** The list grew from one to a
    /// block, and a loop that read `said` as a single command would have gone
    /// on checking the first and ignoring the two under it — which are the
    /// two a first-time reader has never typed.
    #[test]
    fn what_init_says_to_run_next_is_a_command_omh_accepts() {
        for harness in [Some("claude"), Some("opencode"), None] {
            let lines = cmd::init::next_after_init(harness);
            assert!(
                !lines.is_empty(),
                "init has to leave somebody with something to run"
            );
            for (said, _) in &lines {
                let argv: Vec<String> = said.split_whitespace().map(str::to_string).collect();
                let (_, argv) = cli::session_prefix(argv);
                assert!(
                    Cli::try_parse_from(&argv).is_ok(),
                    "init tells a new user to run `{said}`, which omh does not accept"
                );
            }
        }
    }

    /// Only `dispatch` holds the whole `Cli`.
    ///
    /// This is what makes the scan below exact rather than hopeful. That scan
    /// reads the dispatch arms and asks which of them name `cli.session`; it
    /// can only be right if no arm hands the whole `Cli` to something that
    /// reads the session out of sight. `run` used to do exactly that, which is
    /// why the scan once carried a second clause matching a wholesale `cli`.
    ///
    /// The clause was formatting-dependent — it looked for `", cli,"`, so
    /// rustfmt breaking an argument list across lines silently disarmed it —
    /// and it was removed on the belief that it would otherwise misfire.
    /// cmd::init::Measured afterwards, it would not have: it had already been disarmed by
    /// exactly that formatting change, so removing it changed nothing and
    /// proved nothing.
    ///
    /// This replaces it with something that does not depend on how a line is
    /// wrapped. Keep the `Cli` in one function and the session cannot be read
    /// anywhere the scan does not look.
    #[test]
    fn only_dispatch_is_handed_the_whole_cli() {
        let whole = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )
        .unwrap();
        // The production half only. Below the test module the same spelling
        // appears inside this very scan, and a guard that counts itself is a
        // guard that can never reach one.
        let body = whole
            .split_once("#[cfg(test)]")
            .expect("the test module is still spelled this way")
            .0;
        let holders: Vec<&str> = body
            .match_indices("cli: &Cli")
            .map(|(at, _)| {
                // Walk back to the `fn` that owns this parameter.
                let head = &body[..at];
                let start = head.rfind("fn ").unwrap_or(0);
                let name = &body[start + 3..at];
                name.split(['(', '<']).next().unwrap_or("").trim()
            })
            .collect();
        assert_eq!(
            holders,
            vec!["dispatch"],
            "a function other than `dispatch` takes the whole `Cli`, so it can \
             read `cli.session` where the dispatch scan cannot see it"
        );
    }

    /// What `consumes_session` claims and what the dispatch does agree.
    ///
    /// The exhaustive match makes a new command a compile error until somebody
    /// classifies it. It cannot make them classify it *correctly* — and the
    /// wrong direction is silent: a command marked as reading the session that
    /// does not read it goes straight back to answering with the scope thrown
    /// away, which is the defect the predicate exists to end.
    ///
    /// So this reads both places out of the source and compares them: a
    /// variant whose dispatch arm mentions `cli.session` must be one
    /// `consumes_session` answers `true` for, and the reverse.
    ///
    /// Reading the source rather than calling the function is deliberate —
    /// building one `Cmd` of every variant to ask it would mean writing the
    /// list a third time, and the third copy is the one that rots.
    #[test]
    fn the_reads_and_the_refusals_agree_with_the_dispatch() {
        // Two files, read separately, because the halves this compares now
        // live apart: `dispatch` is the crate root's and `consumes_session`
        // went to `cli.rs` with the rest of the parse surface.
        //
        // Separately rather than concatenated, and the difference is not
        // stylistic. Joined, `split_once` finds this test's *own* string
        // literal naming the predicate before it reaches the predicate — so
        // the scan read its own source, found no arms, and the floor below
        // reported the function as unreadable. Each half is looked for in the
        // file that holds it, which has no such ambiguity.
        let read = |rel: &str| {
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
                .unwrap()
        };
        let body = read("src/main.rs");
        let surface = read("src/cli.rs");

        // Every `Cmd::` in a slice of source, ignoring `MemoryCmd::` and
        // friends — a suffix match would count `MemoryCmd::Remember` as a
        // top-level `Remember`, which is how the first version of this scan
        // grew a variant that does not exist.
        fn variants(text: &str) -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            let bytes = text.as_bytes();
            for (at, _) in text.match_indices("Cmd::") {
                if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                    continue;
                }
                let name: String = text[at + 5..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric())
                    .collect();
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            out
        }

        // Anchored on the function first: `session_prefix` matches on
        // `&cli.cmd` too, and splitting on that alone reads the wrong block —
        // silently, and with an empty result that would have looked like
        // agreement without the floors below.
        let dispatch = body
            .split_once("fn dispatch(cli: &Cli, ctx: &out::Ctx) -> Result<()> {")
            .expect("dispatch is still spelled this way")
            .1
            .split_once("match &cli.cmd {")
            .expect("dispatch still matches on cli.cmd")
            .1;

        let mut reads = std::collections::BTreeSet::new();
        let mut arm: Option<(String, String)> = None;
        // Naming `cli.session` is the whole rule, and it is exact only because
        // `only_dispatch_is_handed_the_whole_cli` holds: nothing outside this
        // function has a `Cli` to read a session out of.
        //
        // It once carried a second half — *or hands the whole `cli` down* —
        // for the days when `run` did. That clause matched the literal
        // `", cli,"`, so rustfmt wrapping an argument list across lines
        // disarmed it without a word, and it was disarmed by the time anyone
        // thought to remove it. Removing it therefore changed nothing, which
        // is also why removing it proved nothing.
        let consuming = |text: &str| text.contains("cli.session");
        for line in dispatch.lines() {
            if line == "    }" {
                break;
            }
            if line.starts_with("        Cmd::") {
                if let Some((name, text)) = arm.take() {
                    if consuming(&text) {
                        reads.insert(name);
                    }
                }
                let name = variants(line).into_iter().next().unwrap_or_default();
                arm = Some((name, line.to_string()));
            } else if let Some((_, text)) = arm.as_mut() {
                text.push_str(line);
            }
        }
        if let Some((name, text)) = arm {
            if consuming(&text) {
                reads.insert(name);
            }
        }

        // Variants the predicate answers `true` for. Arms are read by their
        // *result*, not by position: `matches!` counts as true because one of
        // that command's verbs reads the session.
        let predicate = surface
            .split_once("fn consumes_session(cmd: &Cmd) -> bool {")
            .expect("the predicate is still spelled this way")
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        let chunks: Vec<&str> = predicate.split("=>").collect();
        let mut claims = std::collections::BTreeSet::new();
        for i in 0..chunks.len().saturating_sub(1) {
            let result = chunks[i + 1].trim_start();
            if !(result.starts_with("true") || result.starts_with("matches!")) {
                continue;
            }
            // For any arm but the first, drop the previous arm's result before
            // reading the pattern.
            let pattern = if i == 0 {
                chunks[i]
            } else {
                chunks[i]
                    .split_once(',')
                    .map(|(_, rest)| rest)
                    .unwrap_or("")
            };
            claims.extend(variants(pattern));
        }

        assert!(
            !reads.is_empty(),
            "the scan found no dispatch arm reading the session, so its \
             agreement means nothing"
        );
        assert!(
            !claims.is_empty(),
            "the scan could not read `consumes_session`, so its agreement \
             means nothing"
        );
        assert_eq!(
            reads, claims,
            "these disagree about which commands act on a session.\n  \
             dispatch: {reads:?}\n  consumes_session: {claims:?}"
        );
    }

    // `reserved_lists_every_command_and_alias` and
    // `no_bundled_definition_shadows_a_command` were here. Both existed
    // because `omh <anything>` was a launch, so a command name and a harness
    // name shared one namespace and `RESERVED` was the list keeping them
    // apart. With no bare-name slot there is no shared namespace: an adapter
    // called `s` or `config` is reached by `omh new s`, and shadows nothing.
    // Deleted rather than left to fail, because what they guarded is gone.

    /// No name omh ships is both a harness and an editor.
    ///
    /// `omh new zed` and `omh s attach zed` are different commands reaching
    /// different directories, so a name in both is not ambiguous to the
    /// parser — it is ambiguous to the person, and to `tool_hint`, which
    /// answers "`zed` is an editor" for anything it finds in `editors/`. A
    /// name that is both would have `omh new` quietly use the adapter while
    /// the hint said the opposite.
    ///
    /// `no_bundled_definition_shadows_a_command` used to load both directories
    /// in one body — for a different reason, and it went with `RESERVED`. The
    /// namespace it half-covered is the one that still exists, so this is what
    /// should have replaced it rather than nothing.
    #[test]
    fn no_bundled_adapter_shares_a_name_with_an_editor() {
        let harnesses: std::collections::BTreeSet<String> =
            Adapter::load_dir(std::path::Path::new(BUNDLED_ADAPTERS))
                .unwrap()
                .into_iter()
                .map(|a| a.name)
                .collect();
        let editors: std::collections::BTreeSet<String> =
            editor::Editor::load_dir(std::path::Path::new(BUNDLED_EDITORS))
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect();
        assert!(
            !harnesses.is_empty() && !editors.is_empty(),
            "the scan read {} harnesses and {} editors, so its silence says \
             nothing",
            harnesses.len(),
            editors.len()
        );
        let both: Vec<&String> = harnesses.intersection(&editors).collect();
        assert!(
            both.is_empty(),
            "these are shipped as both a harness and an editor, so `omh new` \
             and the hint that follows it disagree: {both:?}"
        );
    }

    /// The grammar splits harnesses from editors, so the one mistake everybody
    /// will make is typing an editor where a harness goes. Say the fix.
    #[test]
    fn naming_an_editor_where_a_harness_goes_names_the_fix() {
        let hint = tool_hint("zed", &["claude".into()], &["zed".into()]);
        assert!(hint.contains("omh s attach zed"), "got: {hint}");
    }

    #[test]
    fn an_unknown_word_lists_the_harnesses() {
        let hint = tool_hint(
            "emacs",
            &["claude".into(), "opencode".into()],
            &["zed".into()],
        );
        assert!(
            hint.contains("claude") && hint.contains("opencode"),
            "got: {hint}"
        );
        assert!(!hint.contains("attach"), "not an editor: {hint}");
    }

    #[test]
    fn a_command_typed_as_a_harness_is_no_longer_a_thing_that_can_happen() {
        // This asserted that `config` — a command typed where a harness
        // went — pointed at `config --help` rather than reporting an
        // unknown harness. There is no "where a harness goes" any more: a bare
        // word is not a launch, so `config` is the command and cannot be
        // mistaken for anything. What survives is the editor half, which is
        // still a real confusion because editors and harnesses are both names.
        let hint = tool_hint("zed", &["claude".into()], &["zed".into()]);
        assert!(hint.contains("omh s attach zed"), "got: {hint}");
    }

    /// Regression: bundled definitions were written only if absent, so a fix
    /// omh shipped never reached anyone who had already run `init`. The one
    /// that mattered was a wrong credential path, which made auth silently
    /// capture nothing.
    #[test]
    fn bundled_definitions_are_refreshed_not_just_seeded() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("claude.toml"), "name = \"stale\"\n").unwrap();

        cmd::init::install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        let shipped =
            std::fs::read_to_string(std::path::Path::new(BUNDLED_ADAPTERS).join("claude.toml"))
                .unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("claude.toml")).unwrap(),
            shipped
        );
    }

    /// The refresh above is only acceptable because the old bytes survive it,
    /// and nothing asserted that they did — deleting the backup entirely kept
    /// the suite green.
    #[test]
    fn the_file_it_replaces_is_kept_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let mine = "name = \"mine, edited\"\n";
        std::fs::write(dest.join("claude.toml"), mine).unwrap();

        cmd::init::install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("claude.toml.yours")).unwrap(),
            mine,
            "the replaced file must be recoverable byte for byte"
        );
    }

    /// **And it is recoverable under a name that exists**, for every kind omh
    /// ships rather than only the TOML ones.
    ///
    /// The backup was named by *replacing* the extension with a literal
    /// `toml.yours`, which was right by accident while everything shipped was
    /// TOML and wrong the moment `hooks/` shipped JSON: an edited
    /// `rust-test.json` was saved as `rust-test.toml.yours` while the message
    /// on screen said `rust-test.json.yours`. Somebody looking for their edit
    /// where omh told them to look would not find it, and would reasonably
    /// conclude it had been discarded.
    ///
    /// Iterated over every shipped kind, because a guard written against
    /// adapters alone is a guard that passes on exactly the case that broke.
    #[test]
    fn a_replaced_file_is_kept_under_the_name_omh_names() {
        for kind in bundled::ALL {
            let d = tempfile::tempdir().unwrap();
            let dest = d.path().join(kind.dir());
            std::fs::create_dir_all(&dest).unwrap();
            let first = kind.files()[0].name;
            let mine = "this is what I wrote\n";
            std::fs::write(dest.join(first), mine).unwrap();

            cmd::init::install_bundled(&dest, kind, &out::Ctx::plain()).unwrap();

            // The name omh prints, spelled the way omh prints it.
            let backup = dest.join(format!("{first}.yours"));
            assert_eq!(
                std::fs::read_to_string(&backup).ok().as_deref(),
                Some(mine),
                "{}: an edit must be recoverable at {}",
                kind.dir(),
                backup.display()
            );
        }
    }

    /// A file omh cannot read as text is still a file somebody wrote.
    ///
    /// `read_to_string` fails on a single non-UTF-8 byte — one accented
    /// character pasted into a description is enough — and collapsing that to
    /// "absent" meant the overwrite went ahead with no backup and no message.
    /// The read failed, the write succeeded, and the edit was gone.
    #[test]
    fn an_edit_omh_cannot_read_as_text_is_still_backed_up() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let mine = b"name = \"caf\xe9\"\n"; // latin-1 é: valid file, invalid UTF-8
        std::fs::write(dest.join("claude.toml"), mine).unwrap();

        cmd::init::install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        assert_eq!(
            std::fs::read(dest.join("claude.toml.yours")).unwrap(),
            mine,
            "bytes omh cannot decode are still bytes it must not discard"
        );
    }

    /// Definitions you add yourself are yours; omh only manages its own.
    #[test]
    fn definitions_omh_does_not_ship_are_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("mine.toml"), "name = \"mine\"\n").unwrap();

        cmd::init::install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("mine.toml")).unwrap(),
            "name = \"mine\"\n"
        );
    }

    /// A second thing on the line is a flag, not a position.
    ///
    /// `auth` and `doctor` each took an optional bare word after their first
    /// argument — an account for one, a harness for the other. Two problems.
    /// A reader cannot tell `omh auth claude work` from `omh auth work claude`
    /// without already knowing which slot is the account — both are well formed
    /// under the old grammar and they mean different things. And `doctor` is due
    /// a `--session` as well, at which point a bare word beside two flags is the
    /// odd one out.
    ///
    /// Named, both read as what they are, and the account keeps its default
    /// so the common line does not grow.
    #[test]
    fn the_optional_second_word_is_named_rather_than_positional() {
        for line in [
            vec!["omh", "auth", "claude", "--name", "work"],
            vec!["omh", "auth", "claude", "-n", "work"],
            vec!["omh", "auth", "claude"],
            vec!["omh", "doctor", "--harness", "claude"],
            vec!["omh", "doctor"],
            // The alias is advertised in two command tables, and nothing
            // asserted it exists — `every_alias_is_a_single_letter` only checks
            // that whatever aliases *are* there are one character long.
            vec!["omh", "d", "--harness", "claude"],
        ] {
            assert!(
                Cli::try_parse_from(&line).is_ok(),
                "omh accepts `{}`",
                line[1..].join(" ")
            );
        }
        // And the positional forms are gone rather than quietly still working,
        // which is the half a rename that only adds the new spelling skips.
        for line in [
            vec!["omh", "auth", "claude", "work"],
            vec!["omh", "doctor", "claude"],
            // The global `-a`/`--account` names the same thing and is refused
            // here, which is worth pinning because the reason is incidental:
            // this field is still literally called `account`, so it collides
            // with the global's clap id. Rename the field and the global would
            // silently start being accepted-and-discarded on `auth` — measured,
            // and nothing else in the suite would have noticed.
            vec!["omh", "auth", "claude", "-a", "work"],
            vec!["omh", "auth", "claude", "--account", "work"],
        ] {
            let Err(refused) = Cli::try_parse_from(&line) else {
                panic!("`{}` is not a line omh takes any more", line[1..].join(" "));
            };
            assert_eq!(
                refused.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "`{}` is refused as an argument omh does not know, not as \
                 something else that happens to fail",
                line[1..].join(" ")
            );
        }
    }

    /// Every argument says what it is.
    ///
    /// A flag gets a description for free — it is written as a field with a doc
    /// comment, and leaving that off looks wrong on the page. A positional does
    /// not: `harness: String` compiles, reads fine in source, and renders in
    /// `--help` as a bare `<HARNESS>` with an empty column beside it. Twelve of
    /// them had accumulated that way, including on `auth`, where the whole
    /// command is one positional and one flag.
    ///
    /// Walked from the parser rather than listed, and recursive, because half
    /// of them were nested two levels down under `config mcp add` and `repo
    /// set` where no top-level scan would have looked.
    #[test]
    fn every_argument_says_what_it_is() {
        fn walk(cmd: &clap::Command, path: &str, bare: &mut Vec<String>) {
            for arg in cmd.get_positionals() {
                if arg.get_help().is_none() {
                    bare.push(format!("{path} <{}>", arg.get_id()));
                }
            }
            for sub in cmd.get_subcommands() {
                walk(sub, &format!("{path} {}", sub.get_name()), bare);
            }
        }
        let mut bare = Vec::new();
        let root = Cli::command();
        walk(&root, "omh", &mut bare);
        // A walk that descended into nothing would agree with everything.
        let mut seen = 0;
        fn count(cmd: &clap::Command, seen: &mut usize) {
            *seen += cmd.get_positionals().count();
            for sub in cmd.get_subcommands() {
                count(sub, seen);
            }
        }
        count(&root, &mut seen);
        assert!(
            seen > 15,
            "the walk found {seen} arguments, fewer than omh has"
        );
        assert!(
            bare.is_empty(),
            "these render in `--help` as a name with nothing beside it: {bare:#?}"
        );
    }

    /// Aliases only earn their keep if they are actually short.
    ///
    /// **Recursively**, and that is not a tidy-up. It walked the top level
    /// only, so moving `attach` under `sessions` would not have turned this
    /// red — it would have made it *blind*, and a guard that stops seeing a
    /// thing looks exactly like a guard that approves of it. The floor below
    /// is what keeps the recursion honest: it has to reach more aliases than
    /// the top level holds, or the walk has quietly stopped descending.
    #[test]
    fn every_alias_is_a_single_letter() {
        fn walk(cmd: &clap::Command, seen: &mut usize) {
            for sub in cmd.get_subcommands() {
                for alias in sub.get_visible_aliases() {
                    *seen += 1;
                    assert_eq!(alias.chars().count(), 1, "`{alias}` is not a shortcut");
                }
                walk(sub, seen);
            }
        }
        let root = Cli::command();
        let mut seen = 0;
        walk(&root, &mut seen);

        // **Structural, not numeric.** A count floor answers "did it descend at
        // all", which a hard-coded single extra level satisfies while `config
        // mcp <verb>` at depth three goes unvisited. It also rested on there
        // being a nested alias to find — retire the one and the guard accuses
        // the recursion of a fault that belongs to an empty set.
        //
        // Naming the deep paths asks the question that matters: did the walk
        // reach here. `config mcp` is the deepest the tree goes.
        fn reached(cmd: &clap::Command, path: &[&str]) -> bool {
            match path {
                [] => true,
                [head, rest @ ..] => cmd
                    .get_subcommands()
                    .find(|s| s.get_name() == *head)
                    .is_some_and(|s| reached(s, rest)),
            }
        }
        for path in [
            &["sessions"][..],
            &["sessions", "attach"][..],
            &["settings", "mcp"][..],
            &["settings", "mcp", "add"][..],
        ] {
            assert!(
                reached(&root, path),
                "`omh {}` is not reachable by the same walk this test makes, so \
                 whatever it checked, it did not check there",
                path.join(" ")
            );
        }
        assert!(seen > 0, "no aliases at all, and the loop asserted nothing");
    }

    // ── omh's flags versus the harness's ────────────────────────────────────
    //
    // `passthrough`, `omh_globals` and six tests were here. They arbitrated a
    // question the grammar no longer asks: with the bare name gone, both
    // launch spellings take their harness arguments after a `--`, so clap's
    // `last = true` decides whose a flag is before anything of omh's runs.
    // `passthrough` could still be *called*, but its refusal had become
    // unreachable — its callers inserted the separator it broke on — and its
    // remedy named two spellings this release deleted. A guard that cannot
    // fire, advising commands that do not parse, is worse than no guard.
    //
    // What replaced it is `omh_new_gives_the_harness_only_what_follows_a_double_dash`
    // in tests/cli.rs, which asks the parser rather than a second opinion.

    /// Under `omh new`, `--` is how a flag reaches the harness — and the only
    /// way.
    ///
    /// The bare-name form had to guess: a `--json` after the harness could be a
    /// request to omh or an argument for the harness, and `passthrough` resolved it by
    /// refusing omh's own long flags and leaving shorts alone. That rule is a
    /// judgement about which mistake is likelier, and `src/main.rs`'s own
    /// comment on it admits as much.
    ///
    /// `omh new` does not guess. Everything before `--` is omh's, everything
    /// after it is the harness's, and there is no third category. So
    /// `omh new claude --json` reports omh as JSON, while
    /// `omh new claude -- --json` hands `--json` to claude — including for
    /// flags omh also has, which is the case the bare form cannot express at
    /// all without `--` either.
    ///
    /// This is a parse-level test on purpose: it is the parser that decides
    /// which side of the separator a token lands on, and a parse test runs on
    /// every platform rather than only where a container runtime exists.
    #[test]
    fn omh_new_gives_the_harness_only_what_follows_a_double_dash() {
        let after = Cli::try_parse_from(cli_argv(&["new", "claude", "--", "--json"]))
            .expect("`--` is how a harness flag is spelled here");
        assert!(
            !after.json,
            "`--json` after `--` is claude's, so omh must not have taken it"
        );
        match after.cmd {
            Cmd::New { harness, args } => {
                assert_eq!(harness, "claude");
                assert_eq!(args, vec!["--json".to_string()], "and claude gets it");
            }
            _ => panic!("expected a launch"),
        }

        // Before the separator it is omh's, and that is not a bug to be fixed
        // later — it is what makes the separator mean something.
        let before = Cli::try_parse_from(cli_argv(&["new", "claude", "--json"]))
            .expect("omh's own flag, in omh's own position");
        assert!(before.json, "`--json` before `--` is omh's");
        match before.cmd {
            Cmd::New { args, .. } => assert!(args.is_empty(), "nothing was handed on"),
            _ => panic!("expected a launch"),
        }

        // A short flag omh also has. The bare-name form leaves shorts to the
        // harness by a rule; here the separator says so outright.
        //
        // `-s`, because `-a` is not omh's any more: the account is the setting
        // and there is no flag to collide with. `-s` is the remaining short
        // that would otherwise be read as omh's.
        let short = Cli::try_parse_from(cli_argv(&["new", "claude", "--", "-s", "work"]))
            .expect("a short flag after the separator");
        assert!(short.session.is_none(), "`-s` after `--` is claude's");
        match short.cmd {
            Cmd::New { args, .. } => assert_eq!(args, vec!["-s".to_string(), "work".to_string()]),
            _ => panic!("expected a launch"),
        }
    }

    /// **Nothing in this file writes to a stream directly.**
    ///
    /// The reason `out::Ctx` exists at all: 197 `println!`s here meant the
    /// wording could not be tested, the same fact was phrased two ways in two
    /// commands, and `--json` had nowhere to hook in. All of them now go
    /// through `Ctx`, and this is what stops the 198th being added — the pull
    /// is real, because a bare `println!` is one line and a report type is
    /// twenty.
    ///
    /// Two exemptions, both named:
    ///
    /// - `main` itself renders the error and must write it without a `Ctx`
    ///   method, because a `Ctx` method is what it would be reporting about.
    /// - `memory_serve` speaks MCP on stdout. What it writes is protocol, not
    ///   a report, and one report-shaped line would break the handshake.
    ///
    /// Read off the source rather than enforced by the type system, which
    /// cannot express "no macro calls here". A grep in a test is cruder than a
    /// lint and catches the same mistake at the same moment.
    #[test]
    fn no_command_writes_to_a_stream_behind_the_output_layer() {
        // **The whole tree, not `main.rs`.** This read one file for as long as
        // it existed, which is why `memory/deliver.rs` could write straight to
        // stderr — bypassing `--json` and `--color` — and dump a cargo build
        // log as the first thing a new user sees after `init` tells them to run
        // `omh new`. A guard that reads one file makes a claim about one file
        // and was cited as a claim about the program.
        let files = rust_sources(&["src"]);
        the_whole_tree(&files);

        let mut offenders: Vec<String> = Vec::new();
        for file in &files {
            // `out.rs` **is** the output layer. Every macro in it is the
            // implementation of the methods this rule is about, so exempting
            // the file is exempting the thing rather than making a hole in it.
            if file.file_name().is_some_and(|n| n == "out.rs") {
                continue;
            }
            let source = std::fs::read_to_string(file).expect("a source this crate compiles");
            let name = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(file)
                .display()
                .to_string();
            // Below the test module is fixture and assertion text, not
            // anything omh prints — the same cut the printed-line scan makes,
            // and for the same reason.
            let source = match source.find("\nmod tests {") {
                Some(at) => source[..at].to_string(),
                None => source,
            };
            for (i, line) in source.lines().enumerate() {
                let line = line.trim();
                // Only calls, never the word in a doc comment or a string that
                // talks *about* the rule — this very comment mentions
                // `println!`.
                if ["println!", "print!(", "eprintln!", "eprint!("]
                    .iter()
                    .any(|m| line.starts_with(m))
                {
                    offenders.push(format!("{name}:{}: {line}", i + 1));
                }
            }
        }

        // **Declared, not counted.** Widening this scan from one file to the
        // tree turned up ten sites, and the honest thing is to name every one
        // rather than assert a number that goes stale the first time somebody
        // moves a line.
        //
        // Two are the exemptions this rule has always had: the error renderer
        // in `main`, which cannot report through the thing it is reporting
        // about, and the MCP line reader, which speaks protocol.
        //
        // The tenth is neither: it is a relay of a child's stream, listed
        // separately below.
        //
        // The other seven are **owed a fix, not excused one**. They predate the
        // scan reading their files at all, and every one writes an `omh: ` line
        // straight to stderr — no palette, and not suppressed under `--json`,
        // which is what `progress` and `announce` exist to do. Routing them
        // means threading a `Ctx` through `image`, `facts` and the delivery
        // path, roughly sixteen call sites, one of which (`sandbox`) has none
        // in scope. That is its own change; doing it inside a release fix would
        // be a large diff in the wrong week. The list is here so it cannot grow
        // quietly in the meantime — an eighth entry fails this test.
        // Named by what the line *is*, not by where it sits. The first draft
        // pinned `file:line` and went stale the moment anything above it grew,
        // which is a guard failing for a reason that has nothing to do with
        // what it guards.
        let named = [
            // The two real exemptions.
            ("src/main.rs", "out::problem"),
            ("src/mcp.rs", "omh-mcp: ignoring unparseable line"),
            // Owed a `Ctx`. See above.
            ("src/facts.rs", "could not read"),
            // Wrapped, so the macro line carries no text of its own — the
            // only one of these where the file alone has to do it.
            ("src/facts.rs", "eprintln!("),
            ("src/image.rs", "this repo's toolchain, first run only"),
            ("src/image.rs", "(first run only)"),
            ("src/image.rs", "omh: building {t}"),
            ("src/image.rs", "could not list images to reap"),
            ("src/image.rs", "this build replaced"),
        ];
        // **A relay is not a message.** These do not write anything omh
        // composed — they hand a child process's stream back to the terminal
        // verbatim, which is what `Stdio::inherit` did before omh needed to
        // read the stream as well as show it. So they are not debts and are
        // counted separately: the seven above stay seven, and this list may
        // not quietly become a second way in.
        //
        // `build` reads docker's stderr so it can say *why* a build failed —
        // behind a TLS-inspecting proxy the answer is `ca_cert`, and doctor
        // cannot answer it because there is no image to inspect. Reading it
        // means relaying it, or a multi-minute build goes silent.
        let relayed = [
            ("src/image.rs", "eprint!(\"{text}\")"),
            ("src/image.rs", "omh: lost the rest of the build log"),
        ];

        let unexpected: Vec<&String> = offenders
            .iter()
            .filter(|o| {
                !named
                    .iter()
                    .chain(relayed.iter())
                    .any(|(file, what)| o.starts_with(file) && o.contains(what))
            })
            .collect();
        assert!(
            unexpected.is_empty(),
            "every write goes through out::Ctx but the sites named in this test — \
             found {unexpected:#?}"
        );
        for (file, what) in named.iter().chain(relayed.iter()) {
            assert!(
                offenders
                    .iter()
                    .any(|o| o.starts_with(file) && o.contains(what)),
                "{file} no longer writes `{what}` — a stale exemption is a hole \
                 this test says is not there"
            );
        }
        assert_eq!(
            named.len(),
            9,
            "the debt register grew. Two exemptions and seven sites owed a \
             `Ctx` — an eighth owed site is a fix, not an entry"
        );
        // The comment above `relayed` says it "may not quietly become a second
        // way in", and nothing made that true: only `named` was counted, so
        // appending a bare `eprintln!` here was green with no number moving.
        // A claim about containment that is not measured is the shape this
        // whole guard exists to catch.
        assert_eq!(
            relayed.len(),
            2,
            "a relay is a child's stream handed back verbatim — the two are \
             `build`'s log line and the one note it prints when that log ends \
             early. A third is a claim somebody has to justify, not a line to add"
        );
    }

    // ── candidate guards (mutation testing) ─────────────────────────────────

    /// **Only "not found" means absent.** A bundled file omh cannot open — a
    /// permission it does not have, a name that is now a directory — is not a
    /// file that is not there, and collapsing the two overwrites somebody's
    /// edit with no backup and no message. The read fails, the write succeeds,
    /// and the edit is gone.
    ///
    /// The non-UTF-8 half of this rule is guarded one test up. This is the
    /// other half, and deleting it changed no test.
    #[test]
    #[cfg(unix)]
    fn an_edit_omh_cannot_open_at_all_is_never_treated_as_absent() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let target = dest.join("claude.toml");
        std::fs::write(&target, "name = \"mine\"\n").unwrap();
        // Write-only, deliberately. A mode omh can neither read nor write
        // would be caught by the *write* failing, which proves nothing about
        // the read — this is the shape the shipped bug actually had: the read
        // failed, the write succeeded, and the edit was gone.
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o200)).unwrap();

        let outcome =
            cmd::init::install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

        let e = format!(
            "{:#}",
            outcome.expect_err("a file omh could not read must not be overwritten in silence")
        );
        assert!(e.contains("claude.toml"), "and it names the file: {e}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "name = \"mine\"\n",
            "and the edit is still there"
        );
    }

    /// **A hook belonging to an ecosystem this repo is not stays out of the
    /// list; everything else stays in.** Both halves, because the filter has
    /// two ways to be wrong and one of them offers a rust repo `go-test` while
    /// the other drops every hook that belongs to nothing in particular —
    /// which is most of them.
    #[test]
    fn a_hook_that_belongs_to_nothing_is_offered_everywhere() {
        let declared = BTreeMap::from([
            ("rust-test".to_string(), Some("rust".to_string())),
            ("go-test".to_string(), Some("go".to_string())),
            ("graph-refresh".to_string(), None),
        ]);
        let names = vec![
            "rust-test".to_string(),
            "go-test".to_string(),
            "graph-refresh".to_string(),
            "never-declared".to_string(),
        ];
        let rust: BTreeSet<String> = ["rust".to_string()].into_iter().collect();

        assert_eq!(
            cmd::catalogue::applicable_hooks(names, &declared, &rust),
            vec![
                "rust-test".to_string(),
                "graph-refresh".to_string(),
                "never-declared".to_string()
            ],
            "an ecosystem hook is filtered by the ecosystem; nothing else is"
        );
    }
    /// `main.rs` stays small enough to read.
    ///
    /// It reached 8,434 lines of production code and about a hundred
    /// top-level functions — `init` alone was 575 lines — while the other
    /// forty modules stayed cleanly bounded. Nothing failed because of it;
    /// that is the point. A module grows this way one reasonable commit at a
    /// time, and no test in the suite has an opinion about it, so the only
    /// thing that ever pushed back was somebody reading the file and minding.
    ///
    /// A budget is a crude instrument and deliberately so: it cannot say
    /// whether a function belongs here, only that the file has stopped being
    /// one. What it does is make the next accretion an argument rather than a
    /// default — raise it on purpose, or put the code where it goes.
    ///
    /// **Production lines only, and the tests below have not moved with the
    /// code they guard.** That is a debt, not a design: they share helpers and
    /// reach the command functions through `dispatch`, so relocating ~5,000
    /// lines of them is its own change with its own risk, and doing it in the
    /// same breath as the move would have put both beyond review. The
    /// production split is the part that makes the file navigable; this test
    /// is what stops it filling up again while that debt is outstanding.
    ///
    /// Counting tests here would be wrong in any case, for the reason it is
    /// wrong today: it would reward moving a function out and leaving its
    /// guard behind, which is the opposite of what should happen next.
    #[test]
    fn the_crate_root_stays_a_crate_root() {
        const BUDGET: usize = 1_500;
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )
        .unwrap();
        let prod = body
            .lines()
            .position(|l| l.starts_with("#[cfg(test)]"))
            .unwrap_or(body.lines().count());
        assert!(
            prod <= BUDGET,
            "src/main.rs holds {prod} lines of production code, over a budget of \
             {BUDGET}. Move a command into `src/cmd/`, or raise the budget \
             deliberately and say why"
        );
    }
}
