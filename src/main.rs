//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh claude          omh opencode          omh codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod auth;
mod base;
mod bundled;
mod carry;
mod config;
mod container;
mod detect;
mod doctor;
mod editor;
mod facts;
mod hook;
mod idle;
mod image;
mod mcp;
mod memory;
mod notice;
mod persist;
mod profile;
mod render;
mod rules;
mod runtime;
mod selection;
mod session;
mod settings;
mod ssh;
mod stack;
mod why;

use adapter::Adapter;
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use profile::{Paths, Profile};
use session::Session;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "omh", version, about, long_about = None)]
struct Cli {
    /// Print the launch plan instead of running it.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Reuse an existing session instead of creating a new one.
    #[arg(long, short, global = true)]
    session: Option<String>,

    /// Start a fresh session instead of resuming the most recent one.
    ///
    /// Refused alongside `--session`, which names one: `session::pick` returns
    /// the explicit id and never looks at `new`, so the two together used to
    /// resolve by quietly dropping one of them.
    #[arg(long, global = true, conflicts_with = "session")]
    new: bool,

    /// Which captured account to log in as.
    #[arg(long, short = 'a', global = true)]
    account: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

/// omh's own long flags, taken from the parser rather than written out.
///
/// A list would rot: the next global flag added would fall outside the guard
/// below without anyone noticing, which is exactly how the guarded mistake
/// happens in the first place.
fn omh_globals() -> Vec<String> {
    use clap::CommandFactory;
    Cli::command()
        .get_arguments()
        .filter(|a| a.is_global_set())
        .filter_map(|a| a.get_long().map(|long| format!("--{long}")))
        .collect()
}

/// The harness's arguments, refusing any of omh's own flags among them.
///
/// `omh <harness> …` takes everything after the name as the harness's argv, so
/// `omh opencode --dry-run` handed omh's flag to opencode and launched for
/// real. Silent, and worst for exactly the flag whose meaning is "change
/// nothing" — so this refuses rather than warns, and says the form that works.
///
/// Long forms only. `-s` is omh's session flag and is also a flag plenty of
/// harnesses have; refusing shorts would break launches that work today to
/// guard a mistake nobody has made. `--` ends the inspection and is consumed,
/// for the day a harness really does have `--new`.
fn passthrough(argv: &[String], globals: &[String]) -> Result<Vec<String>> {
    let mut out = vec![argv[0].clone()];
    let mut rest = argv[1..].iter();
    for arg in rest.by_ref() {
        if arg == "--" {
            break;
        }
        if globals.iter().any(|g| g == arg) {
            anyhow::bail!(
                "`{arg}` is omh's flag, not {}'s, and everything after a harness \
                 name belongs to the harness\n  \
                 try  omh {arg} {}\n  \
                 or   omh {} -- {arg}   to pass it on regardless",
                argv[0],
                argv[0],
                argv[0]
            );
        }
        out.push(arg.clone());
    }
    out.extend(rest.cloned());
    Ok(out)
}

/// Built-ins and their aliases always beat a harness name — otherwise an
/// adapter called `s` or `config` would silently shadow a command.
pub const RESERVED: [&str; 18] = [
    "init", "doctor", "d", "auth", "ls", "attach", "a", "sessions", "s", "config", "c", "graph",
    "why", "memory", "help", "use", "unuse", "repo",
];

#[derive(Subcommand)]
enum Cmd {
    /// Set this repo up. Decides everything; asks nothing.
    Init,
    /// Verify a harness actually sees the profile, inside a real sandbox.
    #[command(visible_alias = "d")]
    Doctor { harness: Option<String> },
    /// Who put this here, and on what grounds.
    Why {
        /// A base-set entry, something you added, or something omh rejected.
        thing: String,
    },
    /// Open the code graph in your browser.
    Graph {
        session: Option<String>,
        /// Stop the graph server; the session keeps running.
        #[arg(long)]
        stop: bool,
    },
    /// Log a harness in once. Repeat with different names for several accounts.
    Auth {
        harness: String,
        /// Account name, e.g. `personal` or `work`.
        #[arg(default_value = auth::DEFAULT_ACCOUNT)]
        account: String,
    },
    /// What you have here: harnesses, editors, sessions.
    Ls,
    /// Open a session in an editor, over SSH.
    #[command(visible_alias = "a")]
    Attach {
        /// Defaults to $OMH_EDITOR or $EDITOR.
        editor: Option<String>,
    },
    /// Work with sessions.
    #[command(visible_alias = "s")]
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },
    /// Your defaults and your catalogue, or change them.
    #[command(visible_alias = "c")]
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
    /// This checkout: what it uses, what it decided, and what decided it.
    Repo {
        #[command(subcommand)]
        cmd: Option<RepoCmd>,
    },
    /// Select a catalogue entry for this repo. Writes the committed file: what
    /// a project uses is a fact about the project, and a teammate cloning it
    /// should get the same one.
    Use {
        /// One of rules, skills, mcp, commands, subagents, hooks.
        capability: Option<String>,
        name: Option<String>,
        /// Resync every list to the whole catalogue.
        #[arg(long)]
        all: bool,
    },
    /// Stop using a catalogue entry here.
    Unuse { capability: String, name: String },
    /// The note store: what is in it, and what is wrong with it.
    Memory {
        #[command(subcommand)]
        cmd: Option<MemoryCmd>,
    },
    /// Anything else is a harness: `omh claude`, `omh opencode`.
    #[command(external_subcommand)]
    Run(Vec<String>),
}

#[derive(Subcommand)]
enum McpCmd {
    /// Servers, with the layer each comes from.
    Ls,
    /// Add a server to your catalogue.
    Add {
        name: String,
        command: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
    },
    /// Remove a server from your catalogue.
    Rm { name: String },
    /// Import servers you already configured in an installed harness.
    Import {
        harness: String,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// Sessions, their branches, and how far they have drifted.
    Ls,
    /// Remove a session — its container and its worktree. A branch holding
    /// commits is kept.
    Rm { session: String },
    /// Stop a sandbox. The worktree and branch survive.
    Down { session: Option<String> },
    /// What a session changed, against its base branch.
    Diff {
        session: Option<String>,
        /// Defaults to the repo's own default branch.
        #[arg(long)]
        base: Option<String>,
    },
    /// Commit a session's work onto its branch. Run on the host: the sandbox
    /// has no git, and the worktree omh keeps out of your way is not somewhere
    /// you should have to go.
    Commit {
        /// The message, verbatim. Without it, git opens your editor.
        #[arg(short = 'm', long)]
        message: Option<String>,
        /// Commit without the files omh carried in from your checkout.
        #[arg(long)]
        skip_carried: bool,
    },
    /// Push a session's branch to origin under a name a reviewer can read.
    Push {
        /// The branch name on origin. Required the first time, remembered after.
        name: Option<String>,
        /// Open a pull request with `gh` once it is pushed.
        #[arg(long)]
        pr: bool,
    },
}

/// Two scopes, so two commands. `omh config` narrows to mean **you** — your
/// catalogue and your defaults. `omh repo` means **this checkout**.
///
/// `--layer` used to carry both, and it strained because the two want opposite
/// defaults: what a project *uses* is a fact about the project and should be
/// committed, while what a project *overrides* holds `carry_in` paths and MCP
/// env and must not be committable by accident. One flag cannot express two
/// opposite defaults.
#[derive(Subcommand)]
enum ConfigCmd {
    /// Set one of your defaults, in `~/.omh/settings.toml`.
    Set {
        key: String,
        value: String,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// Remove one of your defaults.
    Unset {
        key: String,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// Open your settings, or one catalogue entry, in $EDITOR.
    Edit {
        /// One of rules, skills, mcp, commands, subagents, hooks. Without it,
        /// your settings file.
        capability: Option<String>,
        /// Which entry. Without it, the capability's directory.
        name: Option<String>,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// MCP servers — configuration, so it lives here.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Switch one of omh's features on here.
    Enable { feature: String },
    /// Switch one of omh's features off here. Nothing is uninstalled.
    Disable { feature: String },
    /// Set a value for this checkout. Gitignored by default, because these
    /// carry `carry_in` paths and MCP env and a mistyped key must not be
    /// committable by accident.
    Set {
        key: String,
        value: String,
        /// Write the committed file instead, and say so.
        #[arg(long)]
        shared: bool,
    },
    /// Remove a value, letting any lower layer resurface.
    Unset {
        key: String,
        #[arg(long)]
        shared: bool,
    },
}

/// Deliberately short. `promote` and `stale` arrive with the layers and the
/// expiry events they act on; a subcommand that prints "not implemented" is
/// worse than its absence, because `--help` advertises it.
#[derive(Subcommand)]
enum MemoryCmd {
    /// Record what surprised you. Writes to the gitignored layer, always.
    Remember {
        /// What you thought would happen.
        #[arg(long)]
        expected: String,
        /// What actually happened.
        #[arg(long)]
        observed: String,
        /// The command, the error, the file.
        #[arg(long)]
        evidence: String,
        /// A question this note answers, as somebody would later ask it.
        /// Repeat for several. A note nobody can find is a note nobody wrote.
        #[arg(long = "answers")]
        answers: Vec<String>,
        /// Keys of notes this connects to. Keys, not titles: a key is
        /// computable before its target exists.
        #[arg(long = "relates-to")]
        relates_to: Vec<String>,
        /// One of the closed set omh can evaluate itself.
        #[arg(long)]
        invalidated_by: Option<String>,
        /// Who observed it. Defaults to this session when there is one.
        #[arg(long)]
        source: Option<String>,
        /// What to do when the derived key is taken. Skipping is a mode you
        /// ask for, never a fallback — as a fallback every real conflict
        /// disappears silently.
        #[arg(long, value_parser = parse_if_exists, default_value = "error")]
        if_exists: memory::IfExists,
    },
    /// Speak MCP on stdin/stdout. Launched by the harness, not by you.
    ///
    /// Hidden because it is a wire protocol, not a command: it prints JSON-RPC
    /// frames and waits, which is indistinguishable from a hang if you run it
    /// by hand. Paths arrive as arguments because this runs inside the
    /// sandbox, where there is no repo to discover.
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        team: std::path::PathBuf,
        #[arg(long)]
        local: std::path::PathBuf,
        /// The session this server serves. Defaults to `$OMH_SESSION`, which
        /// omh already sets in the sandbox — so the base set can declare
        /// static arguments and still record real provenance.
        #[arg(long)]
        session: Option<String>,
    },
    /// Share a note with the repo: local → team. The only human gate there is.
    Promote {
        /// One or more keys. Notes that link to each other must be named
        /// together, or each would leave the other dangling for a teammate.
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Notes the world has moved on from. A join, never a judgement.
    Stale,
    /// Schema and hygiene violations, across both layers.
    Lint,
    /// Remove one note. Never a neighbour; reports what linked to it.
    Rm {
        key: String,
        #[arg(long, value_parser = parse_note_layer)]
        layer: Option<memory::Layer>,
        /// Which file, when one key somehow reached two of them. Path
        /// relative to the layer's root, as `rm` prints it.
        #[arg(long)]
        at: Option<String>,
    },
}

fn main() -> Result<()> {
    // A closed pipe (`omh ls | head`) is not a crash. Without this, Rust's
    // default panics on the failed write and prints a backtrace.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    match &cli.cmd {
        Cmd::Init => init(&cwd),
        Cmd::Auth { harness, account } => auth_cmd(&cwd, harness, account),
        Cmd::Ls => ls(&cwd),
        Cmd::Doctor { harness } => doctor_cmd(&cwd, harness.as_deref(), cli.dry_run),
        Cmd::Why { thing } => why_cmd(&cwd, thing),
        Cmd::Graph { session, stop } => graph(&cwd, session.as_deref(), *stop),
        Cmd::Attach { editor } => attach(&cwd, cli.session.as_deref(), editor.as_deref()),

        Cmd::Sessions { cmd } => match cmd {
            SessionsCmd::Ls => sessions_ls(&cwd),
            SessionsCmd::Rm { session } => rm(&cwd, session),
            SessionsCmd::Down { session } => down(&cwd, session.as_deref()),
            SessionsCmd::Diff { session, base } => diff(
                &cwd,
                session.as_deref().or(cli.session.as_deref()),
                base.as_deref(),
            ),
            SessionsCmd::Commit {
                message,
                skip_carried,
            } => commit(
                &cwd,
                cli.session.as_deref(),
                message.as_deref(),
                *skip_carried,
            ),
            SessionsCmd::Push { name, pr } => {
                push(&cwd, cli.session.as_deref(), name.as_deref(), *pr)
            }
        },

        Cmd::Config { cmd } => match cmd {
            None => show_config(&cwd),
            Some(ConfigCmd::Set { key, value, layer }) => {
                set(&cwd, key, value, layer_or(*layer, config::Layer::Personal))
            }
            Some(ConfigCmd::Unset { key, layer }) => {
                unset(&cwd, key, layer_or(*layer, config::Layer::Personal))
            }
            Some(ConfigCmd::Edit {
                capability,
                name,
                layer,
            }) => edit(
                &cwd,
                capability.as_deref(),
                name.as_deref(),
                layer_or(*layer, config::Layer::Personal),
            ),
            Some(ConfigCmd::Mcp { cmd }) => mcp(&cwd, cmd, cli.dry_run),
        },

        Cmd::Repo { cmd } => match cmd {
            None => show_repo(&cwd),
            Some(RepoCmd::Enable { feature }) => feature_switch(&cwd, feature, true),
            Some(RepoCmd::Disable { feature }) => feature_switch(&cwd, feature, false),
            Some(RepoCmd::Set { key, value, shared }) => set(&cwd, key, value, repo_layer(*shared)),
            Some(RepoCmd::Unset { key, shared }) => unset(&cwd, key, repo_layer(*shared)),
        },

        Cmd::Use {
            capability,
            name,
            all,
        } => use_cmd(&cwd, capability.as_deref(), name.as_deref(), *all),
        Cmd::Unuse { capability, name } => unuse_cmd(&cwd, capability, name),

        Cmd::Memory { cmd } => match cmd {
            None => memory_ls(&cwd),
            Some(MemoryCmd::Lint) => memory_lint(&cwd),
            Some(MemoryCmd::Stale) => memory_stale(&cwd),
            Some(MemoryCmd::Promote { keys }) => memory_promote(&cwd, keys),
            Some(MemoryCmd::Serve {
                team,
                local,
                session,
            }) => memory_serve(team.clone(), local.clone(), session.clone()),
            Some(MemoryCmd::Rm { key, layer, at }) => memory_rm(&cwd, key, *layer, at.as_deref()),
            Some(MemoryCmd::Remember {
                expected,
                observed,
                evidence,
                answers,
                relates_to,
                invalidated_by,
                source,
                if_exists,
            }) => memory_remember(
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
                cli.session.as_deref(),
            ),
        },

        // Before `run` looks anything up: which flags are whose is a question
        // about the command line, and answering it after resolving an adapter
        // would report an unknown harness for a mistyped flag.
        Cmd::Run(argv) => run(&cwd, &passthrough(argv, &omh_globals())?, &cli),
    }
}

/// What to tell someone whose word matched nothing. Pure so it can be tested:
/// the message is the entire value of this path.
fn tool_hint(name: &str, harnesses: &[String], editors: &[String]) -> String {
    if editors.iter().any(|e| e == name) {
        return format!("`{name}` is an editor — try `omh attach {name}`");
    }
    if RESERVED.contains(&name) {
        return format!("`{name}` is a command — see `omh {name} --help`");
    }
    format!(
        "unknown harness `{name}`\n  available: {}",
        harnesses.join(", ")
    )
}

/// Neither a harness nor a reserved word — say what is available, since the
/// user cannot tell from the name alone which kind they meant.
fn unknown_tool(paths: &Paths, name: &str, original: anyhow::Error) -> anyhow::Error {
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

/// The docker half of `container::reuse`: gather the three facts it decides on.
///
/// One exec, not two. Whether the container can be entered and what is running
/// inside it are the same question asked of the same command — and a container
/// that refuses the exec cannot answer the second, which is why an unreadable
/// probe short-circuits to "replace it" rather than to "nothing is running".
fn reuse_decision(
    backend: &dyn runtime::Runtime,
    name: &str,
    plan: &container::Plan,
    session: &Session,
) -> container::Reuse {
    // `|| true` so an absent socket directory is an empty listing rather than a
    // failed exec — the failure this reads is the mount namespace one, and
    // conflating the two would replace every container that has never run a
    // harness.
    let probe = backend.exec_args(
        name,
        &[
            "sh".into(),
            "-c".into(),
            format!("ls -1 {} 2>/dev/null || true", persist::SOCKET_DIR),
        ],
        false,
    );
    let Some(listing) = image::container_probe(backend.program(), &probe) else {
        return container::reuse(false, &Default::default(), plan, &[]);
    };
    container::reuse(
        true,
        &image::container_stamp(backend.program(), name),
        plan,
        &persist::live(&session.id, &listing),
    )
}

/// Bring a session's sandbox up if it is not already. A session is a *running
/// container*, not a launch — that is what lets an editor attach to the same
/// place the agent is working.
fn session_up(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    opts: container::Options,
    // The recipe behind `opts.image`. Handed in beside it rather than derived
    // here, so the tag a session runs and the layer that gets built come from
    // one `sandbox()` call and cannot describe different images — the split
    // that let `init` build a layer no launch ever ran.
    recipe: &[&str],
) -> Result<(Box<dyn runtime::Runtime>, String)> {
    let backend = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))?;
    let name = paths.container(&session.id);
    let running = image::container_running(backend.program(), &name);

    // Before planning, because the plan mounts the memory server only if a
    // binary exists. Degraded rather than fatal: a session without memory is
    // still a session, and refusing to launch over it would be the tail
    // wagging the dog — the same rule as a capability a harness cannot express.
    //
    // `ensure` is also what *resolves* the path, rather than the caller's
    // earlier `available()`. That ordering is the whole point and it is easy to
    // lose: on a first launch `available()` answers `None` because the binary
    // has not been cross-built yet, `ensure` then builds it, and a plan holding
    // the earlier answer mounts nothing — after printing that it was building
    // the very thing it goes on to ignore. Owning the field here means no
    // caller can sample it too early.
    let mut opts = opts;
    match memory::deliver::ensure(
        backend.program(),
        paths,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    ) {
        Ok(bin) => opts.memory_bin = Some(bin),
        Err(e) => {
            eprintln!("omh: memory server unavailable — {e:#}");
            opts.memory_bin = None;
        }
    }

    // The account must reach *this* plan: this is the container that actually
    // runs. Building it without credentials is how every session started
    // logged out while `--dry-run` advertised the mounts.
    say_selection(profile, &opts.repo);
    let plan = container::plan(paths, profile, adapter, session, &[], opts)?;
    plan.validate(&backend.caps())?;

    // The plan is built before this rather than after, because the plan *is*
    // the question: a running container is only this session if it was made
    // from the same one. Cheap — `ensure` above is a path check once the binary
    // is cached, and the staging the plan performs happens every launch anyway.
    if running {
        match reuse_decision(backend.as_ref(), &name, &plan, session) {
            container::Reuse::Attach => return Ok((backend, name)),
            container::Reuse::Blocked { live, changed } => anyhow::bail!(
                "session {id} is running {} and cannot be reused for this launch \
                 ({})\n  stop it with        omh s down {id}\n  \
                 or start a fresh one  omh --new {}",
                live.join(", "),
                changed.join(", "),
                adapter.name,
                id = session.id,
            ),
            container::Reuse::Restart(why) => {
                eprintln!(
                    "omh: restarting the sandbox for {} — {}",
                    session.label(),
                    why.join(", ")
                );
                let _ = image::container_remove(backend.program(), &name);
            }
        }
    }

    say_rules(&plan);
    image::ensure_stack(backend.program(), adapter, recipe)?;
    image::ensure_network(backend.program(), &plan.network)?;

    let key = ssh::ensure_key(&paths.keys())?;
    let pubkey = std::fs::read_to_string(key.with_extension("pub"))?;
    let port = ssh::port(&paths.repo_name(), &session.id);

    let _ = image::container_remove(backend.program(), &name); // a stopped one blocks --name
    let args = backend.up_args(&plan, &name, port, pubkey.trim());
    let out = Command::new(backend.program()).args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "starting session {}: {}",
            session.id,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // The session's worktree is not the checkout indexed at init — it holds
    // whatever the agent has since written. Index it now; the Stop hook keeps
    // it current from here.
    let project = base::project_name(&paths.repo_name(), &session.id);
    let _ = Command::new(backend.program())
        .args(backend.exec_args(
            &name,
            &[
                base::GRAPH_BIN.into(),
                "cli".into(),
                "index_repository".into(),
                "--repo-path".into(),
                container_workdir().into(),
                "--name".into(),
                project,
                "--mode".into(),
                "fast".into(),
            ],
            false,
        ))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    Ok((backend, name))
}

fn attach(cwd: &std::path::Path, id: Option<&str>, chosen: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
        .context("no adapters installed — run `omh init`")?;
    let adapter = Adapter::find(&paths.adapters(), &harness)?;
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        sandbox.top_up(
            &paths,
            backend.program(),
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
        )?;
    }

