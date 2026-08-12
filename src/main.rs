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
mod hook;
mod idle;
mod image;
mod mcp;
mod memory;
mod persist;
mod profile;
mod render;
mod rules;
mod runtime;
mod session;
mod settings;
mod ssh;
mod why;

use adapter::Adapter;
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use profile::{Paths, Profile};
use session::Session;
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
    #[arg(long, global = true)]
    new: bool,

    /// Which captured account to log in as.
    #[arg(long, short = 'a', global = true)]
    account: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

/// Built-ins and their aliases always beat a harness name — otherwise an
/// adapter called `s` or `config` would silently shadow a command.
pub const RESERVED: [&str; 15] = [
    "init", "doctor", "d", "auth", "ls", "attach", "a", "sessions", "s", "config", "c", "graph",
    "why", "memory", "help",
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
    /// Show settings with provenance, or change them.
    #[command(visible_alias = "c")]
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
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
    /// Remove a worktree. The branch is always kept.
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

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set a value. Defaults to the gitignored layer so secrets cannot leak.
    Set {
        key: String,
        value: String,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Remove a value from one layer, letting any lower layer resurface.
    Unset {
        key: String,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Open a layer's profile in $EDITOR.
    Edit {
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// MCP servers — configuration, so it lives here.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
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
            None => show_config(&cwd, None),
            Some(ConfigCmd::Set { key, value, layer }) => set(&cwd, key, value, *layer),
            Some(ConfigCmd::Unset { key, layer }) => unset(&cwd, key, *layer),
            Some(ConfigCmd::Edit { layer }) => edit(&cwd, *layer),
            Some(ConfigCmd::Mcp { cmd }) => mcp(&cwd, cmd, cli.dry_run),
        },

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

        Cmd::Run(argv) => run(&cwd, argv, &cli),
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

/// Bring a session's sandbox up if it is not already. A session is a *running
/// container*, not a launch — that is what lets an editor attach to the same
/// place the agent is working.
fn session_up(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    opts: container::Options,
) -> Result<(Box<dyn runtime::Runtime>, String)> {
    let backend = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))?;
    let name = paths.container(&session.id);
    if image::container_running(backend.program(), &name) {
        return Ok((backend, name));
    }

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
    let plan = container::plan(paths, profile, adapter, session, &[], opts)?;
    plan.validate(&backend.caps())?;
    say_rules(&plan);
    image::ensure(backend.program(), adapter)?;
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
            omh: omh_own(&paths)?,
        },
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
    let own = omh_own(&paths)?;
    let mut checks = doctor::checks(&profile, &adapter, &own);
    if account.is_some() {
        checks.extend(doctor::credential_checks(&adapter));
    }
    // Only if the resolved profile actually declares it: a check for a server
    // nobody configured would fail honestly and mean nothing.
    //
    // Read through `render::parse_layers` rather than `config::servers`, which
    // returns only each server's *command* — the arguments are what say which
    // directories it will look in, and those are the whole point of the check.
    let declared = render::parse_layers(&profile.sources(adapter::Capability::Mcp))?;
    // Not when this repo has switched the feature off: the server is left out
    // of the document on purpose, so checking for it is checking a claim omh
    // deliberately did not make.
    if let Some(server) = declared
        .get(memory::tools::SERVER_KEY)
        .filter(|_| !own.disabled_servers.contains(memory::tools::SERVER_KEY))
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
    };
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }
    let mut plan = container::plan(&paths, &profile, &adapter, &session, &[], opts)?;
    say_rules(&plan);
    plan.argv = vec!["sh".into(), "-c".into(), doctor::probe_script(&checks)];

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    if dry_run {
        println!("{}", doctor::probe_script(&checks));
        return Ok(());
    }

    image::ensure(backend.program(), &adapter)?;
    image::ensure_network(backend.program(), &plan.network)?;

    match &account {
        Some(a) => println!(
            "omh doctor: {name} (in {}, account {})\n",
            image::tag_for(&adapter),
            a.file_name().unwrap_or_default().to_string_lossy()
        ),
        None => println!(
            "omh doctor: {name} (in {}, no account — credentials unchecked)\n",
            image::tag_for(&adapter)
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
    Ok(())
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

    let seeds = detect::seeds(&paths.repo);
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
        McpCmd::Ls => show_config(cwd, Some("mcp")),

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

fn show_config(cwd: &std::path::Path, section: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let sections: Vec<(&str, Vec<config::Setting>)> = match section {
        Some("policy") => vec![("policy", config::policy(&paths)?)],
        Some("mcp") => vec![("mcp", config::servers(&paths)?)],
        None => vec![
            ("policy", config::policy(&paths)?),
            ("mcp", config::servers(&paths)?),
        ],
        Some(other) => anyhow::bail!("unknown section `{other}` — expected policy or mcp"),
    };

    for (name, settings) in sections {
        println!("{name}:");
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
            println!("  {:<16} {:<28} ← {}{shadowed}", s.key, s.value, s.layer);
        }
        println!();
    }
    Ok(())
}

fn set(cwd: &std::path::Path, key: &str, value: &str, layer: Option<config::Layer>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let w = config::set(
        &paths,
        key,
        value,
        layer.unwrap_or(config::Layer::DEFAULT_WRITE),
    )?;
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

fn unset(cwd: &std::path::Path, key: &str, layer: Option<config::Layer>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let layer = layer.unwrap_or(config::Layer::DEFAULT_WRITE);
    if config::unset(&paths, key, layer)? {
        println!("removed {key} from the {layer} layer");
    } else {
        println!("{key} was not set in the {layer} layer");
    }
    Ok(())
}

fn edit(cwd: &std::path::Path, layer: Option<config::Layer>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let layer = layer.unwrap_or(config::Layer::DEFAULT_WRITE);
    let file = layer.file(&paths);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    Command::new(editor).arg(&file).status()?;
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
                 \x20 has it already. Not carried; remove it with `omh config edit`.",
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
        omh: omh_own(&paths)?,
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

    // The session is a running container many harnesses take turns inhabiting.
    // Exec into it rather than starting a throwaway, so switching harness is
    // instant, MCP daemons stay warm, and `omh code` has something to attach to.
    let (backend, name) = session_up(
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            tty: false,
            ..opts.clone()
        },
    )?;
    eprintln!("{status_line}");
    let status = Command::new(backend.program())
        .args(backend.exec_args(&name, &plan.argv, true))
        .status()?;
    eprintln!("\nomh: review with  omh diff {}", session.id);
    std::process::exit(status.code().unwrap_or(1));
}

/// What omh contributes to a session here: the hooks and rules sections the
/// base manifest generates, minus the features this repo switched off.
///
/// Resolved by the caller of `container::plan` rather than inside it, the rule
/// `memory_bin` and `base` already follow — the manifest is a file, and a probe
/// inside `plan` is a probe no test can reach.
fn omh_own(paths: &Paths) -> Result<base::Own> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let settings = settings::resolve(paths, &manifest)?;
    // What the catalogue still declares, so removing a server takes its feature
    // with it. `omh config mcp rm codegraph` edits `mcp.json` and nothing
    // else, so this read is where that instruction is kept or broken.
    let installed = config::servers(paths)?.into_iter().map(|s| s.key).collect();
    let mut own = base::own(&manifest, &settings.off, &installed)?;
    own.mcp_env = settings.mcp_env;
    Ok(own)
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
    for s in detect::stacks(&paths.repo) {
        let from = format!("{}, detected from {}", s.name, s.marker);
        for (suffix, command) in [("test", s.test), ("format", s.format)] {
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

    // Detect rather than ask.
    let stacks = detect::stacks(&paths.repo);
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
    for stack in &stacks {
        for (name, body) in stack_hooks(stack) {
            write_if_absent(&repo_omh.join("hooks").join(name), &body)?;
        }
    }
    // Appended, not overwritten: re-running init must not eat a line you added.
    let gitignore = paths.repo.join(".omh/.gitignore");
    // Left tracked, a machine-local override gets committed to the team's repo.
    ensure_line(&gitignore, settings::LOCAL)?;

    // Report every decision, so `omh why` has something to explain. Printed as
    // each one is made rather than collected for the end, which is why the
    // image and graph lines below appear inside the summary.
    println!("omh init — decided, asked nothing\n");
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
            "\n  ! {} stacks detected; hooks were written for all of them.\n    \
             drop the ones you do not want: .omh/hooks/",
            stacks.len()
        );
    }

    println!("\n  catalogue  {}", paths.root.display());
    println!("  this repo  {}  (committed)", repo_omh.display());
    // The image. Without it the headline command cannot run, so init is not
    // finished until this exists.
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
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
    }

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

