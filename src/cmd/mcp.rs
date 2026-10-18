//! `omh settings mcp` — the servers in your catalogue.
//!
//! An MCP server is declared once, here, and rendered into whatever shape
//! each harness reads. `crate::mcp` and `crate::render` do that; this is the
//! command surface over it.

use crate::adapter::{self, Adapter};
use crate::cli::McpCmd;
use crate::out;
use crate::profile::Paths;
use crate::{config, render, report, settings};
use anyhow::{Context, Result};

/// Takes the `Paths` its caller resolved rather than resolving its own.
///
/// It called `Paths::discover`, which refuses outside a repository — so the
/// arm above it went to the trouble of `Paths::anywhere` and then handed down
/// a `cwd` that threw the answer away. `omh settings mcp ls` reads your
/// catalogue and nothing else; it refused to list it in a directory that
/// happened not to be a checkout.
pub(crate) fn mcp(paths: &Paths, cmd: &McpCmd, dry_run: bool, ctx: &out::Ctx) -> Result<()> {
    match cmd {
        McpCmd::Ls => show_servers(paths, ctx),

        McpCmd::Add {
            name,
            command,
            args,
            env,
            replace,
        } => {
            // omh's own: `omh upgrade` puts its definition back over yours,
            // and `rm` refuses it for the same reason.
            if let Some(feature) = crate::cmd::settings::shipped_servers()?.get(name) {
                anyhow::bail!(
                    "the `{name}` server is omh's own — the {feature} feature, and \
                     `omh upgrade` puts its definition back. To switch it off in a repo:\n  \
                     omh set {feature} off\n\
                     and for every repo `omh init` sets up after:\n  \
                     omh settings set {feature} off"
                );
            }
            // Written over in silence, a second `linear` took the first one's
            // arguments and its token with it.
            let held = config::held_server(paths, name)?;
            if let (Some(held), false) = (&held, replace) {
                anyhow::bail!(
                    "your catalogue already has a `{name}` server — {}. To write over it:\n  \
                     omh settings mcp add --replace {name} <command> [args…]",
                    described(held)
                );
            }
            let server = render::Server {
                command: command.clone(),
                args: args.clone(),
                env: env.iter().cloned().collect(),
            };
            // The server is built and the destination resolved either way;
            // only the write is withheld. `mcp_import` in the same enum has
            // read `dry_run` since it was written, and these two did not.
            if dry_run {
                let verb = match &held {
                    Some(held) => format!("would replace {name} ({})", described(held)),
                    None => format!("would add {name}"),
                };
                ctx.say(&report::Action::new(
                    "mcp-planned",
                    format!("{verb} → {}", config::mcp_path(paths).display()),
                ));
                return Ok(());
            }
            let w = config::mcp_add(paths, name, server)?;
            let mut action =
                report::Action::new("mcp-added", format!("wrote → {}", w.path.display())).data(
                    serde_json::json!({ "server": name, "path": w.path.display().to_string() }),
                );
            if !env.is_empty() {
                // The catalogue is not committed, so nothing here reaches a
                // teammate — but it does reach every repo you work in, which is
                // the wrong scope for a token scoped to one of them.
                action = action.note(format!(
                    "this env applies in every repo. For one repo only, put \
                     [mcp.{name}.env] in .omh/{}",
                    settings::LOCAL
                ));
            }
            ctx.say(&action);
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
                        format!("adapter {harness} does not say where to import from; pass --from")
                    })?;
                    adapter::expand_host(template, &home, &paths.repo)
                }
            };

            let raw = std::fs::read_to_string(&source).with_context(|| {
                format!(
                    "reading {} — pass --from to point somewhere else",
                    source.display()
                )
            })?;
            let incoming = render::parse(binding.render, &raw)?;

            let outcome = config::mcp_import(paths, incoming, *force, dry_run)?;
            let wrote = (!dry_run && !outcome.added.is_empty())
                .then(|| config::mcp_path(paths).display().to_string());

            let considered = outcome
                .added
                .iter()
                .map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Took,
                    detail: String::new(),
                })
                .chain(outcome.unchanged.iter().map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Kept,
                    detail: "already identical".into(),
                }))
                .chain(outcome.conflicts.iter().map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Conflict,
                    detail: "differs — keeping yours; --replace to overwrite".into(),
                }))
                .collect();

            ctx.say(&report::Imported {
                what: harness.clone(),
                source: source.display().to_string(),
                considered,
                noun: "servers".into(),
                dry_run,
                wrote,
                selected_in: Vec::new(),
            });
            Ok(())
        }
    }
}

/// Does this repo already say what it uses?
///
/// Read from the committed file directly rather than through
/// `settings::resolve`, which merges the gitignored layer over it: a `[use]`
/// in `settings.local.toml` is one person's override and is not this repo
/// having decided anything, so treating it as one would leave a fresh checkout
/// with no list of its own.
pub(crate) fn repo_has_selection(paths: &Paths) -> Result<bool> {
    // Through `config`, which distinguishes absent from unreadable. Reading the
    // file here with `let Ok(..) else { return Ok(false) }` reintroduced the
    // exact conflation `config::read_layer` was written about, in the one place
    // where the answer decides whether `init` overwrites a curated list — and
    // it was a third parse strategy for a file that already had two.
    config::declares(paths, config::Layer::Shared, config::USE)
}

/// The catalogue's MCP servers, with whose each one is.
pub(crate) fn show_servers(paths: &Paths, ctx: &out::Ctx) -> Result<()> {
    ctx.say(&report::Servers {
        servers: config::servers(paths)?
            .into_iter()
            .map(|s| report::Setting {
                key: s.key,
                value: s.value,
                whose: Some(s.layer.whose().to_string()),
            })
            .collect(),
    });
    Ok(())
}

/// A server in words a screen can carry: what it runs, its arguments counted
/// and its environment named — never a value of either, which is where a
/// token goes.
fn described(server: &render::Server) -> String {
    let mut out = format!("runs {}", server.command);
    if !server.args.is_empty() {
        out.push_str(&format!(
            ", with {} argument{}",
            server.args.len(),
            crate::cmd::harvest::plural(server.args.len())
        ));
    }
    if !server.env.is_empty() {
        out.push_str(&format!(
            ", env {}",
            server.env.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    out
}