    std::fs::create_dir_all(paths.worktrees())?;
    let id = session::pick(&paths.worktrees(), id, false);
    let session = Session::new(&paths.worktrees(), id);
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;
    carry_in(&paths, &session)?;
    let _ = idle::touch(&paths.runs(), &session.id);

    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, None, configured.as_deref())?
        .map(|a| auth::dir(&paths, &adapter.name, &a));
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }
    session_up(
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: false,
            account_dir: account,
            memory_bin: memory::deliver::available(&paths),
            base: Some(session::default_branch(&paths.repo)),
            omh: own,
            repo,
            image: sandbox.tag.clone(),
            resolves: sandbox.resolves.clone(),
        },
        &sandbox.recipe(),
    )?;

    // The integration point is a managed SSH config include, not an IDE plugin —
    // that is what keeps every editor working without omh knowing about any.
    let home = dirs::home_dir().context("no home directory")?;
    let alias = ssh::host_alias(&paths.repo_name(), &session.id);
    let key = ssh::ensure_key(&paths.keys())?;
    let blocks: Vec<String> = session::list(&paths.worktrees())
        .into_iter()
        .map(|s| {
            ssh::config_block(
                &ssh::host_alias(&paths.repo_name(), &s),
                ssh::port(&paths.repo_name(), &s),
                &key,
            )
        })
        .collect();
    ssh::write_hosts(&home.join(".ssh/config.d/omh"), &blocks)?;
    ssh::ensure_include(&home.join(".ssh/config"))?;

    let fallback = std::env::var("OMH_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .and_then(|e| {
            let base = std::path::Path::new(&e)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some(base)
        });
    let wanted = chosen.map(str::to_string).or(fallback);
    let ed = wanted
        .as_deref()
        .and_then(|n| editor::Editor::find(&paths.editors(), n));

    match ed {
        // An editor that is not installed is not an error — the URL is still a
        // good answer, and launching nothing silently would not be.
        Some(ed) if runtime::installed(&ed.bin) => {
            let cmd = ed.command(&alias);
            println!("omh: opening {} in {}", ssh::url(&alias), ed.name);
            let ok = Command::new(&cmd[0])
                .args(&cmd[1..])
                .status()
                .map(|s| s.success());
            if !matches!(ok, Ok(true)) {
                // Remote launches fail for ordinary reasons — missing extension,
                // handshake refused. Printing nothing leaves the user waiting
                // for a window that will never open.
                eprintln!("omh: {} did not open the session", ed.name);
                println!("\n  {}", ssh::url(&alias));
                println!("  ssh {alias}");
            }
        }
        other => {
            if let Some(ed) = other {
                println!("omh: `{}` is not installed on this machine\n", ed.bin);
            } else if let Some(w) = &wanted {
                println!("omh: no editor named `{w}` — see `omh ls`\n");
            }
            println!("session {} is up\n", session.id);
            println!("  {}", ssh::url(&alias));
            println!("  ssh {alias}\n");
            for e in editor::Editor::load_dir(&paths.editors())? {
                println!("  {:<8} {}", e.name, e.command(&alias).join(" "));
            }
        }
    }
    Ok(())
}

/// Serve the graph UI from the session and open it.
///
/// Started on demand rather than always: the port is reserved when the session
/// is created (it has to be), but a process nobody looks at is waste.
/// The graph UI, once per repo.
///
/// Not per session: every session's graph lives in one volume, so a per-session
/// server showed every other session's graph anyway. Matching the server's
/// scope to its data's scope removes the duplication, survives sessions coming
/// and going, and lets the container mount only the index.
fn graph(cwd: &std::path::Path, _id: Option<&str>, stop: bool) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    let container = base::ui_container(&paths.repo_name());

    if stop {
        if !image::container_running(backend.program(), &container) {
            println!("the graph is not running");
            return Ok(());
        }
        image::container_remove(backend.program(), &container)?;
        println!("omh: graph stopped; sessions keep running");
        return Ok(());
    }

    let port = base::ui_port(&container);
    if !image::container_running(backend.program(), &container) {
        // A stopped container of the same name blocks `run --name`.
        let _ = image::container_remove(backend.program(), &container);

        let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
            .into_iter()
            .map(|a| a.name)
            .collect();
        let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
            .context("no adapters installed — run `omh init`")?;
        let adapter = Adapter::find(&paths.adapters(), &harness)?;
        image::ensure(backend.program(), &adapter)?;

        let out = Command::new(backend.program())
            .args(base::ui_run_args(
                &image::tag_for(&adapter),
                &container,
                &paths.cache_volume(),
                port,
            ))
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "could not start the graph: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }

    let url = format!("http://127.0.0.1:{port}");
    println!("omh: graph at {url}");
    println!("  every session's graph for this repo, in one place");
    println!("  stop with: omh graph --stop");
    let _ = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(&url)
    .status();
    Ok(())
}

/// Stop sessions nobody has used for longer than `policy.idle_timeout`.
///
/// N sessions is N containers — the sprawl `docs/design/risks.md` names. Only
/// the container stops; the worktree and branch survive, so relaunching resumes
/// exactly where you left off.
///
/// Best-effort by design: this runs on the way to starting a session, and a
/// failure to reap must never stop you working.
fn reap_idle(paths: &Paths, launching: &str) {
    let Some(raw) = policy_value(paths, "idle_timeout") else {
        return;
    };
    let Some(timeout) = idle::parse_duration(&raw) else {
        // Say so rather than ignoring silently — a setting that resolves with
        // provenance and then does nothing is exactly what this feature was.
        eprintln!("omh: ignoring idle_timeout `{raw}` — expected a duration like 30m, 2h, 90s");
        return;
    };
    let Ok(backend) = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))
    else {
        return;
    };

    let running: Vec<(String, Option<std::time::SystemTime>)> = session::list(&paths.worktrees())
        .into_iter()
        .filter(|id| image::container_running(backend.program(), &paths.container(id)))
        .map(|id| {
            let last = idle::last_used(&paths.runs(), &id);
            (id, last)
        })
        .collect();

    for id in idle::expired(&running, timeout, std::time::SystemTime::now(), launching) {
        match image::container_remove(backend.program(), &paths.container(&id)) {
            Ok(()) => {
                eprintln!("omh: stopped {id} — idle over {raw} (worktree and branch survive)")
            }
            Err(e) => eprintln!("omh: could not stop idle session {id}: {e}"),
        }
    }
}

fn down(cwd: &std::path::Path, id: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    let ids = match id {
        Some(i) => vec![i.to_string()],
        None => session::list(&paths.worktrees()),
    };
    for i in &ids {
        let name = paths.container(i);
        if image::container_running(backend.program(), &name) {
            match image::container_remove(backend.program(), &name) {
                Ok(()) => println!("stopped {i}; worktree and branch survive"),
                Err(e) => eprintln!("omh: {i} is still running: {e}"),
            }
        } else {
            println!("{i} was not running");
        }
    }
    Ok(())
}

/// Launch the real image with the real mounts and ask the harness's own paths
/// what they can see. Nothing in process can answer this: a green unit suite
/// proves omh mounts a path, never that anything reads it.
fn doctor_cmd(cwd: &std::path::Path, harness: Option<&str>, dry_run: bool) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let name = match harness {
        Some(h) => h.to_string(),
        None => {
            let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
                .into_iter()
                .map(|a| a.name)
                .collect();
            detect::preferred_harness(&names, &|h| runtime::installed(h))
                .context("no adapters installed — run `omh init`")?
        }
    };
    let adapter = Adapter::find(&paths.adapters(), &name)?;

    // Credentials are the half no in-process test can reach: whether a token
    // saved here survives depends on how the runtime binds the path.
    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, None, configured.as_deref())
        .unwrap_or(None)
        .map(|a| auth::dir(&paths, &name, &a));

    // Resolved once and used for both the checks and the plan below, so the
    // probe cannot check a session different from the one it launches.
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        sandbox.top_up(
            &paths,
            backend.program(),
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
        )?;
    }
    let mut checks = doctor::checks(&profile, &adapter, &own, &repo, &sandbox.resolves)?;
    if account.is_some() {
        checks.extend(doctor::credential_checks(&adapter));
    }
    // Only if the resolved profile actually declares it: a check for a server
    // nobody configured would fail honestly and mean nothing.
    //
    // Read through `render::parse_layers` rather than `config::servers`, which
    // returns only each server's *command* — the arguments are what say which
    // directories it will look in, and those are the whole point of the check.
    let declared = render::parse_layers(&profile.sources(adapter::Capability::Mcp)?)?;
    // Not when this repo has switched the feature off: the server is left out
    // of the document on purpose, so checking for it is checking a claim omh
    // deliberately did not make.
    if let Some(server) = declared
        .get(memory::tools::SERVER_KEY)
        .filter(|_| !repo.disabled_servers.contains(memory::tools::SERVER_KEY))
    {
        checks.extend(doctor::memory_checks(server));
    }
    if checks.is_empty() {
        println!("nothing to check: the profile is empty");
        return Ok(());
    }

    let session = Session::scratch(paths.scratch("doctor"), "doctor".into());
    session.ensure(&paths.repo, "")?;

    let opts = container::Options {
        staging: container::Staging::Apply,
        // No dtach and no terminal: the probe's output has to be captured.
        persist: persist::Mode::None,
        tty: false,
        account_dir: account.clone(),
        memory_bin: memory::deliver::available(&paths),
        // The probe has to compose the same rules a launch would, or it proves
        // the harness reads a document nobody will be given.
        base: Some(session::default_branch(&paths.repo)),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }
    say_selection(&profile, &opts.repo);
    let mut plan = container::plan(&paths, &profile, &adapter, &session, &[], opts)?;
    say_rules(&plan);
    plan.argv = vec!["sh".into(), "-c".into(), doctor::probe_script(&checks)];

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    if dry_run {
        println!("{}", doctor::probe_script(&checks));
        return Ok(());
    }

    image::ensure_stack(backend.program(), &adapter, &sandbox.recipe())?;
    image::ensure_network(backend.program(), &plan.network)?;

    match &account {
        Some(a) => println!(
            "omh doctor: {name} (in {}, account {})\n",
            sandbox.tag,
            a.file_name().unwrap_or_default().to_string_lossy()
        ),
        None => println!(
            "omh doctor: {name} (in {}, no account — credentials unchecked)\n",
            sandbox.tag
        ),
    }
    let out = Command::new(backend.program())
        .args(backend.args(&plan))
        .output()?;
    let outcomes = doctor::parse(&String::from_utf8_lossy(&out.stdout));
    let _ = session.remove(&paths.repo, ""); // diagnostic: leave no session behind

    for o in &outcomes {
        println!(
            "  {} {:<10} {}",
            if o.ok { "\u{2713}" } else { "\u{2717}" },
            o.name,
            o.detail
        );
    }

    if outcomes.is_empty() {
        anyhow::bail!(
            "the probe produced no output — the sandbox did not run it\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if !doctor::passed(&outcomes) {
        anyhow::bail!(
            "{} of {} checks failed",
            outcomes.iter().filter(|o| !o.ok).count(),
            outcomes.len()
        );
    }
    println!(
        "\n  all {} checks passed — {name}'s adapter paths are verified",
        outcomes.len()
    );
    Ok(())
}

fn sessions_ls(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)).ok();
    let base = session::default_branch(&paths.repo);
    let sessions = session::list(&paths.worktrees());
    if sessions.is_empty() {
        println!("no sessions");
    }
    for id in sessions {
        let sess = Session::new(&paths.worktrees(), id.clone());
        let up = backend
            .as_ref()
            .map(|b| image::container_running(b.program(), &paths.container(&id)))
            .unwrap_or(false);
        let drift = match sess.behind(&paths.repo, &base) {
            0 => String::new(),
            n => format!("  ({n} behind {base})"),
        };
        println!(
            "  {id:<8} {:<14} {:<9} {:<20}{drift}",
            sess.label(),
            if up { "up" } else { "stopped" },
            work_state(&sess, &paths.repo, &base),
        );
    }

    let left = leftovers(&paths, backend.as_deref());
    if !left.is_empty() {
        println!(
            "\n{} removed but left something behind: {}",
            if left.len() == 1 {
                "1 session was"
            } else {
                "sessions were"
            },
            left.join(", ")
        );
        println!("  clear each with  omh s rm <id>");
    }
    Ok(())
}

/// Session ids with a container or a run directory but no worktree.
///
/// Invisible until now, and not merely untidy: an orphan container holds a
/// session id, and the next session to take that id used to exec straight into
/// it. That was the mount-namespace failure. `s rm` cleans up after itself now,
/// so this reports what older versions left — and anything a hand
/// `git worktree remove` strands from here on.
///
/// A run directory counts only when it carries the marker `idle::touch` writes.
/// `omh doctor` and `omh auth` stage into the same tree under their own names,
/// and neither is a session anybody could resume or would want reported.
fn leftovers(paths: &Paths, backend: Option<&dyn runtime::Runtime>) -> Vec<String> {
    let live = session::list(&paths.worktrees());
    let mut found: Vec<String> = std::fs::read_dir(paths.runs())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|id| idle::last_used(&paths.runs(), id).is_some())
        .collect();

    if let Some(backend) = backend {
        let prefix = paths.container("");
        if let Ok(out) = Command::new(backend.program())
            .args(["ps", "-a", "--format", "{{.Names}}"])
            .output()
        {
            found.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|n| n.trim().strip_prefix(&prefix))
                    .map(str::to_string),
            );
        }
    }

    found.retain(|id| !live.contains(id));
    found.sort();
    found.dedup();
    found
}

/// Where a session is in the cycle, phrased as the next thing to do about it.
///
/// Ordered most-actionable first, and deliberately one answer rather than a
/// tally: `s ls` is read at a glance, and a session with uncommitted work needs
/// committing whatever else is also true of it.
fn work_state(session: &Session, repo: &std::path::Path, base: &str) -> String {
    // A git that cannot answer is never rendered as an answer. Every accessor
    // below runs through the worktree's `.git` pointer, which goes stale when a
    // checkout moves and is already handled as a real case by `Session::remove`
    // — and a blank column reads as "nothing here" for a session that may be
    // holding a day of work the user is about to `s rm`.
    let (uncommitted, unpushed) = match (session.uncommitted(), session.unpushed()) {
        (Ok(uncommitted), Ok(unpushed)) => (uncommitted, unpushed),
        _ => return "?".into(),
    };

    if let n @ 1.. = uncommitted {
        return format!("{n} uncommitted");
    }
    match unpushed {
        Some(n @ 1..) => format!("{n} to push"),
        // Nothing origin does not already have. Report the name it went out
        // under, which is what you would look for in a list of PRs — `omh/s01`
        // is not a name anybody searches for.
        Some(_) => match session.published_as() {
            Ok(Some(target)) => format!("→ {target}"),
            Ok(None) => String::new(),
            Err(_) => "?".into(),
        },
        // Never pushed, which is not the same as nothing to push: this is the
        // state the loop passes through every time, between `s commit` and the
        // first `s push`. Measured against the base branch instead, because a
        // blank here reads as a session nobody touched.
        None => match session.commits(repo, base) {
            0 => String::new(),
            n => format!("{n} to push"),
        },
    }
}

