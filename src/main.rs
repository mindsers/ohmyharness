//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh claude          omh opencode          omh codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod config;
mod container;
mod detect;
mod image;
mod persist;
mod profile;
mod render;
mod runtime;
mod session;

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

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold `.omh/` in this repo.
    Init,
    /// Log a harness in once, capturing its credentials into a volume.
    Auth { harness: String },
    /// List sessions and the harnesses available.
    Ls,
    /// Show what a session changed, against a base branch.
    Diff {
        session: String,
        #[arg(long, default_value = "main")]
        base: String,
    },
    /// Remove a session's worktree. The branch is kept.
    Rm { session: String },
    /// Show effective settings and which layer each one comes from.
    Config {
        /// Limit to a section: `policy` or `mcp`.
        section: Option<String>,
    },
    /// Set a setting. Defaults to the gitignored layer so secrets cannot leak.
    Set {
        key: String,
        value: String,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Remove a setting from one layer, letting any lower layer resurface.
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
    /// Manage MCP servers.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Anything else is a harness name: `omh claude`, `omh opencode`.
    #[command(external_subcommand)]
    Run(Vec<String>),
}

#[derive(Subcommand)]
enum McpCmd {
    /// List servers with the layer each comes from.
    Ls,
    /// Add a server. Defaults to the gitignored layer, because MCP env holds tokens.
    Add {
        name: String,
        command: String,
        /// Arguments passed to the server command.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Remove a server from one layer, letting any lower layer resurface.
    Rm {
        name: String,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
    /// Import servers you already configured in an installed harness.
    Import {
        /// Harness to import from — determines both the format and where to look.
        harness: String,
        /// Override the file to read.
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        /// Overwrite servers that already exist with different settings.
        #[arg(long)]
        force: bool,
        #[arg(long, value_parser = parse_layer)]
        layer: Option<config::Layer>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    match &cli.cmd {
        Cmd::Init => init(&cwd),
        Cmd::Auth { harness } => auth(&cwd, harness),
        Cmd::Ls => ls(&cwd),
        Cmd::Diff { session, base } => diff(&cwd, session, base),
        Cmd::Rm { session } => rm(&cwd, session),
        Cmd::Config { section } => show_config(&cwd, section.as_deref()),
        Cmd::Set { key, value, layer } => set(&cwd, key, value, *layer),
        Cmd::Unset { key, layer } => unset(&cwd, key, *layer),
        Cmd::Edit { layer } => edit(&cwd, *layer),
        Cmd::Mcp { cmd } => mcp(&cwd, cmd, cli.dry_run),
        Cmd::Run(argv) => run(&cwd, argv, &cli),
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
    let harness = &argv[0];
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let profile = Profile::resolve(&paths);

    // A dry run must leave no trace: no branch, no worktree, no staged files.
    let opts = container::Options {
        // A dry run must leave no trace: no branch, no worktree, no staged files.
        staging: if cli.dry_run { container::Staging::Skip } else { container::Staging::Apply },
        persist: policy_value(&paths, "persistence")
            .as_deref()
            .unwrap_or("dtach")
            .parse()?,
    };

    std::fs::create_dir_all(paths.worktrees())?;
    let id = cli
        .session
        .clone()
        .unwrap_or_else(|| session::next_id(&paths.worktrees()));
    let session = Session::new(&paths.worktrees(), id);
    if opts.staging == container::Staging::Apply {
        session.ensure(&paths.repo)?;
    }

    let plan = container::plan(&paths, &profile, &adapter, &session, &argv[1..], opts)?;

    // The backend is pluggable so the opinion stays escapable, and a plan the
    // chosen runtime cannot honour must fail here rather than start a sandbox
    // with the profile silently missing.
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;
    if opts.staging == container::Staging::Apply {
        image::ensure(backend.program(), &adapter)?;
        image::ensure_network(backend.program(), &plan.network)?;
    }
    let args = backend.args(&plan);

    let status_line = match plan.degradation() {
        Some(d) => format!("omh: {} on {} — {d}", adapter.name, session.branch, d = d),
        None => format!("omh: {} on {}", adapter.name, session.branch),
    };

    if cli.dry_run {
        println!("{status_line}");
        println!("worktree {}", session.worktree.display());
        println!("\n{} {}", backend.program(), args.join(" \\\n       "));
        return Ok(());
    }

    eprintln!("{status_line}");
    let status = Command::new(backend.program()).args(&args).status()?;
    eprintln!("\nomh: review with  omh diff {}", session.id);
    std::process::exit(status.code().unwrap_or(1));
}

fn init(cwd: &std::path::Path) -> Result<()> {
    // 1. Fail fast. Everything below is wasted work outside a repo.
    let paths = Paths::discover(cwd)?;

    // 2. A fresh install has no adapters, so `omh <harness>` would fail no
    //    matter what else init did. Ship them before anything else.
    let adapters = install_bundled_adapters(&paths)?;
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
    println!("  adapters   {} ({})", adapters.len(), adapters.join(", "));
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
    let dir = paths.adapters();
    std::fs::create_dir_all(&dir)?;
    let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters");
    if let Ok(entries) = std::fs::read_dir(&bundled) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                write_if_absent(&dir.join(entry.file_name()), &std::fs::read_to_string(&path)?)?;
            }
        }
    }
    Ok(Adapter::load_dir(&dir)?.into_iter().map(|a| a.name).collect())
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

fn auth(cwd: &std::path::Path, harness: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let dest = paths.creds(&adapter.name);
    // Deliberately not implemented rather than faked: this runs the harness's
    // own login flow in a throwaway container and captures `adapter.creds` into
    // `dest`. It is v0 scope — if `omh claude` re-prompts for login, the whole
    // premise is dead.
    anyhow::bail!(
        "not implemented yet\n\
         plan: docker run --rm -it -v {}:{} omh/{} {} /login\n\
         capturing: {}",
        dest.display(),
        "/home/agent/.creds",
        adapter.name,
        adapter.bin,
        adapter.creds.join(", ")
    );
}

fn ls(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;

    println!("harnesses:");
    let adapters = Adapter::load_dir(&paths.adapters())?;
    if adapters.is_empty() {
        println!("  (none — add {}/<name>.toml)", paths.adapters().display());
    }
    for a in &adapters {
        let creds = if paths.is_authed(&a.name) { "authed" } else { "not authed" };
        println!("  {:<10} {}", a.name, creds);
    }

    println!("\nsessions:");
    let sessions = session::list(&paths.worktrees());
    if sessions.is_empty() {
        println!("  (none)");
    }
    for s in sessions {
        println!("  {s:<10} omh/{s}");
    }
    Ok(())
}

fn diff(cwd: &std::path::Path, id: &str, base: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());
    print!("{}", session.diff(&paths.repo, base)?);
    Ok(())
}

fn rm(cwd: &std::path::Path, id: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());
    session.remove(&paths.repo)?;
    println!("removed session {id}; branch omh/{id} kept");
    Ok(())
}
