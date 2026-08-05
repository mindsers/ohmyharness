//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh claude          omh opencode          omh codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod auth;
mod config;
mod container;
mod detect;
mod doctor;
mod editor;
mod image;
mod persist;
mod profile;
mod render;
mod runtime;
mod session;
mod ssh;

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

    /// Branch a new session from this ref instead of the default branch.
    #[arg(long, global = true)]
    from: Option<String>,

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
pub const RESERVED: [&str; 12] = [
    "init", "doctor", "d", "auth", "ls", "attach", "a", "sessions", "s", "config", "c", "help",
];

#[derive(Subcommand)]
enum Cmd {
    /// Set this repo up. Decides everything; asks nothing.
    Init,
    /// Verify a harness actually sees the profile, inside a real sandbox.
    #[command(visible_alias = "d")]
    Doctor { harness: Option<String> },
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
    /// Anything else is a harness: `omh claude`, `omh opencode`.
    #[command(external_subcommand)]
    Run(Vec<String>),
}

#[derive(Subcommand)]
enum McpCmd {
    /// Servers, with the layer each comes from.
    Ls,
    /// Add a server. Defaults to the gitignored layer, because MCP env holds tokens.
    Add {
        name: String,
        command: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Remove a server from one layer.
    Rm {
        name: String,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Import servers you already configured in an installed harness.
    Import {
        harness: String,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
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
        session: String,
        /// Defaults to the repo's own default branch.
        #[arg(long)]
        base: Option<String>,
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
        Cmd::Attach { editor } => attach(&cwd, cli.session.as_deref(), editor.as_deref()),

        Cmd::Sessions { cmd } => match cmd {
            SessionsCmd::Ls => sessions_ls(&cwd),
            SessionsCmd::Rm { session } => rm(&cwd, session),
            SessionsCmd::Down { session } => down(&cwd, session.as_deref()),
            SessionsCmd::Diff { session, base } => diff(&cwd, session, base.as_deref()),
        },

        Cmd::Config { cmd } => match cmd {
            None => show_config(&cwd, None),
            Some(ConfigCmd::Set { key, value, layer }) => set(&cwd, key, value, *layer),
            Some(ConfigCmd::Unset { key, layer }) => unset(&cwd, key, *layer),
            Some(ConfigCmd::Edit { layer }) => edit(&cwd, *layer),
            Some(ConfigCmd::Mcp { cmd }) => mcp(&cwd, cmd, cli.dry_run),
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
    format!("unknown harness `{name}`\n  available: {}", harnesses.join(", "))
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
) -> Result<(Box<dyn runtime::Runtime>, String)> {
    let backend = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))?;
    let name = paths.container(&session.id);
    if image::container_running(backend.program(), &name) {
        return Ok((backend, name));
    }

    let plan = container::plan(
        paths,
        profile,
        adapter,
        session,
        &[],
        container::Options { tty: false, ..Default::default() },
    )?;
    plan.validate(&backend.caps())?;
    image::ensure(backend.program(), adapter)?;
    image::ensure_network(backend.program(), &plan.network)?;

    let key = ssh::ensure_key(&paths.keys())?;
    let pubkey = std::fs::read_to_string(key.with_extension("pub"))?;
    let port = ssh::port(&paths.repo_name(), &session.id);

    image::container_remove(backend.program(), &name); // a stopped one blocks --name
    let args = backend.up_args(&plan, &name, port, pubkey.trim());
    let out = Command::new(backend.program()).args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "starting session {}: {}",
            session.id,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok((backend, name))
}

fn attach(cwd: &std::path::Path, id: Option<&str>, chosen: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let names: Vec<String> =
        Adapter::load_dir(&paths.adapters())?.into_iter().map(|a| a.name).collect();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
        .context("no adapters installed — run `omh init`")?;
    let adapter = Adapter::find(&paths.adapters(), &harness)?;

    std::fs::create_dir_all(paths.worktrees())?;
    let id = session::pick(&paths.worktrees(), id, false);
    let session = Session::new(&paths.worktrees(), id);
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;
    session_up(&paths, &profile, &adapter, &session)?;

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
            let base = std::path::Path::new(&e).file_name()?.to_string_lossy().into_owned();
            Some(base)
        });
    let wanted = chosen.map(str::to_string).or(fallback);
    let ed = wanted.as_deref().and_then(|n| editor::Editor::find(&paths.editors(), n));