/// Read one policy key through the usual layer merge.
fn policy_value(paths: &Paths, key: &str) -> Option<String> {
    config::policy(paths)
        .ok()?
        .into_iter()
        .find(|s| s.key == key)
        .map(|s| s.value)
}

fn runtime_preference(paths: &Paths) -> String {
    policy_value(paths, "runtime").unwrap_or_else(|| "auto".into())
}

fn parse_layer(s: &str) -> std::result::Result<config::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
}

/// A note's layer, which is a different set from a profile's: notes have no
/// personal layer, and the two they do have never merge.
fn parse_note_layer(s: &str) -> std::result::Result<memory::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
}

/// The agent's working directory inside the sandbox. Named once, so the note
/// store and the launch plan cannot disagree about it.
pub fn container_workdir() -> &'static str {
    "/work"
}

fn parse_if_exists(s: &str) -> std::result::Result<memory::IfExists, String> {
    match s {
        "error" => Ok(memory::IfExists::Error),
        "skip" => Ok(memory::IfExists::Skip),
        "suffix" => Ok(memory::IfExists::Suffix),
        "override" => Ok(memory::IfExists::Override),
        other => Err(format!(
            "unknown --if-exists `{other}` (error, skip, suffix, override)"
        )),
    }
}

/// Record an observation. The key is derived, never chosen: an agent that picks
/// its own cannot be stopped from recording one event twice.
fn memory_remember(
    cwd: &std::path::Path,
    mut input: memory::Remembered,
    if_exists: memory::IfExists,
    session: Option<&str>,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if input.source.trim().is_empty() {
        // Provenance is omh's to supply, so that it cannot be omitted. On the
        // CLI there may be no session, and saying `cli` is honest where
        // inventing a session id would not be.
        input.source = match session {
            Some(id) => format!("session {id}, cli"),
            None => "cli".into(),
        };
    }
    match memory::remember(&paths, &input, if_exists)? {
        memory::Wrote::Created(path) => println!("recorded {}", path.display()),
        // Said out loud: a note that existed is gone, and only `--if-exists
        // override` gets here, so the caller asked for it and can check.
        memory::Wrote::Replaced(path) => {
            println!(
                "replaced {} — the note that was there is gone",
                path.display()
            )
        }
        memory::Wrote::Skipped(key) => println!("`{key}` is already recorded; left alone"),
    }
    Ok(())
}

/// Write one note per tracked document, plus one for what `init` derived.
///
/// Into the **committed** layer: a stub is reproducible from a document every
/// teammate already has, so it is not a claim from experience and does not need
/// a human to vouch for it. `promote` stays reserved for what an agent
/// observed.
fn seed_store(paths: &Paths) -> Result<String> {
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

    let seeds = detect::seeds(&stack::load_dir(&paths.stacks())?, &paths.repo);
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

/// Speak MCP until stdin closes.
///
/// Nothing but protocol may reach stdout — this binary is full of `println!`,
/// and one stray line breaks the very first handshake. Diagnostics go to
/// stderr, which the harness shows as server logs.
fn memory_serve(
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
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    mcp::serve(stdin, stdout, &mut server)
}

/// A join against facts omh already holds. Three groups, because they are
/// three different claims: the world moved, omh cannot tell, and omh was never
/// asked to tell.
fn memory_stale(cwd: &std::path::Path) -> Result<()> {
    use memory::expiry::Verdict;
    let paths = Paths::discover(cwd)?;
    let judged = memory::expiry::judge(&paths, &memory::load(&paths)?)?;

    /// Which heading a verdict belongs under, or `None` for the one that is
    /// counted rather than listed.
    ///
    /// A `match` on the verdict alone, so the compiler owns the mapping. The
    /// grouping used to match on `(&verdict, <integer tag>)`, where a `_` arm
    /// is unavoidable — a verdict added later fell through it, was counted in
    /// no group and tallied nowhere, so the note simply vanished from the
    /// command. `evaluate` refusing to collapse the third answer buys nothing
    /// if the printer drops the fourth.
    fn heading(verdict: &Verdict) -> Option<&'static str> {
        match verdict {
            Verdict::Stale { .. } => Some("stale"),
            Verdict::Unknown { .. } => Some("omh cannot tell"),
            Verdict::NoTrigger => Some("no expiry — carries only its date"),
            Verdict::Fresh => None,
        }
    }

    let mut printed = false;
    for group in [
        "stale",
        "omh cannot tell",
        "no expiry — carries only its date",
    ] {
        let members: Vec<&memory::expiry::Judged> = judged
            .iter()
            .filter(|j| heading(&j.verdict) == Some(group))
            .collect();
        if members.is_empty() {
            continue;
        }
        if printed {
            println!();
        }
        printed = true;
        println!("{group}:");
        for j in members {
            // Every line carries its date and its layer, exactly as `recall`
            // does: a note reported without those cannot be judged.
            print!("  {:<44} {:<5}  {}", j.key, j.layer.to_string(), j.recorded);
            match &j.verdict {
                Verdict::Stale { because } | Verdict::Unknown { because } => {
                    println!("  — {because}")
                }
                Verdict::NoTrigger | Verdict::Fresh => println!(),
            }
        }
    }

    let count = |f: fn(&Verdict) -> bool| judged.iter().filter(|j| f(&j.verdict)).count();
    let fresh = count(|v| matches!(v, Verdict::Fresh));
    let stale = count(|v| matches!(v, Verdict::Stale { .. }));
    let unknown = count(|v| matches!(v, Verdict::Unknown { .. }));

    if !printed && fresh == 0 {
        println!("no notes yet");
    } else if fresh > 0 {
        println!("\n{fresh} still current");
    }

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
fn memory_promote(cwd: &std::path::Path, keys: &[String]) -> Result<()> {
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
                eprintln!("omh: {}", b.say());
            }
            anyhow::bail!("promoted nothing");
        }
    };
    memory::promote::apply(&steps)?;
    print!("{}", memory::promote::report(&steps, &paths));
    Ok(())
}

/// The store, by layer, with what points at each note.
fn memory_ls(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let notes = memory::load(&paths)?;
    if notes.is_empty() {
        println!("no notes yet — the store fills as work surprises the agent");
        return Ok(());
    }
    print!("{}", memory::render_list(&notes));
    Ok(())
}

/// The store-quality meter. Violations are grouped by rule rather than listed
/// flat, because the count per rule is the signal and the individual lines are
/// how you act on it.
fn memory_lint(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let found = memory::lint(&paths)?;
    if found.is_empty() {
        println!("no violations");
        return Ok(());
    }
    for v in &found {
        let mark = match v.rule.severity() {
            memory::Severity::Refused => "refused",
            memory::Severity::Warning => "warning",
        };
        println!("{mark:<8} {:<6} {}", v.layer.to_string(), v.detail);
    }
    println!();
    for (rule, count) in memory::tally(&found) {
        println!("  {count:>3}  {rule:?}");
    }

    // The report is the product, so it prints in full before this decides the
    // exit code. Warnings do not fail the command: `Orphan` fires on every
    // note nothing links to, and a gate that is always red gates nothing.
    let refused = memory::refused(&found);
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
fn memory_rm(
    cwd: &std::path::Path,
    key: &str,
    layer: Option<memory::Layer>,
    at: Option<&str>,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let removed = memory::remove(&paths, layer, key, at)?;
    println!("removed {key} ({})", removed.layer);
    if removed.layer.is_committed() {
        // The file is gone here, but a teammate still has it until the
        // deletion is committed. Saying so beats letting someone believe a
        // shared note disappeared for everybody.
        println!("  it was committed — teammates keep it until you commit the deletion");
    }
    if !removed.inbound.is_empty() {
        println!(
            "  still linked from {} — those links now dangle, and `omh memory lint` lists them",
            removed.inbound.join(", ")
        );
    }
    Ok(())
}

fn parse_env(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))
}

fn mcp(cwd: &std::path::Path, cmd: &McpCmd, dry_run: bool) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    match cmd {
        McpCmd::Ls => show_servers(cwd),

        McpCmd::Add {
            name,
            command,
            args,
            env,
        } => {
            let server = render::Server {
                command: command.clone(),
                args: args.clone(),
                env: env.iter().cloned().collect(),
            };
            let w = config::mcp_add(&paths, name, server)?;
            println!("wrote → {}", w.path.display());
            if !env.is_empty() {
                // The catalogue is not committed, so nothing here reaches a
                // teammate — but it does reach every repo you work in, which is
                // the wrong scope for a token scoped to one of them.
                println!(
                    "note: this env applies in every repo. For one repo only, \
                     put [mcp.{name}.env] in .omh/{}",
                    settings::LOCAL
                );
            }
            Ok(())
        }

        McpCmd::Rm { name } => {
            if config::mcp_remove(&paths, name)? {
                println!("removed {name} from your catalogue");
            } else {
                println!("{name} is not in your catalogue");
            }
            Ok(())
        }

        McpCmd::Import {
            harness,
            file,
            force,
        } => {
            let adapter = Adapter::find(&paths.adapters(), harness)?;
            let binding = adapter
                .supports(adapter::Capability::Mcp)
                .with_context(|| format!("{harness} has no MCP capability to import from"))?;

            let home = dirs::home_dir().context("no home directory")?;
            let source = match file {
                Some(f) => f.clone(),
                None => {
                    let template = binding.import.as_deref().with_context(|| {
                        format!("adapter {harness} does not say where to import from; pass --file")
                    })?;
                    adapter::expand_host(template, &home, &paths.repo)
                }
            };

            let raw = std::fs::read_to_string(&source).with_context(|| {
                format!(
                    "reading {} — pass --file to point somewhere else",
                    source.display()
                )
            })?;
            let incoming = render::parse(binding.render, &raw)?;

            let report = config::mcp_import(&paths, incoming, *force, dry_run)?;

            println!("import from {harness} ({})", source.display());
            for name in &report.added {
                println!("  + {name}");
            }
            for name in &report.unchanged {
                println!("  = {name} (already identical)");
            }
            for name in &report.conflicts {
                println!("  ! {name} (differs — keeping yours; --force to overwrite)");
            }
            if report.added.is_empty() && report.conflicts.is_empty() && report.unchanged.is_empty()
            {
                println!("  (no servers found)");
            }
            if dry_run {
                println!("\n--dry-run: nothing written");
            } else if !report.added.is_empty() {
                println!("\nwrote → {}", config::mcp_path(&paths).display());
            }
            Ok(())
        }
    }
}

/// Does this repo already say what it uses?
///
/// Read from the committed file directly rather than through `settings::resolve`,
/// which merges three layers: a `[use]` in *your* personal file is your default
/// everywhere and is not this repo having decided anything, so treating it as
/// one would leave a fresh checkout with no list of its own.
fn repo_has_selection(paths: &Paths) -> Result<bool> {
    // Through `config`, which distinguishes absent from unreadable. Reading the
    // file here with `let Ok(..) else { return Ok(false) }` reintroduced the
    // exact conflation `config::read_layer` was written about, in the one place
    // where the answer decides whether `init` overwrites a curated list — and
    // it was a third parse strategy for a file that already had two.
    config::declares(paths, config::Layer::Shared, config::USE)
}

/// The catalogue's MCP servers, with whose each one is.
fn show_servers(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let servers = config::servers(&paths)?;
    println!("mcp:");
    if servers.is_empty() {
        println!("  (nothing set)");
    }
    for s in servers {
        // Content says whose it is; a setting says which file decided it.
        println!("  {:<16} {:<28} ← {}", s.key, s.value, s.layer.whose());
    }
    Ok(())
}

/// Select a catalogue entry for this repo, or resync the whole list.
///
/// Writes the **committed** file. What a project uses is a fact about the
/// project, and a teammate cloning it should get the same selection — the
/// opposite default from `omh repo set`, which holds secrets.
///
/// A capability with no list is following the whole catalogue, so adding one
/// name to it has to write the catalogue out first. Writing `["tdd"]` alone
/// would silently turn off everything else, which is the one thing a command
/// called `use` must never do.
fn use_cmd(
    cwd: &std::path::Path,
    capability: Option<&str>,
    name: Option<&str>,
    all: bool,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if all {
        if capability.is_some() {
            anyhow::bail!("`--all` resyncs every capability — it takes no arguments");
        }
        let lists = catalogue_lists(&paths)?;
        for w in write_lists(&paths, &lists)? {
            println!("resynced to your catalogue — wrote → {}", w.path.display());
        }
        for (cap, names) in &lists {
            println!("  {:<11} {}", cap.to_string(), names.len());
        }
        return Ok(());
    }

    let (Some(key), Some(name)) = (capability, name) else {
        anyhow::bail!(
            "omh use <capability> <name>, or omh use --all\n  capabilities: {}",
            capability_list()
        );
    };
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    // A name nothing answers to is a typo far more often than a plan, and the
    // launcher would only report it later. `omh config edit` is how you create
    // the entry first.
    let available = catalogue_names(&paths, cap)?;
    if !available.iter().any(|n| n == name) {
        anyhow::bail!(
            "your catalogue has no {cap} called `{name}`. `omh config edit {cap} {name}` \
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
        println!("{cap}/{name} is already used here");
        return Ok(());
    }
    if !already {
        names.push(name.to_string());
    }
    let written = write_lists(
        &paths,
        &std::collections::BTreeMap::from([(cap, names.clone())]),
    )?;
    if was_open {
        // Said out loud, because this is the moment a capability turns from
        // "follows the catalogue" into "this list" — everything is still
        // selected, but from now on by name, and an entry added later will not
        // be.
        println!(
            "{cap} was following your whole catalogue; wrote its {} entries as the list",
            names.len()
        );
    }
    for w in written {
        println!("using {cap}/{name} — wrote → {}", w.path.display());
    }
    Ok(())
}

/// Stop using a catalogue entry here.
fn unuse_cmd(cwd: &std::path::Path, key: &str, name: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    if !names.iter().any(|n| n == name) {
        // Refused rather than written as a no-op: a name this repo never used
        // is a typo, and writing the list back would report success for it.
        anyhow::bail!(
            "{cap}/{name} is not used here. `omh repo` lists what is.\n  \
             using: {}",
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            }
        );
    }
    names.retain(|n| n != name);
    if was_open {
        // The same disclosure `use_cmd` makes, and for the same reason: this is
        // the moment the capability stops following the catalogue. Discarding
        // the flag here was an oversight rather than a decision — `unuse`
        // performs the identical conversion, so a repo with no list at all
        // freezes into one on the command that was meant to remove one name.
        println!(
            "{cap} was following your whole catalogue; wrote its remaining {} entries as the list",
            names.len()
        );
    }
    for w in write_lists(&paths, &std::collections::BTreeMap::from([(cap, names)]))? {
        println!(
            "no longer using {cap}/{name} — wrote → {}",
            w.path.display()
        );
    }
    Ok(())
}