/// The hooks a detected stack gets: run the tests at turn end, format on edit.
///
/// Files rather than prose. A sentence in the rules describing `cargo test` is
/// a sentence; a hook is the thing that runs it — and once written it is the
/// repo's, editable and committed, so a teammate cloning gets the project's
/// test command with the project.
///
/// Extracted from `init` so the commands can be asserted without a container.
/// The guard that used to cover them read the generated `AGENTS.md`, which no
/// longer exists.
fn stack_hooks(stack: &detect::Stack) -> [(String, String); 2] {
    [
        (
            format!("{}-test.json", stack.name),
            format!("{{ \"on\": \"turn-end\", \"run\": \"{}\" }}\n", stack.test),
        ),
        (
            format!("{}-format.json", stack.name),
            format!(
                "{{ \"on\": \"after-tool\", \"tools\": [\"edit\"], \"run\": \"{}\" }}\n",
                stack.format
            ),
        ),
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
            omh: omh_own(&paths)?,
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
        println!("  choose per project with `omh config set account <name>`");
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
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        let name = paths.container(id);
        if image::container_running(backend.program(), &name) {
            let project = base::project_name(&paths.repo_name(), id);
            let _ = Command::new(backend.program())
                .args(backend.exec_args(&name, &base::drop_graph_command(&project), false))
                .output();
        }
    }

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

    /// `omh_own` is the wiring between the manifest and every launch, and
    /// nothing reached it: replacing its body with `Ok(base::Own::default())`
    /// — omh contributing no hooks, no rules sections, nothing — left the
    /// whole suite green.
    ///
    /// That is the failure `tests/cli.rs` says in its own module doc it exists
    /// to notice: a guard correct while the wiring that reaches it is missing.
    /// `container` and `doctor` each build an `Own` in their fixtures, so they
    /// prove a plan handles one and say nothing about whether one arrives.
    #[test]
    fn omh_own_resolves_the_manifest_and_reads_this_repos_settings() {
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
        // The servers a profile declares decide whether a feature was removed
        // rather than merely switched off, so the fixture has to declare them.
        write(
            paths.repo.join(".omh/profile/mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"},"memory":{"command":"omh"}}}"#,
        );

        let own = omh_own(&paths).unwrap();
        assert!(
            !own.hooks.is_empty() && !own.sections.is_empty(),
            "a launch must be given what the manifest ships"
        );

        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n",
        );
        let off = omh_own(&paths).unwrap();
        assert!(
            !off.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "and `[omh]` in this repo has to reach it: {:?}",
            off.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );
        assert!(
            off.hooks.iter().any(|h| h.name == "git-unavailable"),
            "without taking a different feature with it"
        );
    }

    /// A detected stack earns hooks, not prose. The guard that used to cover
    /// this read the generated `AGENTS.md` — a sentence saying `cargo test`
    /// runs nothing, and once that file stopped being written the commands
    /// were asserted nowhere.
    #[test]
    fn a_detected_stack_gets_a_hook_that_runs_its_commands() {
        for stack in detect::stacks(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))) {
            let hooks = stack_hooks(&stack);
            let by = |suffix: &str| {
                hooks
                    .iter()
                    .find(|(name, _)| name.ends_with(suffix))
                    .map(|(_, body)| body.clone())
                    .unwrap_or_else(|| panic!("{} has no {suffix}", stack.name))
            };

            // omh's words, not a harness's. A seeded file spelling `Stop`
            // would work on exactly one harness, and the file `init` writes is
            // the example every hand-written one is copied from.
            let test = by("-test.json");
            assert!(test.contains(stack.test), "must run the tests: {test}");
            assert!(test.contains("\"turn-end\""), "at turn end: {test}");

            let format = by("-format.json");
            assert!(
                format.contains(stack.format),
                "must format the code: {format}"
            );
            assert!(
                format.contains("\"edit\""),
                "when a file is written: {format}"
            );

            for (_, body) in hooks {
                serde_json::from_str::<serde_json::Value>(&body)
                    .unwrap_or_else(|e| panic!("{}: {e} in {body}", stack.name));
            }
        }
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
}