    match ed {
        // An editor that is not installed is not an error — the URL is still a
        // good answer, and launching nothing silently would not be.
        Some(ed) if runtime::installed(&ed.bin) => {
            let cmd = ed.command(&alias);
            println!("omh: opening {} in {}", ssh::url(&alias), ed.name);
            Command::new(&cmd[0]).args(&cmd[1..]).status()?;
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
            image::container_remove(backend.program(), &name);
            println!("stopped {i}; worktree and branch survive");
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
            let names: Vec<String> =
                Adapter::load_dir(&paths.adapters())?.into_iter().map(|a| a.name).collect();
            detect::preferred_harness(&names, &|h| runtime::installed(h))
                .context("no adapters installed — run `omh init`")?
        }
    };
    let adapter = Adapter::find(&paths.adapters(), &name)?;

    let checks = doctor::checks(&profile, &adapter);
    if checks.is_empty() {
        println!("nothing to check: the profile is empty");
        return Ok(());
    }

    std::fs::create_dir_all(paths.worktrees())?;
    let session = Session::new(&paths.worktrees(), "doctor".into());
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;

    let opts = container::Options {
        staging: container::Staging::Apply,
        // No dtach and no terminal: the probe's output has to be captured.
        persist: persist::Mode::None,
        tty: false,
        account_dir: None,
    };
    let mut plan = container::plan(&paths, &profile, &adapter, &session, &[], opts)?;
    plan.argv = vec!["sh".into(), "-c".into(), doctor::probe_script(&checks)];

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    if dry_run {
        println!("{}", doctor::probe_script(&checks));
        return Ok(());
    }

    image::ensure(backend.program(), &adapter)?;
    image::ensure_network(backend.program(), &plan.network)?;

    println!("omh doctor: {name} (in {})\n", image::tag(&name));
    let out = Command::new(backend.program()).args(backend.args(&plan)).output()?;
    let outcomes = doctor::parse(&String::from_utf8_lossy(&out.stdout));
    let _ = session.remove(&paths.repo); // diagnostic: leave no session behind

    for o in &outcomes {
        println!("  {} {:<10} {}", if o.ok { "\u{2713}" } else { "\u{2717}" }, o.name, o.detail);
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
    println!("\n  all {} checks passed — {name}'s adapter paths are verified", outcomes.len());
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
            "  {id:<8} {:<14} {}{drift}",
            sess.branch,
            if up { "up" } else { "stopped" }
        );
    }
    Ok(())
}

/// Read one policy key through the usual layer merge.
fn policy_value(paths: &Paths, key: &str) -> Option<String> {
    config::policy(paths).ok()?.into_iter().find(|s| s.key == key).map(|s| s.value)
}

fn runtime_preference(paths: &Paths) -> String {
    policy_value(paths, "runtime").unwrap_or_else(|| "auto".into())
}

fn parse_layer(s: &str) -> std::result::Result<config::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
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

        McpCmd::Add { name, command, args, env, layer } => {
            let server = render::Server {
                command: command.clone(),
                args: args.clone(),
                env: env.iter().cloned().collect(),
            };
            let w = config::mcp_add(
                &paths,
                layer.unwrap_or(config::Layer::DEFAULT_WRITE),
                name,
                server,
            )?;
            println!("wrote → {}", w.path.display());
            if w.committed {
                println!(
                    "warning: the {} layer is COMMITTED — MCP env often holds tokens",
                    w.layer
                );
            }
            Ok(())
        }

        McpCmd::Rm { name, layer } => {
            let layer = layer.unwrap_or(config::Layer::DEFAULT_WRITE);
            if config::mcp_remove(&paths, layer, name)? {
                println!("removed {name} from the {layer} layer");
            } else {
                println!("{name} was not set in the {layer} layer");
            }
            Ok(())
        }

        McpCmd::Import { harness, file, force, layer } => {
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
                format!("reading {} — pass --file to point somewhere else", source.display())
            })?;
            let incoming = render::parse(binding.render, &raw)?;

            let layer = layer.unwrap_or(config::Layer::DEFAULT_WRITE);
            let report = config::mcp_import(&paths, layer, incoming, *force, dry_run)?;

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
                println!("\nwrote → {}", layer.dir(&paths).join("mcp.json").display());
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