/// Write these lists to every repo layer that has a say in them.
///
/// One capability at a time, because which layers declare `skills` and which
/// declare `mcp` are different questions — `omh use --all` in a repo whose
/// gitignored file overrides exactly one capability must not acquire the other
/// five there.
fn write_lists(
    paths: &Paths,
    lists: &std::collections::BTreeMap<adapter::Capability, Vec<String>>,
) -> Result<Vec<config::Written>> {
    let mut out = Vec::new();
    for (cap, names) in lists {
        let one = std::collections::BTreeMap::from([(*cap, names.clone())]);
        for layer in config::declaring(paths, config::USE, &cap.to_string())? {
            out.push(config::write_selection(paths, layer, &one)?);
        }
    }
    // Two capabilities can share a layer, and reporting the same file twice
    // reads as two writes.
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// This capability's effective list, and whether it had one at all.
///
/// The name is validated here rather than at the write, which is the same rule
/// `[use]` follows: a name is checked where it is minted, so `omh use` cannot
/// put something in the file that reading the file would refuse.
fn current_list(
    paths: &Paths,
    key: &str,
    name: &str,
) -> Result<(adapter::Capability, Vec<String>, bool)> {
    let cap = adapter::Capability::from_key(key).with_context(|| {
        format!(
            "`{key}` is not a capability — expected {}",
            capability_list()
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
             your entries; a feature is all or nothing, so `omh repo enable {feature}` \
             and `omh repo disable {feature}` are its switches."
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
fn catalogue_lists(
    paths: &Paths,
) -> Result<std::collections::BTreeMap<adapter::Capability, Vec<String>>> {
    let mut out = std::collections::BTreeMap::new();
    for cap in adapter::Capability::ALL {
        out.insert(cap, catalogue_names(paths, cap)?);
    }
    Ok(out)
}

/// The names a `[use]` list may hold for `cap`: what the catalogue and this
/// repo declare, minus omh's own, which `[omh]` governs and `[use]` refuses.
fn catalogue_names(paths: &Paths, cap: adapter::Capability) -> Result<Vec<String>> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let owned = manifest.owns();
    Ok(Profile::resolve(paths)
        .entries(cap)?
        .into_iter()
        .filter(|n| !owned.get(&cap).is_some_and(|o| o.contains_key(n)))
        .collect())
}

/// Your defaults and your catalogue.
///
/// Deliberately not the resolved three-layer merge any more — that question is
/// "what is effective *here*", and it moved to `omh repo` with the rest of the
/// repo-scoped reporting. This command narrows to mean **you**.
fn show_config(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);

    println!(
        "your defaults  {}",
        config::Layer::Personal.file(&paths).display()
    );
    let yours: Vec<config::Setting> = config::policy(&paths)?
        .into_iter()
        .filter(|s| s.layer == config::Layer::Personal)
        .collect();
    if yours.is_empty() {
        println!("  (nothing set)");
    }
    for s in yours {
        println!("  {:<16} {}", s.key, s.value);
    }

    println!("\nyour catalogue  {}", paths.root.display());
    for cap in adapter::Capability::ALL {
        let entries = profile.entries(cap)?;
        // The count as well as the names: a catalogue is a thing that grows,
        // and "12" is the number the unselected report will be talking about.
        println!(
            "  {:<11} {:>2}  {}",
            cap.to_string(),
            entries.len(),
            entries.join(", ")
        );
    }
    Ok(())
}

/// What is effective in this checkout, and which file decided it.
///
/// Where the reporting this design keeps promising actually surfaces. With a
/// curated list the useful question stops being "what is this set to" and
/// becomes "why is this skill not here", and that needs the selection, the
/// features and the settings in one place.
fn show_repo(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let policy = settings::resolve(&paths, &manifest)?;

    println!("this repo  {}", paths.repo.join(".omh").display());

    println!("\nsettings");
    let settings = config::policy(&paths)?;
    if settings.is_empty() {
        println!("  (nothing set)");
    }
    for s in settings {
        // Provenance is the point: a three-layer merge you cannot trace is
        // worse than no layering at all.
        let shadowed = if s.shadows.is_empty() {
            String::new()
        } else {
            let names: Vec<_> = s.shadows.iter().map(|l| l.to_string()).collect();
            format!(" (overrides {})", names.join(", "))
        };
        println!("  {:<16} {:<24} ← {}{shadowed}", s.key, s.value, s.layer);
    }

    println!("\nomh's features");
    let mut features: Vec<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    features.sort();
    features.dedup();
    for feature in features {
        let state = if policy.off.contains(feature) {
            "off here"
        } else {
            "on"
        };
        println!("  {feature:<16} {state}");
    }

    println!("\nusing");
    for cap in adapter::Capability::ALL {
        let entries = profile.entries(cap)?;
        let unselected = policy.selection.unselected(cap, &entries);
        // "everything" rather than a list identical to the catalogue's, because
        // the two are different states: one follows the catalogue as it grows
        // and the other is a list that happens to be complete today.
        //
        // Printed in the **declared** order, not `entries`' alphabetical one.
        // For `rules` that order is the whole feature — this page's own docs say
        // "the list is the order" — and building the line from the sorted
        // catalogue made `omh repo` the one place that contradicted it. Filtered
        // by what the catalogue actually holds, so a name nothing answers to is
        // reported as missing rather than listed as used.
        let summary = match policy.selection.order(cap) {
            None => "everything".to_string(),
            Some(order) => {
                let taken: Vec<&str> = order
                    .iter()
                    .filter(|n| entries.iter().any(|e| e == *n))
                    .map(String::as_str)
                    .collect();
                if taken.is_empty() {
                    "nothing".to_string()
                } else {
                    taken.join(", ")
                }
            }
        };
        let note = if unselected.is_empty() {
            String::new()
        } else {
            format!(
                "   ({} not selected: {})",
                unselected.len(),
                unselected.join(", ")
            )
        };
        println!("  {:<11} {summary}{note}", cap.to_string());
    }

    for line in notice::selection(&profile, &policy.selection)? {
        println!("\n{line}");
    }
    Ok(())
}

/// `--layer` is going away. Accepted for one release, saying what replaced it.
///
/// The `keys.toml` treatment minus the refusal: this one is recoverable by
/// retyping, so a hard error would cost more than it protects. What it must not
/// do is keep working silently — a flag that outlives its documentation is how
/// people learn a command by copying a form that is about to stop existing.
fn layer_or(named: Option<config::Layer>, default: config::Layer) -> config::Layer {
    let Some(layer) = named else {
        return default;
    };
    let replacement = match layer {
        config::Layer::Personal => "omh config set",
        config::Layer::Shared => "omh repo set --shared",
        config::Layer::Local => "omh repo set",
    };
    eprintln!(
        "omh: --layer {layer} is going away — that is `{replacement}` now. \
         Two scopes, two commands: `omh config` is you, `omh repo` is this checkout."
    );
    layer
}

/// `omh repo set` writes the gitignored file; `--shared` writes the committed
/// one. The opposite default from `omh use`, deliberately: these carry
/// `carry_in` paths and MCP env, and a mistyped key must not be committable by
/// accident.
fn repo_layer(shared: bool) -> config::Layer {
    if shared {
        config::Layer::Shared
    } else {
        config::Layer::Local
    }
}

fn set(cwd: &std::path::Path, key: &str, value: &str, layer: config::Layer) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let w = config::set(&paths, key, value, layer)?;
    println!("wrote → {}", w.path.display());
    if w.committed {
        // The one mistake git makes unrecoverable.
        println!(
            "warning: the {} layer is COMMITTED — never put a secret here",
            w.layer
        );
    }
    Ok(())
}

fn unset(cwd: &std::path::Path, key: &str, layer: config::Layer) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if config::unset(&paths, key, layer)? {
        println!("removed {key} from the {layer} layer");
    } else {
        println!("{key} was not set in the {layer} layer");
    }
    Ok(())
}

/// `$EDITOR` on your settings, or on one catalogue entry.
///
/// Once `$EDITOR` is spawned it is a full program running as you, and any fence
/// omh drew around it would be decorative — there is no trust boundary between
/// omh and the person whose home directory this is. The boundary that matters
/// is structural and already there: every catalogue directory a sandbox is given
/// is mounted **read-only**.
///
/// This used to say `~/.omh` is not mounted at all, which is simply false —
/// `container.rs` binds each catalogue source at `/omh/layers/<n>/<cap>` — and
/// it is the kind of claim a reader takes on trust. Read-only is the true
/// version and carries the same argument.
///
/// What does need a guard is the **name**, the moment this takes one and joins
/// it to a directory: `omh config edit skills ../../../.ssh/id_rsa` is
/// traversal. Same rule and same function as `[use]` uses, because it is the
/// same act — a name being minted.
fn edit(
    cwd: &std::path::Path,
    capability: Option<&str>,
    name: Option<&str>,
    layer: config::Layer,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let file = match capability {
        None => layer.file(&paths),
        Some(key) => {
            let cap = adapter::Capability::from_key(key).with_context(|| {
                format!(
                    "`{key}` is not a capability — expected {}",
                    capability_list()
                )
            })?;
            let dir = paths.root.join(cap.source());
            match name {
                None => dir,
                Some(name) => {
                    selection::validate_entry_name(name, cap, &dir)?;
                    dir.join(name)
                }
            }
        }
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    Command::new(editor).arg(&file).status()?;
    Ok(())
}

fn capability_list() -> String {
    adapter::Capability::ALL
        .iter()
        .map(adapter::Capability::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Switch one of omh's features on or off in this checkout.
///
/// `enable`/`disable` rather than `use`/`unuse`, because the CLI should teach
/// the file's structure rather than flatten it: if `omh repo disable` took a
/// skill name, the difference between *an entry you chose* and *a feature omh
/// ships* would exist only in the docs.
fn feature_switch(cwd: &std::path::Path, feature: &str, on: bool) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let features: std::collections::BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    if !features.contains(feature) {
        // The entry-name case is the interesting error: it is how somebody
        // discovers the grouping without reading the manifest.
        if let Some(entry) = manifest.entry(feature) {
            anyhow::bail!(
                "`{feature}` is part of the `{}` feature, not a feature itself. \
                 A feature is all or nothing — `omh repo disable {}` switches all of it off.",
                entry.feature,
                entry.feature
            );
        }
        anyhow::bail!(
            "`{feature}` is not one of omh's features ({}). \
             A catalogue entry of yours is `omh use`/`omh unuse`.",
            features.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    // The committed file: which of omh's features a project runs with is a fact
    // about the project, the same argument `omh use` writes there on.
    // ...and the gitignored one too when it already declares this feature, or
    // the switch reports a change the layer beneath it overrules.
    for layer in config::declaring(&paths, config::OMH, feature)? {
        let w = config::write_feature(&paths, layer, feature, on)?;
        println!(
            "{feature} is {} here — wrote → {}",
            if on { "on" } else { "off" },
            w.path.display()
        );
    }
    if !on {
        println!("nothing was uninstalled; the next repo gets it back");
    }
    Ok(())
}

/// Say what composing the project's rules turned up, if anything.
///
/// Called from every path that builds a plan, not just `run`: `attach` and
/// `doctor` compose the same document, and a fallback announced on one path in
/// three is the same silence the notice exists to break. Only when there is
/// something to say — a line printed every launch is a line nobody reads.
fn say_rules(plan: &container::Plan) {
    for notice in plan.rules.notices() {
        eprintln!("omh: {notice}");
    }
}

/// What the launcher noticed about this repo's hooks: which ones it has, which
/// are new or changed, and where detection and the directory disagree.
///
/// Reported on every launch including a dry run — a dry run is exactly when you
/// want to be told what a launch would hand your agent. The returned `Record`
/// is the *other* half: committing it is what spends the "new or changed"
/// call-out, so only a session that actually started may do it.
///
/// Never fatal. A repo whose hook drift cannot be computed is still a repo you
/// can work in; and an unreadable hooks directory stops the launch anyway, in
/// `render::merge_hooks`, which is where it should.
fn say_hooks(paths: &Paths) -> Option<notice::Record> {
    // An unreadable stacks directory is the same class of non-fatal as the rest
    // of this function: it costs the drift report, not the session. Reported
    // and withdrawn, never defaulted to empty — `notice::hooks` reads "no
    // definitions" as "no stack answers to that name", so an empty list does
    // not weaken the report, it inverts it and prints the inversion in omh's
    // own voice.
    let defs = match stack::load_dir(&paths.stacks()) {
        Ok(defs) => defs,
        Err(e) => {
            eprintln!(
                "omh: could not read your stacks, so this repo's hooks went unchecked — {e:#}"
            );
            return None;
        }
    };
    match notice::hooks(paths, &defs, &detect::stacks(&defs, &paths.repo)) {
        Ok((notices, record)) => {
            for notice in notices {
                eprintln!("omh: {notice}");
            }
            Some(record)
        }
        Err(e) => {
            eprintln!("omh: could not check this repo's hooks — {e:#}");
            None
        }
    }
}

/// Say what this repo is not using from your catalogue, and what it named that
/// nothing answers to.
///
/// Called from **every path that builds a plan**, which is the rule `say_rules`
/// states and this broke on arrival: it was wired into `run` alone, so `attach`
/// and `doctor` composed the same profile and said nothing. `attach` is the path
/// where it matters most — it is how you rejoin a session that staged the
/// selection you have since changed.
///
/// Beside `say_hooks` and on the same terms otherwise: reported on every launch
/// including a dry run, never fatal. A selection omh cannot compute is not a
/// reason to refuse a session — and it cannot be one, because the report exists
/// to cover a silence rather than to guard anything.
fn say_selection(profile: &Profile, repo: &settings::RepoPolicy) {
    match notice::selection(profile, &repo.selection) {
        Ok(notices) => {
            for notice in notices {
                eprintln!("omh: {notice}");
            }
        }
        Err(e) => eprintln!("omh: could not check what this repo uses — {e:#}"),
    }
}

/// Mark this repo's hooks as seen, now that a session is actually running.
///
/// Deliberately after the container is up rather than beside the report. The
/// snapshot is what makes "new or changed" fire exactly once, so writing it
/// from a launch that then died — Docker not running, an image that would not
/// build — spent the one notification about somebody else's executable content
/// changing under you, and the retry was silent. A dry run never gets here at
/// all, which is the other half of the same rule.
fn remember_hooks(record: Option<notice::Record>) {
    if let Some(record) = record {
        if let Err(e) = record.commit() {
            // The check succeeded and its notices are already printed; only the
            // bookkeeping failed. Saying "could not check" would send the user
            // looking at their hooks instead of at `~/.omh/run`.
            eprintln!("omh: this repo's hooks were not recorded — {e:#}");
        }
    }
}

/// Copy the checkout's untracked essentials into a worktree, and say what
/// happened — a `.env` you thought you were carrying and are not is exactly the
/// failure that wastes an hour inside the sandbox.
fn carry_in(paths: &Paths, session: &Session) -> Result<()> {
    // The rules themselves are mounted, not written here — this covers the
    // empty placeholder each mount lands on, and any backend that cannot mount
    // a single file. It must run before `plan` places those placeholders.
    carry::hide_staged_rules(&session.worktree)?;

    let patterns = config::policy_list(paths, "carry_in");
    if patterns.is_empty() {
        return Ok(());
    }
    for item in carry::apply(&paths.repo, &session.worktree, &patterns)? {
        match item.action {
            carry::Action::Copied => eprintln!("omh: carried {}", item.path),
            carry::Action::Refreshed => eprintln!("omh: refreshed {}", item.path),
            // The mistake, named where it is made rather than three commands
            // later at `s commit`. `carry_in` is for what a worktree does not
            // get; a tracked file is already on the branch.
            carry::Action::AlreadyTracked => eprintln!(
                "omh: warning: carry_in lists {} — git already tracks it, so the worktree\n\
                 \x20 has it already. Not carried; drop it with `omh repo set carry_in`.",
                item.path
            ),
            carry::Action::Missing => {
                eprintln!(
                    "omh: warning: carry_in lists {} — not in this checkout",
                    item.path
                )
            }
            carry::Action::Unchanged => {}
        }
    }
    Ok(())
}

fn run(cwd: &std::path::Path, argv: &[String], cli: &Cli) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let name = &argv[0];

    let adapter =
        Adapter::find(&paths.adapters(), name).map_err(|e| unknown_tool(&paths, name, e))?;
    let profile = Profile::resolve(&paths);

    // A dry run must leave no trace: no branch, no worktree, no staged files.
    // Which identity this session runs as. Ambiguity is an error rather than a
    // guess: silently using the wrong account is expensive and invisible.
    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(
        &paths,
        &adapter,
        cli.account.as_deref(),
        configured.as_deref(),
    )?
    .map(|a| auth::dir(&paths, name, &a));
    if let Some(account_dir) = &account {
        // The mountpoints have to exist before docker binds over them.
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }

    // Always the trunk, never wherever HEAD happens to be: a session started on
    // a feature branch produces a diff against the wrong baseline. You attach to
    // a session, not to a branch — choosing a base was a knob nobody needed.
    //
    // Resolved before the options rather than beside the session, because the
    // plan needs it too: it is where the project's own rules come from when the
    // worktree has none of its own.
    let base = session::default_branch(&paths.repo);
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    // Not on a dry run, which promises to leave no trace: topping up starts a
    // container and writes `~/.omh/facts.json`. What is already cached is used,
    // so the plan it prints is the plan a real launch would build from the same
    // knowledge.
    if !cli.dry_run {
        if let Ok(backend) =
            runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))
        {
            sandbox.top_up(
                &paths,
                backend.program(),
                &profile.sources(adapter::Capability::Hooks)?,
                &own,
                &repo,
            )?;
        }
    }

    let opts = container::Options {
        // A dry run must leave no trace: no branch, no worktree, no staged files.
        staging: if cli.dry_run {
            container::Staging::Skip
        } else {
            container::Staging::Apply
        },
        persist: policy_value(&paths, "persistence")
            .as_deref()
            .unwrap_or("dtach")
            .parse()?,
        tty: true,
        account_dir: account,
        memory_bin: memory::deliver::available(&paths),
        base: Some(base.clone()),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };

    std::fs::create_dir_all(paths.worktrees())?;
    if let Some(explicit) = cli.session.as_deref() {
        session::validate_id(explicit)?;
    }
    let id = session::pick(&paths.worktrees(), cli.session.as_deref(), cli.new);
    let session = Session::new(&paths.worktrees(), id);
    if opts.staging == container::Staging::Apply {
        session.ensure(&paths.repo, &base)?;
        carry_in(&paths, &session)?;
        // Reap before starting another container, and record that this one is
        // in use so it is not reaped by the next launch.
        reap_idle(&paths, &session.id);
        let _ = idle::touch(&paths.runs(), &session.id);
    }

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &argv[1..],
        opts.clone(),
    )?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    say_rules(&plan);
    say_selection(&profile, &opts.repo);
    let hooks_seen = say_hooks(&paths);

    let status_line = match plan.degradation() {
        Some(d) => format!("omh: {} on {} — {d}", adapter.name, session.label()),
        None => format!("omh: {} on {}", adapter.name, session.label()),
    };

    if cli.dry_run {
        println!("{status_line}");
        println!("worktree {}", session.worktree.display());
        println!(
            "\n{} {}",
            backend.program(),
            backend.args(&plan).join(" \\\n       ")
        );
        return Ok(());
    }

    // The session is a running container. Exec into it rather than starting a
    // throwaway, so MCP daemons stay warm and `omh code` has something to
    // attach to.
    //
    // "Many harnesses take turns inhabiting it" is what this comment used to
    // claim, and it was not true: an image is built per harness, so the second
    // harness execed a binary the image does not contain. `session_up` restarts
    // on that mismatch now — a few seconds, not instant. Making it instant again
    // means one image carrying every installed harness.
    let (backend, name) = session_up(
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            tty: false,
            ..opts.clone()
        },
        &sandbox.recipe(),
    )?;
    // The container is up, so the launch happened and the call-out is spent.
    remember_hooks(hooks_seen);
    eprintln!("{status_line}");
    let status = Command::new(backend.program())
        .args(backend.exec_args(&name, &plan.argv, true))
        .status()?;
    eprintln!("\nomh: review with  omh diff {}", session.id);
    std::process::exit(status.code().unwrap_or(1));
}

/// The two things a launch needs from outside the plan: what omh contributes,
/// and what this repo decided.
///
/// Resolved by the caller of `container::plan` rather than inside it, the rule
/// `memory_bin` and `base` already follow — the manifest is a file, and a probe
/// inside `plan` is a probe no test can reach.
///
/// Returned as a pair rather than merged. They arrive together and travel
/// together, which is exactly what made one struct tempting, but "omh generated
/// this" and "this repo asked for this" are the two answers `omh why` exists to
/// keep apart — and a type that holds both cannot help blurring them.
fn resolved(paths: &Paths) -> Result<(base::Own, settings::RepoPolicy)> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let repo = settings::resolve(paths, &manifest)?;
    // What the catalogue still declares, so removing a server takes its feature
    // with it. `omh config mcp rm codegraph` edits `mcp.json` and nothing
    // else, so this read is where that instruction is kept or broken.
    let installed = config::servers(paths)?.into_iter().map(|s| s.key).collect();
    Ok((base::own(&manifest, &repo.off, &installed)?, repo))
}

