//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh claude          omh opencode          omh codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod container;
mod mcp;
mod profile;
mod session;

use adapter::Adapter;
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
    /// Anything else is a harness name: `omh claude`, `omh opencode`.
    #[command(external_subcommand)]
    Run(Vec<String>),
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
        Cmd::Run(argv) => run(&cwd, argv, &cli),
    }
}

fn run(cwd: &std::path::Path, argv: &[String], cli: &Cli) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let harness = &argv[0];
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let profile = Profile::resolve(&paths);

    std::fs::create_dir_all(paths.worktrees())?;
    let id = cli
        .session
        .clone()
        .unwrap_or_else(|| session::next_id(&paths.worktrees()));
    let session = Session::new(&paths.worktrees(), id);
    session.ensure(&paths.repo)?;

    let plan = container::plan(&paths, &profile, &adapter, &session, &argv[1..])?;
    let args = plan.docker_args();

    if cli.dry_run {
        println!("session  {}  (branch {})", session.id, session.branch);
        println!("worktree {}", session.worktree.display());
        println!("\ndocker {}", args.join(" \\\n       "));
        return Ok(());
    }

    eprintln!("omh: {} on {}", adapter.name, session.branch);
    let status = Command::new("docker").args(&args).status()?;
    eprintln!("\nomh: review with  omh diff {}", session.id);
    std::process::exit(status.code().unwrap_or(1));
}

fn init(cwd: &std::path::Path) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let dir = paths.repo.join(".omh/profile");
    std::fs::create_dir_all(dir.join("skills"))?;
    std::fs::create_dir_all(paths.worktrees())?;

    let agents = dir.join("AGENTS.md");
    if !agents.exists() {
        std::fs::write(&agents, "# Project rules\n\n<!-- read by every harness -->\n")?;
    }
    let mcp = dir.join("mcp.json");
    if !mcp.exists() {
        std::fs::write(&mcp, "{\n  \"mcpServers\": {}\n}\n")?;
    }
    let ignore = paths.repo.join(".omh/.gitignore");
    std::fs::write(&ignore, "worktrees/\n")?;

    println!("initialized {}", dir.display());
    println!("next: omh auth claude   then   omh claude");
    Ok(())
}

fn auth(cwd: &std::path::Path, harness: &str) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let dest = paths.creds(&adapter.name);
    std::fs::create_dir_all(&dest)?;
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
        let creds = if paths.creds(&a.name).exists() { "authed" } else { "not authed" };
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