fn set(
    cwd: &std::path::Path,
    key: &str,
    value: &str,
    layer: Option<config::Layer>,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let w = config::set(&paths, key, value, layer.unwrap_or(config::Layer::DEFAULT_WRITE))?;
    println!("wrote → {}", w.path.display());
    if w.committed {
        // The one mistake git makes unrecoverable.
        println!("warning: the {} layer is COMMITTED — never put a secret here", w.layer);
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
    let dir = layer.dir(&paths);
    std::fs::create_dir_all(&dir)?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    Command::new(editor).arg(&dir).status()?;
    Ok(())
}

fn run(cwd: &std::path::Path, argv: &[String], cli: &Cli) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let name = &argv[0];

    let adapter = Adapter::find(&paths.adapters(), name)
        .map_err(|e| unknown_tool(&paths, name, e))?;
    let profile = Profile::resolve(&paths);

    // A dry run must leave no trace: no branch, no worktree, no staged files.
    // Which identity this session runs as. Ambiguity is an error rather than a
    // guess: silently using the wrong account is expensive and invisible.
    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, name, cli.account.as_deref(), configured.as_deref())?
        .map(|a| auth::dir(&paths, name, &a));

    let opts = container::Options {
        // A dry run must leave no trace: no branch, no worktree, no staged files.
        staging: if cli.dry_run { container::Staging::Skip } else { container::Staging::Apply },
        persist: policy_value(&paths, "persistence")
            .as_deref()
            .unwrap_or("dtach")
            .parse()?,
        tty: true,
        account_dir: account,
    };

    std::fs::create_dir_all(paths.worktrees())?;
    let id = session::pick(&paths.worktrees(), cli.session.as_deref(), cli.new);
    // Branch from the trunk, not from wherever HEAD happens to be — a session
    // started on a feature branch produces a diff against the wrong baseline.
    let base = cli
        .from
        .clone()
        .unwrap_or_else(|| session::default_branch(&paths.repo));
    let session = Session::new(&paths.worktrees(), id);
    if opts.staging == container::Staging::Apply {
        session.ensure(&paths.repo, &base)?;
    }

    let plan = container::plan(&paths, &profile, &adapter, &session, &argv[1..], opts)?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    let status_line = match plan.degradation() {
        Some(d) => format!("omh: {} on {} — {d}", adapter.name, session.branch),
        None => format!("omh: {} on {}", adapter.name, session.branch),
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
    let (backend, name) = session_up(&paths, &profile, &adapter, &session)?;
    eprintln!("{status_line}");
    let status = Command::new(backend.program())
        .args(backend.exec_args(&name, &plan.argv, true))
        .status()?;
    eprintln!("\nomh: review with  omh diff {}", session.id);
    std::process::exit(status.code().unwrap_or(1));
}