/// `omh why <thing>` — who put this here, and on what grounds.
///
/// Needs no container and no session: it is a pure function of the manifest and
/// the resolved profile, which is why it can answer even for something you have
/// removed.
fn why_cmd(cwd: &std::path::Path, thing: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;

    // Servers and hooks are the same kind of thing here: installed, from a
    // layer, chosen by omh or by you.
    let mut installed = config::servers(&paths)?;
    installed.extend(config::hooks(&paths)?);

    // What omh ships, for deciding whether your copy has been changed. MCP
    // servers only: hooks and rules sections are generated at launch, so there
    // is nothing of yours to compare — a file of that name is a leftover, and
    // the `Generated` verdict names it as one rather than as your edit.
    let baselines: std::collections::BTreeMap<String, String> = manifest
        .entries
        .iter()
        .filter_map(|e| e.command.clone().map(|c| (e.name.clone(), c)))
        .collect();

    // Hooks init generates from stack detection are omh's writing but not omh's
    // opinion. Reported as neither the base set nor yours, because claiming
    // either would be false in a way this command exists to prevent.
    //
    // The command and layer travel with the name, so the claim is checkable:
    // init writes these only into the shared layer, and only with the command
    // detection produced. A name match alone proved nothing — anyone can write
    // a file called `rust-test.json`.
    let mut derived = std::collections::BTreeMap::new();
    let stack_defs = stack::load_dir(&paths.stacks())?;
    for s in detect::stacks(&stack_defs, &paths.repo) {
        let from = format!("{}, detected from {}", s.name, s.marker);
        for (suffix, command) in [("test", s.test.clone()), ("format", s.format.clone())] {
            derived.insert(
                format!("{}-{suffix}", s.name),
                why::Derived {
                    from: from.clone(),
                    command: command.to_string(),
                    layer: config::Layer::Shared,
                },
            );
        }
    }

    let source = manifest.source();
    let version = manifest.version.clone();
    let catalog = why::Catalog {
        off: settings::resolve(&paths, &manifest)?.off,
        manifest: &manifest,
        baselines,
        installed,
        derived,
    };
    print!(
        "{}",
        why::render_with_source(&catalog, &catalog.why(thing), &version, &source)
    );
    Ok(())
}

#[allow(dead_code)]
fn init(cwd: &std::path::Path) -> Result<()> {
    // Fail fast. Everything below is wasted work outside a repo.
    let paths = Paths::discover(cwd)?;

    // A fresh install has no adapters, so `omh <harness>` would fail no matter
    // what else init did. Ship them before anything else.
    let adapters = install_bundled_adapters(&paths)?;
    let editors = install_bundled(&paths.editors(), bundled::Shipped::Editors)?;
    // The base set ships as data next to the adapters, for the same reason: the
    // opinion should be reviewable by the people it is imposed on. It travels
    // *inside* the binary now — otherwise a released omh installs nothing — but
    // it still lands as a file in `~/.omh/base`, which is where the
    // reviewability actually lives. `omh why` reads the file init seeds from.
    install_bundled(&paths.base(), bundled::Shipped::Base)?;
    // The stacks, for the same reason and by the same route: what a project
    // needs installed is omh's opinion, and an opinion imposed on somebody
    // should be one they can read. Managed, so a shipped fix always lands.
    install_bundled(&paths.stacks(), bundled::Shipped::Stacks)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    std::fs::create_dir_all(paths.worktrees())?;

    // The catalogue, empty and ready. Created rather than left absent so
    // `omh config edit` has somewhere to open and the shape is discoverable
    // without reading a document.
    for cap in adapter::Capability::ALL {
        if cap != adapter::Capability::Mcp {
            std::fs::create_dir_all(paths.root.join(cap.source()))?;
        }
    }

    // Detect rather than ask — from the stacks just installed above, so both
    // this and the provisioning below read one set of definitions rather than
    // two registries free to drift.
    //
    // They are not the same *list*, though: `detect::stacks` filters through
    // `view`, which drops a stack `detect::conventional` has no commands for,
    // while provisioning takes `stack::detected` whole. So a contributed stack
    // is provisioned and offers no hooks — which is the intended direction (an
    // environment without automation beats automation without an environment),
    // and stops being a gap when build-order item 7 ships hooks as data.
    let stack_defs = stack::load_dir(&paths.stacks())?;
    let stacks = detect::stacks(&stack_defs, &paths.repo);
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
    write_if_absent(
        &repo_omh.join("settings.toml"),
        "# What this repo decided. Settings at the top level; `[omh]` switches\n\
         # omh's own features off here without uninstalling anything.\n\
         #\n\
         # Untracked files the worktree needs — a worktree holds only tracked\n\
         # files, so without this the agent lands somewhere that cannot run your\n\
         # app. This is the ONLY path by which a secret reaches the agent, so\n\
         # keep it short and explicit. node_modules belongs in the image, not here.\n\
         #\n\
         # carry_in = [\".env.local\", \"certs/\"]\n\
         carry_in = []\n\
         \n\
         # [omh]\n\
         # codegraph = false\n",
    )?;
    // omh's own hooks are not seeded. They are generated from the manifest at
    // launch, which is the only arrangement in which omh can ship a fix to
    // them: `write_if_absent` never revisits, so a repo initialised before
    // `git-unavailable` was rewritten would have run the broken pattern
    // forever.
    //
    // The stack's two are files, because a toolset does not change weekly and
    // when it does the thing you want is a file you can open, in the repo where
    // the change belongs, reviewed with the commit that made it.
    //
    // Every detected stack gets its two, unconditionally. What the sandbox can
    // run decides whether they *fire* — a setting, applied at render — never
    // whether the file exists. `init` sets a preference; it does not decide a
    // repo's contents on the strength of one machine's image.
    for stack in &stacks {
        for hook in stack_hooks(stack) {
            write_if_absent(&repo_omh.join("hooks").join(hook.name), &hook.body)?;
        }
    }

    // The selection, written out with every catalogue entry named — after the
    // stack hooks, so this repo's own two are in the list it writes.
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
    if !repo_has_selection(&paths)? {
        let lists = catalogue_lists(&paths)?;
        config::write_selection(&paths, config::Layer::Shared, &lists)?;
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
    let mut gaps: Vec<Gap> = Vec::new();
    let mut declined: Vec<Gap> = Vec::new();
    let mut asked = 0usize;
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        // Without it the headline command cannot run, so init is not finished
        // until this exists — and until it exists there is no sandbox to ask
        // about a toolchain.
        if image::exists(backend.program(), &image::tag_for(&adapter)) {
            println!("  image      {} (already built)", image::tag_for(&adapter));
        } else {
            println!(
                "\n  building {} — first run only\n",
                image::tag_for(&adapter)
            );
            image::ensure(backend.program(), &adapter)?;
            println!("\n  image      {}", image::tag_for(&adapter));
        }

        // Which image the question below is asked about. Defaults to the
        // harness layer and is replaced by this repo's stack layer as soon as
        // one is resolved — asking the wrong image is how omh would report a
        // missing `cargo` about a sandbox that has one, or miss one that does
        // not.
        let mut session_tag = image::tag_for(&adapter);

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
            let answered = if candidates.is_empty() {
                Vec::new()
            } else {
                match Command::new(backend.program())
                    .args(stack::predicate_args(
                        &image::tag_for(&adapter),
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
                        println!(
                            "  provision  the sandbox could not be asked ({}) — nothing recorded",
                            out.status
                        );
                        for line in String::from_utf8_lossy(&out.stderr).lines().take(3) {
                            println!("             {line}");
                        }
                        Vec::new()
                    }
                    Ok(out) => doctor::parse(&String::from_utf8_lossy(&out.stdout)),
                    Err(e) => {
                        // Non-fatal, and never fatal *silently*: `init` sets a
                        // repo up, and failing that over a diagnostic would be
                        // the tail wagging the dog — but saying nothing would
                        // let somebody believe the sandbox had been checked.
                        println!("  provision  could not ask the sandbox ({e}) — nothing recorded");
                        Vec::new()
                    }
                }
            };

            for a in answered.iter().filter(|a| !a.ok) {
                if let stack::Verdict::CouldNotAnswer(code) = stack::verdict(a) {
                    println!(
                        "  provision  {}'s condition could not answer{} — not applied",
                        a.name,
                        code.map(|c| format!(" (exit {c})")).unwrap_or_default()
                    );
                }
            }

            // Recorded only when something was actually measured. `reconcile`
            // drops every `true` it is not told about, so writing an empty
            // answer would erase the repo's resolution rather than leave it be.
            if let Some(fired) = fired_from(candidates.len(), &answered) {
                let recorded = record_resolution(&paths, &fired)?;
                for key in recorded.iter().filter(|(_, on)| **on).map(|(k, _)| k) {
                    println!("  provision  {key}");
                }

                // The stack layer, through the same function every launch
                // reads — so what `init` reports built is what `omh run` runs,
                // by construction rather than by two implementations agreeing.
                //
                // Re-resolved from disk rather than reusing `recorded`, which
                // is the committed table alone: `record_resolution` has just
                // written it, and a `false` in `settings.local.toml` means *not
                // on this laptop*, which is the laptop building the image.
                let (own, repo) = resolved(&paths)?;
                let sandbox = sandbox(&paths, &adapter, &repo)?;
                session_tag = sandbox.tag.clone();
                image::ensure_stack(backend.program(), &adapter, &sandbox.recipe())?;
                if sandbox.tag != image::tag_for(&adapter) {
                    println!("  image      {} (this repo's toolchain)", sandbox.tag);
                }

                // And what that image turned out to contain, measured once and
                // remembered: every launch afterwards reads `~/.omh/facts.json`
                // rather than starting a container to ask again.
                //
                // Two readings of one probe. A `needs` that did not resolve is
                // a **provisioning failure** and is reported here — the recipe
                // ran and the environment still does not work, which is exactly
                // what shipping rustup with no `cc` looked like. The same
                // measurements suppress a hook whose program is missing, which
                // is a different question about the same fact.
                let hook_dirs = Profile::resolve(&paths).sources(adapter::Capability::Hooks)?;
                let mut sandbox = sandbox;
                sandbox.top_up(&paths, backend.program(), &hook_dirs, &own, &repo)?;
                for name in &sandbox.owed {
                    if sandbox.resolves.get(name) == Some(&false) {
                        println!("  provision  {name} did not resolve after installing");
                    }
                }
            }
        }

        // Answers already on file, so a question is asked once and not once per
        // `init`. Read through `settings::resolve` rather than the repo's file
        // alone: a toolchain missing on *this* machine is a `settings.local`
        // decision, and the team's answer must not be the only one that counts.
        //
        // Propagated, never defaulted. Every error this raises — an unreadable
        // layer, a table nobody reads, a value omh cannot parse — would
        // otherwise become an empty map, and an empty map is indistinguishable
        // from "nobody has answered anything". omh would then re-ask questions
        // already on file and write the answers back into a file it had just
        // failed to read.
        let mut decided = settings::resolve(&paths, &manifest)?.toolchain;
        // Probed once. The answers change what omh does with the result, never
        // what the sandbox contains, so asking the container twice would cost a
        // second run to be told the same thing.
        let probe = probe_sandbox(backend.program(), &session_tag, &stacks);
        let mut triage = triage_for(&stacks, &probe, &decided);
        // Ask, then act — so a hook somebody asks for is written on this run
        // rather than on the next one.
        if !triage.gaps.is_empty() {
            let answers = ask_about_gaps(&triage.gaps)?;
            if !answers.is_empty() {
                asked = answers.len();
                record_toolchain_answers(&repo_omh, &answers)?;
                decided.extend(answers);
                triage = triage_for(&stacks, &probe, &decided);
            }
        }
        gaps = triage.gaps;
        declined = triage.declined;
    }
    // No harness is no image, and no image is no sandbox to ask about. The
    // hooks are already written either way.

    // Report every decision, so `omh why` has something to explain. Printed as
    // each one is made rather than collected for the end, which is why the
    // image and graph lines below appear inside the summary.
    // The headline is a claim about this run, so it has to be able to stop
    // being true. omh derives what it can and asks only what a probe could not
    // settle; printing "asked nothing" after putting a question on screen would
    // make the promise the tagline is selling into a thing the user just
    // watched it break.
    match asked {
        0 => println!("omh init — decided, asked nothing\n"),
        1 => println!("omh init — decided all but one question\n"),
        n => println!("omh init — decided the rest; asked {n} questions\n"),
    }
    println!("  harnesses  {} ({})", adapters.len(), adapters.join(", "));
    println!("  editors    {} ({})", editors.len(), editors.join(", "));
    match &harness {
        Some(h) => println!(
            "  harness    {h}{}",
            if runtime::installed(h) {
                "  (found on your host)"
            } else {
                "  (default; nothing detected on host)"
            }
        ),
        None => println!("  harness    none — no adapters available"),
    }
    if stacks.is_empty() {
        println!(
            "  stack      none detected — write your test and format hooks into \
             .omh/hooks/"
        );
    } else {
        for s in &stacks {
            println!(
                "  stack      {} (from {}) → test `{}`, format `{}`",
                s.name, s.marker, s.test, s.format
            );
        }
    }
    // Named, with the evidence, because the alternative is the failure this
    // replaces: a hook that runs on turn one and reports `cargo: not found`,
    // which says nothing about who decided to run cargo or where it looked.
    //
    // "will fail", not "will not run". A gap survives to here only when nothing
    // was recorded — no terminal to ask at, or the answer stopped at EOF — and
    // with nothing recorded `render::suppressed_by_toolchain` has no reason to
    // drop the hook, so it runs and fails. Reporting it as disabled would send
    // somebody away believing this was handled, and the next turn would produce
    // the exact error the report claimed to have prevented.
    for g in &gaps {
        println!(
            "  gap        {} needs `{}`, absent from this sandbox → `{}` will fail\n             \
             nothing on file: re-run `omh init` on a terminal, or add \
             {} = \"skip\" under [toolchain]",
            g.stack, g.program, g.command, g.program
        );
    }
    // Reported, not merely obeyed. A hook missing because somebody said so and
    // a hook missing because omh never thought of it look identical in
    // `.omh/hooks/`, and the second one is a bug — so the answer on file is
    // printed every run, with the one line that undoes it.
    for g in &declined {
        println!(
            "  skipped    {} needs `{}` — you chose skip, so `{}` stays off here\n             \
             the hook file is written; drop `{}` from [toolchain] to switch it on",
            g.stack, g.program, g.command, g.program
        );
    }

    // What the repo already documents becomes notes that *point* at it.
    // Printing the seeds instead would derive them every run, show them once,
    // and keep them nowhere.
    match seed_store(&paths) {
        Ok(report) => println!("  memory     {report}"),
        // Never fatal. A repo that cannot be ingested is still a repo omh set
        // up, and failing `init` over the note store would be the tail
        // wagging the dog.
        Err(e) => println!("  memory     not seeded: {e:#}"),
    }

    // Derive, then confirm: a hypothesis worth correcting is not a questionnaire.
    if stacks.len() > 1 {
        println!(
            "\n  ! {} stacks detected; hooks were written for every command the \
             sandbox can run.\n    drop the ones you do not want: .omh/hooks/",
            stacks.len()
        );
    }

    println!("\n  catalogue  {}", paths.root.display());
    println!("  this repo  {}  (committed)", repo_omh.display());
    // The index lives in a container volume, so it has to be built inside the
    // sandbox — one built on the host would land where no session can read it.
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        let args = base::index_args(
            &image::tag_for(&adapter),
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
            Ok(_) => println!(
                "  graph      indexing in background → {}",
                paths.cache_volume()
            ),
            Err(e) => println!("  graph      could not start indexing: {e}"),
        }
    }

    println!("\n  base set  ({})", manifest.version);
    for (name, why) in manifest.rationale() {
        println!("    {name:<10} {why}");
    }
    // Named here because this is the moment somebody wonders what that is and
    // why it was installed without being asked.
    println!("\n  omh why <name>  what it costs, what was considered instead, how to remove it");
    println!("\nnot yet done: recall, cost accounting.");
    println!("next: omh {}", harness.as_deref().unwrap_or("config"));
    Ok(())
}

