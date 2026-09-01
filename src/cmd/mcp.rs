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
        } => {
            let server = render::Server {
                command: command.clone(),
                args: args.clone(),
                env: env.iter().cloned().collect(),
            };
            // The server is built and the destination resolved either way;
            // only the write is withheld. `mcp_import` in the same enum has
            // read `dry_run` since it was written, and these two did not.
            if dry_run {
                ctx.say(&report::Action::new(
                    "mcp-planned",
                    format!("would add {name} → {}", config::mcp_path(paths).display()),
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

        McpCmd::Rm { name } => {
            // **Refused, not reported.** A name that was not there is a typo
            // far more often than a plan, and this arm used to exit 0 saying so
            // — the one command in the group that did. `use`, `unuse`,
            // `memory rm` and `memory promote` all refuse the same mistake, and
            // a script cannot tell a removal from a misspelling when both come
            // back green.
            if dry_run {
                let known = config::servers(paths)?;
                anyhow::ensure!(
                    known.iter().any(|s| s.key == *name),
                    "no server `{name}` in your catalogue."
                );
                ctx.say(&report::Action::new(
                    "mcp-planned",
                    format!("would remove {name} from your catalogue"),
                ));
                return Ok(());
            }
            if !config::mcp_remove(paths, name)? {
                let known = config::servers(paths)?
                    .into_iter()
                    .map(|s| s.key)
                    .collect::<Vec<_>>();
                anyhow::bail!(
                    "no server `{name}` in your catalogue.\n  servers: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                );
            }
            ctx.say(
                &report::Action::new("mcp-removed", format!("removed {name} from your catalogue"))
                    .data(serde_json::json!({ "server": name, "removed": true })),
            );
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
                    detail: "differs — keeping yours; --force to overwrite".into(),
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