#[allow(dead_code)]
fn init(cwd: &std::path::Path) -> Result<()> {
    // 1. Fail fast. Everything below is wasted work outside a repo.
    let paths = Paths::discover(cwd)?;

    // 2. A fresh install has no adapters, so `omh <harness>` would fail no
    //    matter what else init did. Ship them before anything else.
    let adapters = install_bundled_adapters(&paths)?;
    let editors = install_bundled(&paths.editors(), "editors")?;
    std::fs::create_dir_all(paths.root.join("profile/skills"))?;
    std::fs::create_dir_all(paths.worktrees())?;

    // 3. Detect rather than ask.
    let stacks = detect::stacks(&paths.repo);
    let names: Vec<String> = adapters.iter().cloned().collect();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h));

    // 4. Write layer 2 from what was detected. Never overwrite a human's file.
    let shared = paths.repo.join(".omh/profile");
    let local = paths.repo.join(".omh/local");
    for dir in [&shared, &local] {
        std::fs::create_dir_all(dir.join("skills"))?;
    }
    std::fs::create_dir_all(shared.join("hooks"))?;
    write_if_absent(&shared.join("AGENTS.md"), &detect::agents_md(&stacks))?;
    write_if_absent(&shared.join("mcp.json"), "{\n  \"mcpServers\": {}\n}\n")?;
    write_if_absent(
        &shared.join("policy.toml"),
        "# Untracked files the worktree needs. This is the ONLY path by which a\n\
         # secret reaches the agent, so keep it short and explicit.\n\
         carry_in = []\n",
    )?;
    for stack in &stacks {
        write_if_absent(
            &shared.join("hooks").join(format!("{}-test.json", stack.name)),
            &format!(
                "{{ \"event\": \"Stop\", \"matcher\": \"\", \"command\": \"{}\" }}\n",
                stack.test
            ),
        )?;
        write_if_absent(
            &shared.join("hooks").join(format!("{}-format.json", stack.name)),
            &format!(
                "{{ \"event\": \"PostToolUse\", \"matcher\": \"Edit|Write\", \"command\": \"{}\" }}\n",
                stack.format
            ),
        )?;
    }
    // Appended, not overwritten: re-running init must not eat a line you added.
    ensure_line(&paths.repo.join(".omh/.gitignore"), "local/")?;

    // 5. Report every decision, so `omh why` has something to explain.
    println!("omh init — decided, asked nothing\n");
    println!("  harnesses  {} ({})", adapters.len(), adapters.join(", "));
    println!("  editors    {} ({})", editors.len(), editors.join(", "));
    match &harness {
        Some(h) => println!("  harness    {h}{}", if runtime::installed(h) {
            "  (found on your host)"
        } else {
            "  (default; nothing detected on host)"
        }),
        None => println!("  harness    none — no adapters available"),
    }
    if stacks.is_empty() {
        println!("  stack      none detected — add commands to .omh/profile/AGENTS.md");
    } else {
        for s in &stacks {
            println!("  stack      {} (from {}) → test `{}`, format `{}`",
                s.name, s.marker, s.test, s.format);
        }
    }

    let seeds = detect::seeds(&paths.repo);
    if seeds.is_empty() {
        println!("  memory     nothing to derive yet");
    } else {
        println!("  memory     seeded from {} source{}:", seeds.len(),
            if seeds.len() == 1 { "" } else { "s" });
        for seed in &seeds {
            println!("               {:<12} {}", seed.source, seed.fact);
        }
    }

    // Derive, then confirm: a hypothesis worth correcting is not a questionnaire.
    if stacks.len() > 1 {
        println!(
            "\n  ! {} stacks detected; hooks were written for all of them.\n    \
             drop the ones you do not want: .omh/profile/hooks/",
            stacks.len()
        );
    }

    println!("\n  layers     {}  (committed)", shared.display());
    println!("             {}  (gitignored)", local.display());
    // 5. The image. Without it the headline command cannot run, so init is not
    //    finished until this exists.
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        if image::exists(backend.program(), &image::tag(h)) {
            println!("  image      {} (already built)", image::tag(h));
        } else {
            println!("\n  building {} — first run only\n", image::tag(h));
            image::ensure(backend.program(), &adapter)?;
            println!("\n  image      {}", image::tag(h));
        }
    }

    println!("\nnot yet done: code graph, memory store.");
    println!("next: omh {}", harness.as_deref().unwrap_or("config"));
    Ok(())
}

/// Adapters ship with omh but live in `~/.omh`. Without this a fresh install
/// cannot launch anything, which is the state the tool was in until now.
fn install_bundled_adapters(paths: &Paths) -> Result<Vec<String>> {
    install_bundled(&paths.adapters(), "adapters")?;
    Ok(Adapter::load_dir(&paths.adapters())?.into_iter().map(|a| a.name).collect())
}