/// Adapters ship with omh but live in `~/.omh`. Without this a fresh install
/// cannot launch anything, which is the state the tool was in until now.
fn install_bundled_adapters(paths: &Paths) -> Result<Vec<String>> {
    install_bundled(&paths.adapters(), bundled::Shipped::Adapters)?;
    Ok(Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect())
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
fn install_bundled(dest: &std::path::Path, kind: bundled::Shipped) -> Result<Vec<String>> {
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
            let backup = target.with_extension("toml.yours");
            std::fs::write(&backup, &existing)
                .with_context(|| format!("saving your {name} as {}", backup.display()))?;
            // stderr: this is a warning about data, and stdout is the report.
            eprintln!(
                "  replaced   {} (yours saved as {name}.yours)",
                target.display()
            );
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
fn ensure_line(path: &std::path::Path, line: &str) -> Result<()> {
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

fn write_if_absent(path: &std::path::Path, contents: &str) -> Result<()> {
    if !path.exists() {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// One hook a detected stack earns: the file, its body, and the command inside
/// it.
///
/// The command travels with the file rather than being recovered from it. The
/// alternative — pairing `stack_hooks`' output back up with `stack.test` and
/// `stack.format` by array position — makes a reordering of that array silently
/// check the format hook against the test command, and nothing would fail.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StackHook {
    name: String,
    body: String,
    command: String,
}

/// A hook `init` did not write, and the evidence for why.
///
/// This is what a question is built from, so it carries the whole story: the
/// stack that asked for the hook, the command that would not have run, and the
/// program that was missing. A gap that only said "rust is broken" would leave
/// the human to guess what omh actually looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Gap {
    stack: String,
    command: String,
    program: String,
}

/// What `init` found missing, split by whether anybody has answered for it yet.
///
/// No list of hooks to write, deliberately. `init` writes them all: the file is
/// the repo's statement about itself and it is committed, so a toolchain absent
/// from one machine must not decide whether it exists.
#[derive(Debug, Default, PartialEq, Eq)]
struct Triage {
    /// Gaps nobody has answered yet — the only ones `init` may ask about.
    gaps: Vec<Gap>,
    /// Gaps already answered `skip`. Kept rather than dropped: a hook that is
    /// absent because somebody decided it should be, and a hook that is absent
    /// because omh never thought of it, look identical on disk. Only this
    /// distinguishes them, and `omh why` has to be able to.
    declined: Vec<Gap>,
}

/// The key a TOML line assigns to, if it assigns to one.
///
/// Deliberately not a parse. This is used to find a line to *replace* inside a
/// table whose comments and spacing have to survive, and a round trip through a
/// TOML value cannot preserve either. Quotes are trimmed so a bare `cargo` and a
/// quoted `"cargo"` are recognised as the same key, which is what TOML would
/// say too.
fn toml_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let key = line.split('=').next()?.trim().trim_matches('"');
    (!key.is_empty()).then_some(key)
}

/// How a toolchain answer is spelled in `settings.toml`. One source, so the
/// value written and the value `settings::Toolchain` reads back cannot drift.
fn toolchain_word(t: settings::Toolchain) -> &'static str {
    match t {
        settings::Toolchain::Skip => "skip",
        settings::Toolchain::Assume => "assume",
    }
}

/// Put answers into a `settings.toml` without disturbing what it already says.
///
/// Textual, not a serialiser round-trip. `toml::to_string` would reformat the
/// document and drop every comment in it — including the ones `init` wrote to
/// explain `carry_in`, which exist precisely so somebody reading the file later
/// knows what it is for.
///
/// The trap is the second answer. Appending another `[toolchain]` header makes
/// a duplicate table, and TOML refuses the *whole document* — so the next
/// `init` cannot read a file omh wrote itself, and every setting in it is lost
/// at once. New keys therefore go inside the existing table when there is one.
fn with_toolchain_answers(
    existing: &str,
    answers: &BTreeMap<String, settings::Toolchain>,
) -> String {
    let lines: Vec<String> = answers
        .iter()
        .map(|(program, t)| format!("{program} = \"{}\"", toolchain_word(*t)))
        .collect();

    if let Some(at) = existing
        .lines()
        .position(|l| l.trim_start().starts_with("[toolchain]"))
    {
        let mut out: Vec<String> = existing.lines().map(String::from).collect();
        // The table ends at the next header, or at the end of the file. Writing
        // past it would put a `[toolchain]` key inside `[mcp]`.
        let end = out
            .iter()
            .skip(at + 1)
            .position(|l| l.trim_start().starts_with('['))
            .map_or(out.len(), |i| at + 1 + i);

        let mut appended = Vec::new();
        for (program, t) in answers {
            let line = format!("{program} = \"{}\"", toolchain_word(*t));
            // A key already in the table is *replaced*. Appending a second
            // `cargo =` is a duplicate key, which TOML refuses exactly as hard
            // as the duplicate header guarded above — same outcome, and the
            // header check cannot see it.
            match out[at + 1..end]
                .iter()
                .position(|l| toml_key(l) == Some(program))
            {
                Some(i) => out[at + 1 + i] = line,
                None => appended.push(line),
            }
        }
        out.splice(end..end, appended);
        return out.join("\n") + "\n";
    }

    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(
        "\n# Programs omh could not find in this repo's sandbox, and what you\n\
         # said about each. The hook files are written either way — this decides\n\
         # whether they run here. Delete a line to be asked again by `omh init`.\n\
         #   skip    do not run hooks whose command needs it\n\
         #   assume  run them; the sandbox will have it by launch\n\
         # Put a machine-only answer in settings.local.toml instead.\n\
         [toolchain]\n",
    );
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Write the answers into this repo's `settings.toml`.
fn record_toolchain_answers(
    repo_omh: &std::path::Path,
    answers: &BTreeMap<String, settings::Toolchain>,
) -> Result<()> {
    let path = repo_omh.join("settings.toml");
    // Bytes that will not decode are *not* an absent file. Collapsing the two
    // and then writing the result back is how `carry_in`, `[omh]`, `[use]` and
    // `[mcp]` all disappear at once over one stray byte — the read fails, the
    // write succeeds, and somebody's settings are gone. `settings::read` and
    // `install_bundled` both already carry this scar; only `NotFound` means
    // absent.
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "reading {} to record your answer — it is left untouched",
                    path.display()
                )
            })
        }
    };
    std::fs::write(&path, with_toolchain_answers(&existing, answers))
        .with_context(|| format!("recording your answer in {}", path.display()))
}

/// Put one question per missing program, and only when something is actually
/// missing.
///
/// The rule that keeps this from being a wizard: a repo whose sandbox has
/// everything is asked nothing at all, which is every repo until it is not.
/// Each question carries its own evidence — what needs the program, and what
/// would have run — because the failure this replaces was `cargo: not found`,
/// which names neither who wanted cargo nor where omh looked for it.
///
/// Silence is `skip`, so pressing Enter through it writes no hook that cannot
/// run. Not asking at all — no terminal, a CI runner — is the same answer, but
/// it is *not recorded*: nobody chose it, and writing it down would answer a
/// question on behalf of somebody who was never shown it.
fn ask_about_gaps(gaps: &[Gap]) -> Result<BTreeMap<String, settings::Toolchain>> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        return Ok(BTreeMap::new());
    }
    let stdin = std::io::stdin();
    ask_about_gaps_with(gaps, &mut stdin.lock(), &mut std::io::stdout())
}

/// The questioning itself, with the terminal handed in.
///
/// Split from [`ask_about_gaps`] so it can be tested at all. Reading
/// `std::io::stdin()` directly would leave the dedup, the default, and the EOF
/// case — every rule that decides what gets written into somebody's committed
/// settings file — asserted by nothing.
fn ask_about_gaps_with(
    gaps: &[Gap],
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> Result<BTreeMap<String, settings::Toolchain>> {
    // By program, not by hook: `cargo` is one question even though it holds up
    // two of rust's hooks, and the answer settles both.
    let mut by_program: BTreeMap<&str, Vec<&Gap>> = BTreeMap::new();
    for g in gaps {
        by_program.entry(g.program.as_str()).or_default().push(g);
    }

    let mut answers = BTreeMap::new();
    for (program, blocked) in by_program {
        let wanted_by: BTreeSet<&str> = blocked.iter().map(|g| g.stack.as_str()).collect();
        writeln!(
            out,
            "\n  ! `{program}` is not installed in this repo's sandbox.\n    \
             {} detected it, and these will not run: {}",
            wanted_by.into_iter().collect::<Vec<_>>().join(", "),
            blocked
                .iter()
                .map(|g| format!("`{}`", g.command))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(
            out,
            "\n      [s] skip    write no hook needing {program}\n      \
             [a] assume  write them anyway — the sandbox will have {program} by launch\n"
        )?;
        write!(out, "    {program} [S/a] ")?;
        out.flush()?;

        let mut line = String::new();
        // EOF mid-question: the terminal went away. Stop asking rather than
        // spinning through the rest reading empty strings and recording an
        // answer for each — answers nobody gave, written into a committed file
        // and never asked again because they are now on record.
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let answer = match line.trim().to_ascii_lowercase().as_str() {
            "a" | "assume" => settings::Toolchain::Assume,
            _ => settings::Toolchain::Skip,
        };
        answers.insert(program.to_string(), answer);
    }
    Ok(answers)
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
fn fired_from(asked: usize, answered: &[doctor::Outcome]) -> Option<BTreeSet<String>> {
    if asked == 0 {
        return Some(BTreeSet::new());
    }
    // One line per provide, so fewer lines than provides is a report that did
    // not finish — not a report saying "no". Accepting the prefix would make
    // `reconcile` drop every `true` it was not told about and rewrite a
    // committed file without them. `triage_for` was fixed for this same shape
    // earlier, where it only cost a spurious question; here it deletes.
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
fn record_resolution(paths: &Paths, fired: &BTreeSet<String>) -> Result<BTreeMap<String, bool>> {
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
fn installs_for<'a>(
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
fn needs_of(
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
fn probe_targets(
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
fn measure(
    program: &str,
    paths: &Paths,
    tag: &str,
    wanted: &BTreeSet<String>,
) -> Result<BTreeMap<String, bool>> {
    let mut facts = facts::Facts::load(paths);
    let unseen = facts.unseen(tag, wanted);
    if !unseen.is_empty() {
        let borrowed: Vec<&str> = unseen.iter().map(String::as_str).collect();
        let outcomes = match Command::new(program)
            .args(image::probe_args(tag, &doctor::probe_programs(&borrowed)))
            .output()
        {
            Ok(out) => doctor::parse(&String::from_utf8_lossy(&out.stdout)),
            Err(_) => Vec::new(),
        };
        if !outcomes.is_empty() {
            facts.learn(tag, &outcomes);
            facts.save(paths)?;
        }
    }
    Ok(facts.about(tag))
}

/// What this repo's sandbox is: the recipe its stacks provision, the image that
/// recipe produces, and what that image has been measured to contain.
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
struct Sandbox {
    /// Owned, because the definitions they are read from do not outlive this.
    installs: Vec<String>,
    tag: String,
    resolves: BTreeMap<String, bool>,
    /// What the provides this repo installed said must resolve once they had.
    /// Carried here rather than re-derived, so the caller that tops the
    /// measurements up asks about the same list `init` reported on.
    owed: BTreeSet<String>,
}

impl Sandbox {
    fn recipe(&self) -> Vec<&str> {
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
    /// Never fatal, and the errors it can raise are already handled inside
    /// `measure` — a runtime that will not start leaves the facts as they were,
    /// which reads as *nobody has looked*.
    fn top_up(
        &mut self,
        paths: &Paths,
        program: &str,
        hook_dirs: &[PathBuf],
        own: &base::Own,
        repo: &settings::RepoPolicy,
    ) -> Result<()> {
        let wanted = probe_targets(hook_dirs, own, repo, &self.owed)?;
        self.resolves = declared_over(
            measure(program, paths, &self.tag, &wanted)?,
            &repo.toolchain,
        );
        Ok(())
    }
}

/// Measurement, with the `[toolchain]` answers still able to decide.
///
/// **Transitional**, and owed to build-order item 3, which deletes the table
/// with an error naming it. Until then a `cargo = "skip"` somebody committed
/// has to go on working: item 2 replaced the input `render::suppressed_by_probe`
/// reads, and an answer that quietly stops taking effect between two releases
/// is worse than one removed loudly.
///
/// Applied over the measurement rather than beside it, because they answer the
/// same question and only one of them can win. A person's answer wins: `skip`
/// is *this sandbox will not have it*, `assume` is *run the hook anyway*, and
/// each is a statement about the image by somebody who may know its next
/// moment better than a probe of its last one does.
fn declared_over(
    mut measured: BTreeMap<String, bool>,
    decided: &BTreeMap<String, settings::Toolchain>,
) -> BTreeMap<String, bool> {
    for (program, answer) in decided {
        measured.insert(
            program.clone(),
            match answer {
                settings::Toolchain::Skip => false,
                settings::Toolchain::Assume => true,
            },
        );
    }
    measured
}

fn sandbox(paths: &Paths, adapter: &Adapter, repo: &settings::RepoPolicy) -> Result<Sandbox> {
    let defs = stack::load_dir(&paths.stacks())?;
    let detected = stack::detected(&defs, &paths.repo);
    let installs: Vec<String> = installs_for(&detected, &repo.provision)
        .into_iter()
        .map(str::to_string)
        .collect();
    let tag = image::stack_tag(
        adapter,
        &installs.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let resolves = declared_over(facts::Facts::load(paths).about(&tag), &repo.toolchain);
    let owed = needs_of(&detected, &repo.provision);
    Ok(Sandbox {
        installs,
        tag,
        resolves,
        owed,
    })
}

/// Ask the sandbox which of the detected stacks' programs it actually has.
///
/// **Transitional**, and the second container run in `init`. It feeds the
/// `[toolchain]` question only; suppression is decided by `measure`, which asks
/// about the same image and a wider set of programs, and caches the answer.
/// Build-order item 3 deletes the question and this with it.
///
/// Takes the tag rather than deriving it, which is not cosmetic: it derived
/// `image::tag_for(adapter)` while sessions ran the stack layer, so it asked
/// about an image nobody would launch — reporting `cargo` missing in a repo
/// whose sandbox provisions it.
///
/// Never fatal, and never fatal *silently*: every failure path returns no
/// outcomes, which `triage_for` reads as cannot-tell and answers by writing
/// every hook. `init` sets a repo up, and failing that over a diagnostic —
/// or, worse, withholding a repo's hooks because a runtime hiccuped — would be
/// the tail wagging the dog.
///
/// Only stdout is read. A runtime writes its own noise to stderr, and
/// `doctor::parse` already discards anything that is not the protocol, but
/// there is no reason to hand it the chance.
fn probe_sandbox(program: &str, tag: &str, stacks: &[detect::Stack]) -> Vec<doctor::Outcome> {
    let mut wanted: Vec<&str> = stacks
        .iter()
        .flat_map(|s| [s.test.as_str(), s.format.as_str()])
        .filter_map(detect::program)
        .collect();
    // One question per program, not per command: rust asks about `cargo` once
    // even though both of its hooks need it.
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Vec::new();
    }
    match Command::new(program)
        .args(image::probe_args(tag, &doctor::probe_programs(&wanted)))
        .output()
    {
        Ok(out) => doctor::parse(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => Vec::new(),
    }
}

/// What `init` should raise, given whatever the sandbox probe managed to say.
///
/// The fallback is the whole point of the function. An empty report means the
/// probe never ran — the container failed, the image was missing, the runtime
/// hiccuped — and that is indistinguishable from a sandbox with nothing
/// installed if you only look at the outcomes. Treating it as the latter would
/// produce a page of questions about toolchains the user has.
///
/// Silence is therefore *cannot tell*, and `init` asks nothing. The same
/// asymmetry `detect::program` documents — a missed gap costs one confusing
/// hook error, an invented one costs trust in every answer omh gives.
fn triage_for(
    stacks: &[detect::Stack],
    probe: &[doctor::Outcome],
    decided: &BTreeMap<String, settings::Toolchain>,
) -> Triage {
    // An `assume` outranks the probe: it is an answer about the sandbox a
    // session will run in, and the probe only ever saw the one `init` built.
    let assumed = |p: &str| decided.get(p) == Some(&settings::Toolchain::Assume);

    // Keyed on what the probe said was **missing**, never on what it said was
    // present — and the difference is the whole rule, applied per program
    // rather than per report.
    //
    // The protocol prints one line per program, so a program with no line was
    // not answered. Building the set the other way round — "available means it
    // appeared with ok" — makes an unanswered program indistinguishable from a
    // failed one, and `probe.is_empty()` only catches that when *every* line is
    // missing. A report truncated after the first line is non-empty, sails past
    // the guard, and turns every program after the truncation into a definite
    // gap: `init` then asks about a toolchain the user has, Enter records
    // `skip` in the committed settings, and the hooks are off for everyone who
    // clones the repo — with the answer on file, so nobody is asked again.
    let mut triage = if probe.is_empty() {
        Triage::default()
    } else {
        let missing: std::collections::BTreeSet<&str> = probe
            .iter()
            .filter(|o| !o.ok)
            .map(|o| o.name.as_str())
            .collect();
        Triage {
            gaps: missing_programs(stacks, &|p| !missing.contains(p) || assumed(p)),
            declined: Vec::new(),
        }
    };

    // A question with an answer on file is not a question. Moved rather than
    // discarded, so `init` can still say what it did without asking again.
    let (declined, unanswered) = triage
        .gaps
        .into_iter()
        .partition(|g| decided.get(&g.program) == Some(&settings::Toolchain::Skip));
    triage.gaps = unanswered;
    triage.declined = declined;
    triage
}

/// Which of a repo's stack commands name a program the sandbox does not have.
///
/// Pure, with the sandbox injected as a predicate — the same shape as
/// `detect::preferred_harness` and for the same reason it gives: *"the harness
/// itself runs in the sandbox"*. Whether `cargo` exists is a fact about a
/// machine, and the machine that matters is not the one running `init`. Folding
/// the lookup in here would make the answer untestable and would tempt a caller
/// into answering from the host, which is the confusion this exists to end.
///
/// Judged per *command*, never per stack: `go test ./...` needs `go` and
/// `gofmt -w .` needs `gofmt`, so one stack can be half-served.
fn missing_programs(stacks: &[detect::Stack], available: &dyn Fn(&str) -> bool) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for stack in stacks {
        for hook in stack_hooks(stack) {
            // `None` is *cannot tell*. Only a program omh positively read and
            // positively failed to find is a gap; everything else is silence,
            // and silence raises nothing.
            if let Some(p) = detect::program(&hook.command) {
                if !available(p) {
                    gaps.push(Gap {
                        stack: stack.name.clone(),
                        command: hook.command.clone(),
                        program: p.to_string(),
                    });
                }
            }
        }
    }
    gaps
}

/// The hooks a detected stack gets: run the tests at turn end, format on edit.
///
/// Files rather than prose. A sentence in the rules describing `cargo test` is
/// a sentence; a hook is the thing that runs it — and once written it is the
/// repo's, editable and committed, so a teammate cloning gets the project's
/// test command with the project.
///
/// Written for every detected stack, whatever the sandbox on *this* machine
/// happens to hold. The file is committed and travels; whether `cargo` is
/// installed is a fact about one computer, and letting it decide whether the
/// file exists at all would let whoever ran `init` first impose their laptop on
/// everybody who clones afterwards — permanently, because `write_if_absent`
/// never revisits. What a missing toolchain governs is whether the hook *runs*
/// here, which is `[toolchain]` in settings and is applied at render.
///
/// Extracted from `init` so the commands can be asserted without a container.
/// The guard that used to cover them read the generated `AGENTS.md`, which no
/// longer exists.
fn stack_hooks(stack: &detect::Stack) -> [StackHook; 2] {
    // Names from `notice::stack_hook_names`, never spelled again here: the
    // launcher compares what detection expects against what the directory
    // holds, and two spellings of `rust-test` would make it report a hook
    // missing that `init` had just written.
    let [test, format] = notice::stack_hook_names(stack);
    [
        StackHook {
            name: format!("{test}.json"),
            body: format!("{{ \"on\": \"turn-end\", \"run\": \"{}\" }}\n", stack.test),
            command: stack.test.clone(),
        },
        StackHook {
            name: format!("{format}.json"),
            body: format!(
                "{{ \"on\": \"after-tool\", \"tools\": [\"edit\"], \"run\": \"{}\" }}\n",
                stack.format
            ),
            command: stack.format.clone(),
        },
    ]
}

/// Run the harness's own login inside a sandbox, with this account's credential
/// files bind-mounted writable. There is no separate capture step: the login
/// writes straight through to the host.
fn auth_cmd(cwd: &std::path::Path, harness: &str, account: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let adapter = Adapter::find(&paths.adapters(), harness)?;

    if adapter.creds.is_empty() {
        anyhow::bail!(
            "adapter {harness} declares no credential paths, so there is nothing to capture"
        );
    }

    auth::validate_name(account)?;
    let account_dir = auth::dir(&paths, harness, account);
    let already = auth::is_captured(&paths, &adapter, account);
    auth::prepare(&adapter, &account_dir, "/home/agent")?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    image::ensure(backend.program(), &adapter)?;

    // A throwaway: logging in must not leave a branch behind.
    let session = Session::scratch(paths.scratch("auth"), "auth".into());
    session.ensure(&paths.repo, "")?;
    let (own, repo) = resolved(&paths)?;

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &[],
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: true,
            account_dir: Some(account_dir.clone()),
            memory_bin: memory::deliver::available(&paths),
            // Empty, like the base this scratch session was created with at
            // `session.ensure(&paths.repo, "")`: a login is not work on the
            // project, so there are no project rules to look up.
            base: None,
            omh: own,
            repo,
            // The harness image, not this repo's stack layer, for the reason
            // `base` is `None`: a login is not work on the project. Building a
            // toolchain to type a password would spend minutes on a container
            // that is thrown away, and the credential paths a login writes are
            // the same in both images.
            image: image::tag_for(&adapter),
            // So nothing has been measured about it here, and nothing is
            // suppressed. That is the safe direction — a login session running
            // one hook too many costs nothing, and this container exists for
            // the length of an OAuth redirect.
            resolves: BTreeMap::new(),
        },
    )?;
    plan.validate(&backend.caps())?;
    image::ensure_network(backend.program(), &plan.network)?;

    println!(
        "omh auth: logging {harness} in as `{account}`{}",
        if already { " (re-authenticating)" } else { "" }
    );
    println!("  credentials → {}", account_dir.display());
    if let Some(hint) = &adapter.login {
        println!("  next        → {hint}");
    }
    println!();
    let status = Command::new(backend.program())
        .args(backend.args(&plan))
        .status()?;
    if let Err(e) = session.remove(&paths.repo, "") {
        // A leftover `auth` worktree wins `session::current()` and silently
        // becomes the session the next launch runs in.
        eprintln!("omh: warning: could not remove the auth worktree: {e}");
    }

    // Host paths, not guest ones: the guest path names a container that has
    // already been torn down and that the user cannot inspect.
    let unfilled: Vec<std::path::PathBuf> =
        auth::unfilled(&adapter, &account_dir, auth::GUEST_HOME)
            .iter()
            .map(|guest| {
                account_dir.join(
                    guest
                        .strip_prefix(auth::GUEST_HOME)
                        .unwrap_or(guest.as_path()),
                )
            })
            .collect();
    auth::login_outcome(status.success(), &unfilled)
        .map_err(|e| e.context(format!("run `omh auth {harness} {account}` again")))?;
    println!("\nomh: `{account}` captured for {harness}");
    let all = auth::accounts(&paths, &adapter);
    if all.len() > 1 {
        println!("  accounts: {}", all.join(", "));
        println!("  choose per project with `omh repo set account <name>`");
    }
    Ok(())
}

fn ls(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;

    println!("harnesses:");
    let adapters = Adapter::load_dir(&paths.adapters())?;
    if adapters.is_empty() {
        println!("  (none — add {}/<name>.toml)", paths.adapters().display());
    }
    for a in &adapters {
        let accounts = auth::accounts(&paths, a);
        let creds = if accounts.is_empty() {
            "not authed".to_string()
        } else {
            accounts.join(", ")
        };
        println!("  {:<10} {}", a.name, creds);
    }

    let editors = editor::Editor::load_dir(&paths.editors())?;
    if !editors.is_empty() {
        println!("\neditors:");
        for e in &editors {
            let state = if runtime::installed(&e.bin) {
                "installed"
            } else {
                "not installed"
            };
            println!("  {:<10} {state}", e.name);
        }
    }

    println!("\nsessions:");
    let sessions = session::list(&paths.worktrees());
    if sessions.is_empty() {
        println!("  (none)");
    }
    let base = session::default_branch(&paths.repo);
    for id in sessions {
        let sess = Session::new(&paths.worktrees(), id.clone());
        let drift = match sess.behind(&paths.repo, &base) {
            0 => String::new(),
            n => format!("  ({n} behind {base})"),
        };
        println!("  {id:<10} {}{drift}", sess.label());
    }
    Ok(())
}

fn diff(cwd: &std::path::Path, id: Option<&str>, base: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| session::default_branch(&paths.repo));
    let out = session.diff(&paths.repo, &base)?;
    if out.trim().is_empty() {
        // Silence reads as breakage. Say which comparison came up empty.
        println!("no changes on {} (against {base})", session.label());
    } else {
        print!("{out}");
    }
    Ok(())
}

/// The session a command acts on when it acts on work already done.
///
/// Deliberately not `session::pick`: that invents the *next* id when none
/// exists, which is right for a launch — it is about to create that worktree —
/// and wrong for every command that operates on a session that must already be
/// there. Committing into a fabricated id would fail somewhere further down,
/// about a path nobody named.
fn existing_session(paths: &Paths, explicit: Option<&str>) -> Result<Session> {
    let id = match explicit {
        Some(id) => {
            session::validate_id(id)?;
            id.to_string()
        }
        None => session::current(&paths.worktrees())
            .context("no sessions yet — start one with `omh claude`")?,
    };
    let session = Session::new(&paths.worktrees(), id);
    anyhow::ensure!(
        session.worktree.exists(),
        "no session {} — `omh s ls` lists them",
        session.id
    );
    Ok(session)
}

fn commit(
    cwd: &std::path::Path,
    id: Option<&str>,
    message: Option<&str>,
    skip_carried: bool,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;

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
    let s = if n == 1 { "commit" } else { "commits" };
    println!("committed to {} ({n} {s} on the branch)", session.label());
    Ok(())
}

fn push(cwd: &std::path::Path, id: Option<&str>, name: Option<&str>, pr: bool) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let target = session.push(name)?;
    println!("  {} → origin/{target}", session.label());

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

fn rm(cwd: &std::path::Path, id: &str) -> Result<()> {
    session::validate_id(id)?;
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());

    // Drop the graph with the code it describes, while the container is still
    // around to do it. Otherwise the index outlives the worktree forever.
    //
    // Then take the container itself down. A session is the container *and* the
    // worktree, and removing only the worktree leaves a half that can never be
    // reached again: the bind mount still points at the deleted directory, the
    // next launch recreates it at a new inode the mount does not follow, and
    // `session_up` — seeing a container that is up — execs into it and gets
    // "current working directory is outside of container mount namespace root"
    // for every command from then on. Nothing else ever removes it.
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        let name = paths.container(id);
        if image::container_running(backend.program(), &name) {
            let project = base::project_name(&paths.repo_name(), id);
            let _ = Command::new(backend.program())
                .args(backend.exec_args(&name, &base::drop_graph_command(&project), false))
                .output();
        }
        // Best-effort: a container that was never started has nothing to
        // remove, and that must not stop the worktree from going.
        let _ = image::container_remove(backend.program(), &name);
    }

    // The third thing a session owns. Staging is re-rendered on every launch so
    // leaving it costs nothing that breaks — but the `last-used` marker beside
    // it is what says a session ran here, and a marker with no session behind it
    // is how `s ls` learns to report a leftover that is not there any more.
    let _ = std::fs::remove_dir_all(paths.runs().join(id));

    // The branch is reported honestly rather than always claimed as kept: one
    // that never received a commit preserves nothing, and saying otherwise
    // trains people to ignore a namespace filling with dead refs.
    let base = session::default_branch(&paths.repo);
    match session.remove(&paths.repo, &base)? {
        session::Removed::BranchKept => {
            let n = session.commits(&paths.repo, &base);
            let s = if n == 1 { "commit" } else { "commits" };
            println!("removed session {id}; branch omh/{id} kept ({n} {s} to review)");
            println!("  review with  git log {base}..omh/{id}");
            println!("  discard with git branch -D omh/{id}");
        }
        session::Removed::BranchDropped => {
            println!("removed session {id}; branch omh/{id} dropped (no commits)");
        }
        session::Removed::NoBranch => println!("removed session {id}"),
    }

    // The review moment rides on something already happening rather than a
    // ritual nobody performs. Best-effort on purpose: a store omh cannot read
    // is a reason to say nothing, never a reason to leave a session that
    // cannot be removed.
    if let Ok(notes) = memory::load(&paths) {
        if let Some(line) = memory::session_nudge(&notes, id) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    const BUNDLED_ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BUNDLED_EDITORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/editors");

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
        install_bundled(&paths.base(), bundled::Shipped::Base).unwrap();
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

        let (own, _) = resolved(&paths).unwrap();
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
        let (off, policy) = resolved(&paths).unwrap();
        assert!(
            !off.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "and `[omh]` in this repo has to reach it: {:?}",
            off.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );
        assert!(
            off.hooks.iter().any(|h| h.name == "git-unavailable"),
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

    /// A missing toolchain reports a gap and nothing more. It must not reach
    /// back into what `init` writes: `.omh/hooks/` is committed and travels, so
    /// deciding a file's existence from one machine's image lets whoever ran
    /// `init` first impose their laptop on everyone who clones after —
    /// permanently, because `write_if_absent` never revisits.
    ///
    /// Asserted over every shipped stack rather than this repo's own stack: a
    /// rust-shaped implementation would pass a rust-only guard.
    #[test]
    fn what_the_sandbox_lacks_is_reported_and_never_subtracted_from_the_repo() {
        let all = detect::all_shipped();
        let unconditional: Vec<_> = all.iter().flat_map(stack_hooks).collect();

        // A sandbox holding none of the toolchains. Every command is a gap, and
        // the hooks the repo gets are exactly the ones it would have got with
        // no probe at all.
        let bare = missing_programs(&all, &|_| false);
        assert_eq!(
            bare.len(),
            all.len() * 2,
            "every unrunnable command must be reported, not silently dropped"
        );
        assert_eq!(
            all.iter().flat_map(stack_hooks).collect::<Vec<_>>(),
            unconditional,
            "and what init writes is not a function of the probe at all"
        );

        // A sandbox holding everything: nothing to raise.
        assert!(
            missing_programs(&all, &|_| true).is_empty(),
            "nothing is missing here"
        );
    }

    /// `go test ./...` needs `go`; `gofmt -w .` needs `gofmt`. One stack, two
    /// programs — so availability is a fact about a *command*, never about a
    /// stack. A per-stack check reports both or neither, and both answers are
    /// wrong when only one of the two tools is installed.
    #[test]
    fn one_stack_can_need_two_programs_and_each_is_judged_alone() {
        let go = [detect::known(&detect::shipped(), "go").expect("go is a known stack")];
        let gaps = missing_programs(&go, &|p| p == "go");

        assert_eq!(gaps.len(), 1, "only gofmt is missing: {gaps:?}");
        assert_eq!(gaps[0].program, "gofmt", "and it names which: {gaps:?}");
        assert_eq!(gaps[0].command, "gofmt -w .");
    }

    /// When `detect::program` cannot read a command it says `None`, and `None`
    /// is *cannot tell* rather than *needs nothing*. Raising a gap on that
    /// basis would question somebody about a toolchain they have, over a
    /// command omh never managed to read.
    #[test]
    fn a_command_omh_cannot_read_raises_no_gap() {
        let odd = detect::Stack {
            name: "odd".into(),
            marker: "odd.toml".into(),
            // Assignments alone name no executable, so `program` returns None.
            test: "FOO=1".into(),
            format: "FOO=1".into(),
        };
        // A sandbox with nothing at all: the only thing that could raise a gap
        // here is the check itself.
        assert!(
            missing_programs(&[odd], &|_| false).is_empty(),
            "omh must not invent a gap out of a command it could not read"
        );
    }

    /// A probe that reported nothing means the container never ran it — the
    /// failure `doctor::no_output_is_never_a_pass` already guards for the main
    /// probe. Read as *no program is installed*, it would interrogate somebody
    /// about four toolchains they have, because a runtime hiccup looks
    /// identical to an empty sandbox.
    #[test]
    fn a_probe_that_did_not_run_asks_nothing() {
        let all = detect::all_shipped();
        let t = triage_for(&all, &[], &BTreeMap::new());

        assert!(
            t.gaps.is_empty(),
            "silence raises no question: {:?}",
            t.gaps
        );
        assert!(t.declined.is_empty(), "and settles none: {:?}", t.declined);
    }

    /// The wiring, through the real parser rather than a hand-built `Outcome`:
    /// `triage_for` could have been reading the wrong field of the protocol
    /// forever and a fabricated input would never have shown it.
    #[test]
    fn the_triage_acts_on_what_the_probe_reported() {
        let go = [detect::known(&detect::shipped(), "go").expect("go is a known stack")];
        let probe = doctor::parse("ok\tgo\tresolves\nfail\tgofmt\tnot installed in the sandbox\n");
        let t = triage_for(&go, &probe, &BTreeMap::new());

        assert_eq!(t.gaps.len(), 1, "only gofmt is missing: {:?}", t.gaps);
        assert_eq!(t.gaps[0].program, "gofmt");
    }

    /// The probe prints one line per program, so a program with **no** line is
    /// unknown — not absent. Reading silence as absence is the failure this
    /// module's asymmetry exists to prevent, and `probe.is_empty()` only catches
    /// the all-or-nothing form of it: a report truncated after the first line is
    /// non-empty, so it sails past that guard and every unmentioned program
    /// becomes a definite gap.
    ///
    /// What that costs is not one confusing message. `init` asks about a
    /// toolchain the user has, Enter records `skip` into the **committed**
    /// `settings.toml`, and the hooks are switched off for everyone who clones
    /// the repo — with the answer on file, so nobody is ever asked again.
    #[test]
    fn a_program_the_probe_never_mentioned_raises_no_gap() {
        let go = [detect::known(&detect::shipped(), "go").expect("go is a known stack")];
        // The container died after flushing one line. `gofmt` was asked about
        // and never answered.
        let probe = doctor::parse("ok\tgo\tresolves\n");
        let t = triage_for(&go, &probe, &BTreeMap::new());

        assert!(
            t.gaps.is_empty(),
            "a line that never arrived is silence, not a missing gofmt: {:?}",
            t.gaps
        );
    }

    /// `assume` exists for the sandbox that gains a tool after `init` looked —
    /// a base image the user maintains, something installed at launch. The
    /// probe is evidence about one moment, and the person who owns the image
    /// knows more about the next one than omh does. So a recorded `assume`
    /// beats the probe, rather than being overruled by it every run.
    #[test]
    fn an_assumed_program_is_not_raised_however_the_probe_answered() {
        let go = [detect::known(&detect::shipped(), "go").expect("go is a known stack")];
        let probe = doctor::parse("fail\tgo\tnot installed\nfail\tgofmt\tnot installed\n");
        let decided = BTreeMap::from([("gofmt".to_string(), settings::Toolchain::Assume)]);

        let t = triage_for(&go, &probe, &decided);
        assert_eq!(t.gaps.len(), 1, "only the undecided one: {:?}", t.gaps);
        assert_eq!(t.gaps[0].program, "go");
        assert!(
            t.declined.is_empty(),
            "assume is not a decline: {:?}",
            t.declined
        );
    }

    /// The point of writing the answer down. A question re-asked on every
    /// `init` is a wizard, which is the thing omh sells itself as not being —
    /// so a recorded `skip` moves out of the list `init` would ask about,
    /// without being forgotten.
    #[test]
    fn a_recorded_skip_is_not_asked_again_but_is_still_accounted_for() {
        let go = [detect::known(&detect::shipped(), "go").expect("go is a known stack")];
        let probe = doctor::parse("ok\tgo\tresolves\nfail\tgofmt\tnot installed\n");
        let decided = BTreeMap::from([("gofmt".to_string(), settings::Toolchain::Skip)]);

        let t = triage_for(&go, &probe, &decided);
        assert!(
            t.gaps.is_empty(),
            "an answered question is not asked again: {:?}",
            t.gaps
        );
        assert_eq!(
            t.declined.len(),
            1,
            "but it is still accounted for, not forgotten"
        );
        assert_eq!(t.declined[0].program, "gofmt");
    }

    /// The answer has to land in a file that still parses, and still says
    /// everything it said before. Rewriting the document through a TOML
    /// serialiser would round-trip away every comment in it — including the
    /// ones `init` itself wrote to explain `carry_in`.
    #[test]
    fn an_answer_is_added_without_disturbing_what_the_file_already_said() {
        let before =
            "# what this repo decided\ncarry_in = [\".env\"]\n\n[omh]\ncodegraph = false\n";
        let after = with_toolchain_answers(
            before,
            &BTreeMap::from([("cargo".to_string(), settings::Toolchain::Skip)]),
        );

        assert!(
            after.contains("# what this repo decided"),
            "the comments survive: {after}"
        );
        let parsed: toml::Table = toml::from_str(&after).expect("must still be TOML");
        assert_eq!(
            parsed["carry_in"].as_array().unwrap().len(),
            1,
            "and so does the setting: {after}"
        );
        assert_eq!(parsed["toolchain"]["cargo"].as_str(), Some("skip"));
    }

    /// The other duplicate, and the one the header guard does not catch. Adding
    /// a key already in the table produces `cargo = "skip"` beside
    /// `cargo = "assume"`, which TOML refuses exactly as hard as a duplicate
    /// header — so the file omh wrote becomes one omh cannot read, and every
    /// setting in it goes with it.
    ///
    /// Reachable whenever `decided` comes back without an answer that is
    /// nonetheless on file: a `[toolchain]` value omh cannot parse makes
    /// `settings::resolve` fail, and the question is asked again.
    #[test]
    fn answering_a_second_time_replaces_the_line_rather_than_repeating_the_key() {
        let out = with_toolchain_answers(
            "# keep me\n[toolchain]\ncargo = \"assume\"\ngofmt = \"skip\"\n",
            &BTreeMap::from([("cargo".to_string(), settings::Toolchain::Skip)]),
        );

        let parsed: toml::Table = toml::from_str(&out).expect("must still be TOML");
        assert_eq!(
            parsed["toolchain"]["cargo"].as_str(),
            Some("skip"),
            "the new answer wins: {out}"
        );
        assert_eq!(
            parsed["toolchain"]["gofmt"].as_str(),
            Some("skip"),
            "and the untouched one survives: {out}"
        );
        assert!(out.contains("# keep me"), "comments survive: {out}");
    }

    /// A file omh cannot read is not a file omh may replace.
    ///
    /// `read_to_string(..).unwrap_or_default()` collapses *absent* and *cannot
    /// be read* into the same empty string, and the next line writes it back —
    /// so one non-UTF-8 byte in `settings.toml` costs the user their `carry_in`,
    /// their `[omh]` switches, their `[use]` list and their `[mcp]` environment,
    /// silently. This repo has already paid for that lesson twice: `settings::
    /// read` and `install_bundled`, whose comment reads *"the read failed, the
    /// write succeeded, and somebody's edit was gone"*.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // Valid TOML but for one byte no UTF-8 decoder will accept.
        let original: Vec<u8> = b"carry_in = [\"\xff.env\"]\n".to_vec();
        std::fs::write(&path, &original).unwrap();

        let answers = BTreeMap::from([("cargo".to_string(), settings::Toolchain::Skip)]);
        let result = record_toolchain_answers(dir.path(), &answers);

        assert!(
            result.is_err(),
            "an unreadable settings file must be reported, not assumed empty"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "and left exactly as it was"
        );
    }

    /// The trap. A second `[toolchain]` header is a duplicate table, which is
    /// not merely untidy — TOML refuses the whole document, so the next `init`
    /// fails to read a file omh wrote itself, and every setting in it is lost
    /// at once.
    #[test]
    fn a_second_answer_joins_the_table_rather_than_opening_another() {
        let once = with_toolchain_answers(
            "[toolchain]\ncargo = \"skip\"\n",
            &BTreeMap::from([("gofmt".to_string(), settings::Toolchain::Assume)]),
        );

        assert_eq!(
            once.matches("[toolchain]").count(),
            1,
            "one table, not two: {once}"
        );
        let parsed: toml::Table = toml::from_str(&once).expect("must still be TOML");
        assert_eq!(parsed["toolchain"]["cargo"].as_str(), Some("skip"));
        assert_eq!(parsed["toolchain"]["gofmt"].as_str(), Some("assume"));
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
    /// asymmetry `detect::program` and `triage_for` are built on, at the one
    /// point where acting on it would delete somebody's file contents.
    #[test]
    fn a_resolution_nobody_measured_is_never_recorded() {
        assert_eq!(
            fired_from(3, &[]),
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
    /// This is the same defect `triage_for` was fixed for earlier, in the
    /// sibling function, where it only cost a spurious question. Here it edits
    /// a file under version control.
    #[test]
    fn a_partial_report_is_not_a_resolution() {
        let truncated = [outcome("rust/toolchain", true, "applies")];
        assert_eq!(
            fired_from(2, &truncated),
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
            fired_from(0, &[]),
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
            fired_from(3, &answered),
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
        let got = installs_for(&[node], &all);

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
            installs_for(&[&def], &resolved),
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
            installs_for(&[&def], &resolved),
            vec!["install rustup"],
            "the predicate said the linker applies; a person said not here, and \
             a person outranks a predicate"
        );
    }

    /// An answer somebody already committed goes on working.
    ///
    /// Suppression now reads measurements, and `[toolchain]` is the table it
    /// used to read. Deleting that table is build-order item 3 and comes with
    /// an error naming it — but between the two, a `cargo = "skip"` sitting in
    /// somebody's `settings.toml` must not quietly stop taking effect. An
    /// answer that silently becomes a no-op is the worst of the three
    /// possibilities: it is not honoured, and nothing says so.
    ///
    /// Both answers keep their meanings, which are not symmetrical. `skip`
    /// says *this sandbox will not have it*, so it forces an absence over a
    /// measurement that has not been taken. `assume` says *run the hook
    /// anyway* — for a base image the user maintains, or a tool installed at
    /// launch — so it has to survive a measurement that says the program is
    /// missing, or the one answer that keeps a hook working would be the one
    /// answer measurement overrules.
    #[test]
    fn a_committed_toolchain_answer_still_decides_until_it_is_deleted() {
        use settings::Toolchain;
        let measured = BTreeMap::from([
            ("cargo".to_string(), true),
            ("shellcheck".to_string(), false),
        ]);
        let decided = BTreeMap::from([
            ("cargo".to_string(), Toolchain::Skip),
            ("shellcheck".to_string(), Toolchain::Assume),
        ]);

        let got = declared_over(measured, &decided);
        assert_eq!(
            got.get("cargo"),
            Some(&false),
            "`skip` means this sandbox will not have it, whatever a probe saw"
        );
        assert_eq!(
            got.get("shellcheck"),
            Some(&true),
            "`assume` means run the hook anyway, whatever a probe saw"
        );
        // And a program with no answer keeps its measurement, including the
        // unmeasured case — absent stays absent, which suppresses nothing.
        let kept = declared_over(
            BTreeMap::from([("cc".to_string(), false)]),
            &BTreeMap::new(),
        );
        assert_eq!(kept, BTreeMap::from([("cc".to_string(), false)]));
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
            sandbox(&paths, &adapter, &repo).unwrap().tag
        };

        let nothing = with(&[]);
        let pnpm = with(&["node/pnpm"]);
        let yarn = with(&["node/yarn"]);

        assert_eq!(
            nothing,
            image::tag_for(&adapter),
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
        let owed = needs_of(&[&def], &repo.provision);
        let asked = probe_targets(&[hooks], &Default::default(), &repo, &owed).unwrap();

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
            needs_of(&[&def], &resolved),
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
            say_hooks(&paths).is_none(),
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
        record_resolution(&paths, &fired).unwrap();

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

    // ── the question ────────────────────────────────────────────────────────

    fn gap(stack: &'static str, command: &'static str, program: &'static str) -> Gap {
        Gap {
            stack: stack.into(),
            command: command.into(),
            program: program.into(),
        }
    }

    fn ask(gaps: &[Gap], typed: &str) -> (BTreeMap<String, settings::Toolchain>, String) {
        let mut out = Vec::new();
        let answers = ask_about_gaps_with(
            gaps,
            &mut std::io::BufReader::new(typed.as_bytes()),
            &mut out,
        )
        .unwrap();
        (answers, String::from_utf8(out).unwrap())
    }

    /// A decision is about a program, so `cargo` is one question even though it
    /// holds up both of rust's hooks. Asking twice would be asking the same
    /// thing twice and would let somebody answer it two different ways.
    #[test]
    fn one_question_per_program_however_many_hooks_it_blocks() {
        let gaps = [
            gap("rust", "cargo test", "cargo"),
            gap("rust", "cargo fmt", "cargo"),
        ];
        let (answers, shown) = ask(&gaps, "a\n");

        assert_eq!(answers.len(), 1, "one program, one answer: {answers:?}");
        assert_eq!(shown.matches("[S/a]").count(), 1, "asked once: {shown}");
        // Both blocked commands still have to appear, or the question is
        // cheaper to ask than it is to answer.
        assert!(shown.contains("cargo test"), "{shown}");
        assert!(shown.contains("cargo fmt"), "{shown}");
    }

    /// Enter has to be the safe answer. Somebody holding it down through a
    /// first-run prompt must not end up with a hook that fails on every turn —
    /// silence degrades to nothing, never to wrong.
    #[test]
    fn enter_is_skip_so_holding_it_down_writes_no_broken_hook() {
        let (answers, _) = ask(&[gap("rust", "cargo test", "cargo")], "\n");
        assert_eq!(answers.get("cargo"), Some(&settings::Toolchain::Skip));

        // And a typo is not read as consent to the other branch either.
        let (typo, _) = ask(&[gap("rust", "cargo test", "cargo")], "yes please\n");
        assert_eq!(typo.get("cargo"), Some(&settings::Toolchain::Skip));

        let (yes, _) = ask(&[gap("rust", "cargo test", "cargo")], "a\n");
        assert_eq!(yes.get("cargo"), Some(&settings::Toolchain::Assume));
    }

    /// EOF means the terminal went away. Reading on would hand back an empty
    /// line per remaining question and record `skip` for every one of them —
    /// answers nobody gave, written into a committed file, and never asked
    /// again because they are now on record.
    #[test]
    fn a_terminal_that_goes_away_records_no_answer_it_was_not_given() {
        let gaps = [
            gap("rust", "cargo test", "cargo"),
            gap("go", "gofmt -w .", "gofmt"),
        ];
        let (answers, _) = ask(&gaps, "");
        assert!(
            answers.is_empty(),
            "nothing was answered, so nothing is recorded: {answers:?}"
        );

        // One answer given, then the pipe closes: keep the one, invent nothing.
        let (partial, _) = ask(&gaps, "a\n");
        assert_eq!(partial.len(), 1, "only what was typed: {partial:?}");
        assert_eq!(partial.get("cargo"), Some(&settings::Toolchain::Assume));
    }

    /// A detected stack earns hooks, not prose. The guard that used to cover
    /// this read the generated `AGENTS.md` — a sentence saying `cargo test`
    /// runs nothing, and once that file stopped being written the commands
    /// were asserted nowhere.
    #[test]
    fn a_detected_stack_gets_a_hook_that_runs_its_commands() {
        // Every stack omh knows, not `stacks(CARGO_MANIFEST_DIR)` — which is
        // rust and nothing else, so `init` in a node, python or go repo could
        // write files the renderer rejects and the suite would not notice.
        for stack in detect::all_shipped() {
            let hooks = stack_hooks(&stack);
            let by = |suffix: &str| {
                hooks
                    .iter()
                    .find(|h| h.name.ends_with(suffix))
                    .map(|h| h.body.clone())
                    .unwrap_or_else(|| panic!("{} has no {suffix}", stack.name))
            };

            // omh's words, not a harness's. A seeded file spelling `Stop`
            // would work on exactly one harness, and the file `init` writes is
            // the example every hand-written one is copied from.
            let test = by("-test.json");
            assert!(test.contains(&stack.test), "must run the tests: {test}");
            assert!(test.contains("\"turn-end\""), "at turn end: {test}");

            let format = by("-format.json");
            assert!(
                format.contains(&stack.format),
                "must format the code: {format}"
            );
            assert!(
                format.contains("\"edit\""),
                "when a file is written: {format}"
            );

            // Through `hook::Hook::parse`, not `serde_json::Value`. Valid
            // JSON is not a valid hook: a seeded file saying `"on": "Stop"` or
            // `"tools": ["Edit"]` parses as a document and is refused by the
            // renderer at launch, breaking every session in that repo. And
            // `stack_hooks` interpolates a command into a JSON string literal
            // with no escaping, so a future command containing a quote would
            // produce a file nothing could read.
            for h in hooks {
                crate::hook::Hook::parse(&h.body, &h.name).unwrap_or_else(|e| {
                    panic!(
                        "{} seeds a file omh cannot read: {e:#} in {}",
                        stack.name, h.body
                    )
                });
            }
        }
    }

    /// The safety property, restated where it now lives.
    ///
    /// It used to be `Layer::DEFAULT_WRITE`: one flag, one default, and an
    /// unqualified write could never reach version control. `--layer` split
    /// into two commands because the two scopes want opposite defaults, so the
    /// constant went — and the property it carried did not. Neither command's
    /// default may be the committed file, and reaching it has to be asked for
    /// in so many words.
    #[test]
    fn no_unqualified_write_can_reach_version_control() {
        assert!(
            !repo_layer(false).is_committed(),
            "omh repo set holds carry_in paths and MCP env"
        );
        assert!(
            !config::Layer::Personal.is_committed(),
            "omh config set writes your own file"
        );
        assert!(
            repo_layer(true).is_committed(),
            "and --shared is how you say you meant it"
        );
    }

    /// `omh <name>` treats any unknown word as a harness, so a command that is
    /// not in RESERVED could be shadowed by an adapter of the same name. This
    /// keeps the list honest without anyone remembering to update it.
    #[test]
    fn reserved_lists_every_command_and_alias() {
        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            assert!(
                RESERVED.contains(&name),
                "command `{name}` missing from RESERVED"
            );
            for alias in sub.get_visible_aliases() {
                assert!(
                    RESERVED.contains(&alias),
                    "alias `{alias}` missing from RESERVED"
                );
            }
        }
    }

    #[test]
    fn no_bundled_definition_shadows_a_command() {
        for a in Adapter::load_dir(std::path::Path::new(BUNDLED_ADAPTERS)).unwrap() {
            assert!(
                !RESERVED.contains(&a.name.as_str()),
                "adapter `{}` is a command",
                a.name
            );
        }
        for e in editor::Editor::load_dir(std::path::Path::new(BUNDLED_EDITORS)).unwrap() {
            assert!(
                !RESERVED.contains(&e.name.as_str()),
                "editor `{}` is a command",
                e.name
            );
        }
    }

    /// The grammar splits harnesses from editors, so the one mistake everybody
    /// will make is typing an editor where a harness goes. Say the fix.
    #[test]
    fn naming_an_editor_where_a_harness_goes_names_the_fix() {
        let hint = tool_hint("zed", &["claude".into()], &["zed".into()]);
        assert!(hint.contains("omh attach zed"), "got: {hint}");
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
    fn a_command_typed_as_a_harness_points_at_its_help() {
        let hint = tool_hint("config", &["claude".into()], &[]);
        assert!(hint.contains("omh config --help"), "got: {hint}");
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

        install_bundled(&dest, bundled::Shipped::Adapters).unwrap();

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

        install_bundled(&dest, bundled::Shipped::Adapters).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("claude.toml.yours")).unwrap(),
            mine,
            "the replaced file must be recoverable byte for byte"
        );
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

        install_bundled(&dest, bundled::Shipped::Adapters).unwrap();

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

        install_bundled(&dest, bundled::Shipped::Adapters).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("mine.toml")).unwrap(),
            "name = \"mine\"\n"
        );
    }

    /// Aliases only earn their keep if they are actually short.
    #[test]
    fn every_alias_is_a_single_letter() {
        for sub in Cli::command().get_subcommands() {
            for alias in sub.get_visible_aliases() {
                assert_eq!(alias.chars().count(), 1, "`{alias}` is not a shortcut");
            }
        }
    }

    // ── omh's flags versus the harness's ────────────────────────────────────

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// `omh opencode --dry-run` launched for real. Everything after the harness
    /// name is the harness's argv, so omh's own flag went to opencode and omh
    /// never saw it. Found by hand while investigating something else — and a
    /// flag whose entire meaning is "change nothing" is the worst one to
    /// swallow quietly.
    #[test]
    fn omhs_own_flag_after_the_harness_name_is_refused() {
        let err = passthrough(&argv(&["opencode", "--dry-run"]), &omh_globals()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--dry-run"), "name the flag: {msg}");
        assert!(
            msg.contains("omh --dry-run opencode"),
            "and show the form that works: {msg}"
        );
    }

    #[test]
    fn a_flag_the_harness_owns_passes_through_untouched() {
        let given = argv(&["claude", "--resume", "x"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// Short flags are deliberately left alone. `-s` is omh's session flag and
    /// is also a flag plenty of harnesses have; refusing it would break
    /// launches that work today to guard a mistake nobody has made. The long
    /// forms are the ones worth protecting — they are unlikely to collide and
    /// they are what people actually type.
    #[test]
    fn short_flags_belong_to_the_harness() {
        let given = argv(&["claude", "-s", "something"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// The escape hatch, for the day a harness really does have `--new`.
    /// Consumed on the way through, the way every tool that offers `--` does.
    #[test]
    fn a_double_dash_hands_the_rest_to_the_harness() {
        let out = passthrough(&argv(&["claude", "--", "--dry-run"]), &omh_globals()).unwrap();
        assert_eq!(out, argv(&["claude", "--dry-run"]));
    }

    /// The harness's own name is never a flag, and a session id that happens to
    /// look like one is not omh's business either.
    #[test]
    fn only_the_arguments_are_inspected_not_the_harness_name() {
        let given = argv(&["--dry-run"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// Derived from the parser rather than typed out, so a global added later
    /// inherits the guard instead of quietly falling outside it — the same
    /// reason `RESERVED` is checked against the subcommand list.
    #[test]
    fn every_global_flag_is_covered_without_anyone_listing_them() {
        let globals = omh_globals();
        let declared: Vec<String> = Cli::command()
            .get_arguments()
            .filter(|a| a.is_global_set())
            .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
            .collect();
        assert!(!declared.is_empty(), "the parser must have globals at all");
        for flag in declared {
            assert!(globals.contains(&flag), "{flag} is not guarded");
            assert!(
                passthrough(&argv(&["claude", &flag]), &globals).is_err(),
                "{flag} reaches the harness"
            );
        }
    }
}