/// Copy definitions that ship with omh into `~/.omh`.
///
/// Bundled files are **managed**: they are refreshed on every `init`, because a
/// fix omh ships has to reach people who already ran it once. The one that
/// mattered was a wrong credential path, which made `omh auth` capture nothing
/// while reporting success. Definitions you add yourself are left alone.
fn install_bundled(dest: &std::path::Path, kind: &str) -> Result<Vec<String>> {
    std::fs::create_dir_all(dest)?;
    let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(kind);
    if let Ok(entries) = std::fs::read_dir(&bundled) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                std::fs::write(dest.join(entry.file_name()), std::fs::read_to_string(&path)?)?;
            }
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(dest)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
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

    let account_dir = auth::dir(&paths, harness, account);
    let already = auth::is_captured(&paths, harness, account);
    auth::prepare(&adapter, &account_dir, "/home/agent")?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    image::ensure(backend.program(), &adapter)?;

    // A throwaway session: logging in must not create a branch or a worktree.
    std::fs::create_dir_all(paths.worktrees())?;
    let session = Session::new(&paths.worktrees(), "auth".into());
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;

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
    let status = Command::new(backend.program()).args(backend.args(&plan)).status()?;
    let _ = session.remove(&paths.repo);

    // "Something was written" is too weak: a harness writes its default config
    // just by starting, so an aborted login leaves a plausible-looking account
    // with no token in it.
    let unfilled = auth::unfilled(&adapter, &account_dir, "/home/agent");
    if !unfilled.is_empty() {
        anyhow::bail!(
            "the login did not complete — still empty:\n{}\n  \
             run `omh auth {harness} {account}` again and finish signing in",
            unfilled
                .iter()
                .map(|p| format!("    {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    println!("\nomh: `{account}` captured for {harness}");
    let all = auth::accounts(&paths, harness);
    if all.len() > 1 {
        println!("  accounts: {}", all.join(", "));
        println!("  choose per project with `omh config set account <name>`");
    }
    let _ = status;
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
        let accounts = auth::accounts(&paths, &a.name);
        let creds = if accounts.is_empty() { "not authed".to_string() } else { accounts.join(", ") };
        println!("  {:<10} {}", a.name, creds);
    }

    let editors = editor::Editor::load_dir(&paths.editors())?;
    if !editors.is_empty() {
        println!("\neditors:");
        for e in &editors {
            let state = if runtime::installed(&e.bin) { "installed" } else { "not installed" };
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
        println!("  {id:<10} {}{drift}", sess.branch);
    }
    Ok(())
}

fn diff(cwd: &std::path::Path, id: &str, base: Option<&str>) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| session::default_branch(&paths.repo));
    let out = session.diff(&paths.repo, &base)?;
    if out.trim().is_empty() {
        // Silence reads as breakage. Say which comparison came up empty.
        println!("no changes on {} (against {base})", session.branch);
    } else {
        print!("{out}");
    }
    Ok(())
}

fn rm(cwd: &std::path::Path, id: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());
    session.remove(&paths.repo)?;
    println!("removed session {id}; branch omh/{id} kept");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    const BUNDLED_ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BUNDLED_EDITORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/editors");

    /// `omh <name>` treats any unknown word as a harness, so a command that is
    /// not in RESERVED could be shadowed by an adapter of the same name. This
    /// keeps the list honest without anyone remembering to update it.
    #[test]
    fn reserved_lists_every_command_and_alias() {
        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            assert!(RESERVED.contains(&name), "command `{name}` missing from RESERVED");
            for alias in sub.get_visible_aliases() {
                assert!(RESERVED.contains(&alias), "alias `{alias}` missing from RESERVED");
            }
        }
    }

    #[test]
    fn no_bundled_definition_shadows_a_command() {
        for a in Adapter::load_dir(std::path::Path::new(BUNDLED_ADAPTERS)).unwrap() {
            assert!(!RESERVED.contains(&a.name.as_str()), "adapter `{}` is a command", a.name);
        }
        for e in editor::Editor::load_dir(std::path::Path::new(BUNDLED_EDITORS)).unwrap() {
            assert!(!RESERVED.contains(&e.name.as_str()), "editor `{}` is a command", e.name);
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
        let hint = tool_hint("emacs", &["claude".into(), "opencode".into()], &["zed".into()]);
        assert!(hint.contains("claude") && hint.contains("opencode"), "got: {hint}");
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

        install_bundled(&dest, "adapters").unwrap();

        let shipped = std::fs::read_to_string(
            std::path::Path::new(BUNDLED_ADAPTERS).join("claude.toml"),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("claude.toml")).unwrap(), shipped);
    }

    /// Definitions you add yourself are yours; omh only manages its own.
    #[test]
    fn definitions_omh_does_not_ship_are_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("mine.toml"), "name = \"mine\"\n").unwrap();

        install_bundled(&dest, "adapters").unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("mine.toml")).unwrap(), "name = \"mine\"\n");
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
