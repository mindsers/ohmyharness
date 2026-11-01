//! MCP servers as omh holds them: the catalogue's `mcp.json` shape, read
//! from it and from each harness's own file for `omh settings mcp import`.
//!
//! Apart from `render`, which writes them into a harness's file, so reading
//! a server list needs nothing from omh but the adapter's format names.

use crate::adapter::Render;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct CanonicalMcp {
    #[serde(rename = "mcpServers", default)]
    servers: BTreeMap<String, Server>,
}

/// Merge by server name; later layers win.
pub fn parse_layers(files: &[PathBuf]) -> Result<BTreeMap<String, Server>> {
    merge(files)
}

fn merge(files: &[PathBuf]) -> Result<BTreeMap<String, Server>> {
    let mut out = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let parsed: CanonicalMcp =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", f.display()))?;
        out.extend(parsed.servers);
    }
    Ok(out)
}

/// Inverse of `mcp`: read a harness's own config back into canonical form.
///
/// `omh settings mcp import` exists because nobody retypes MCP servers they
/// already have.
/// Every format that renders must also parse, and the pair must round-trip —
/// otherwise import silently drops fields the renderer knows how to write.
pub fn parse(format: Render, raw: &str) -> Result<BTreeMap<String, Server>> {
    match format {
        Render::McpJson => {
            let doc: serde_json::Value = serde_json::from_str(raw).context("parsing MCP JSON")?;
            if doc.get("mcpServers").is_none() && doc.get("projects").is_some() {
                anyhow::bail!(
                    "this config nests servers under `projects` — importing all of \
                     them would pull in servers from unrelated repos. Point --from \
                     at a project-scoped .mcp.json instead."
                );
            }
            let doc: CanonicalMcp = serde_json::from_value(doc).context("reading mcpServers")?;
            Ok(doc.servers)
        }

        Render::CodexToml => {
            #[derive(Deserialize)]
            struct Doc {
                #[serde(default, rename = "mcp_servers")]
                servers: BTreeMap<String, Server>,
            }
            let doc: Doc = toml::from_str(raw).context("parsing codex config.toml")?;
            Ok(doc.servers)
        }

        Render::OpencodeJson => {
            #[derive(Deserialize)]
            struct Doc {
                #[serde(default)]
                mcp: BTreeMap<String, Entry>,
            }
            #[derive(Deserialize)]
            struct Entry {
                #[serde(default)]
                command: Vec<String>,
                #[serde(default)]
                environment: BTreeMap<String, String>,
            }
            let doc: Doc = serde_json::from_str(raw).context("parsing opencode.json")?;
            doc.mcp
                .into_iter()
                .map(|(name, e)| {
                    // opencode folds command and args into one array; split the
                    // head back off or every arg stays glued to the command.
                    let (command, args) = e
                        .command
                        .split_first()
                        .with_context(|| format!("server `{name}` has an empty command"))?;
                    Ok((
                        name,
                        Server {
                            command: command.clone(),
                            args: args.to_vec(),
                            env: e.environment,
                        },
                    ))
                })
                .collect()
        }

        other => anyhow::bail!("`{other:?}` is not an MCP format and cannot be imported"),
    }
}

/// What an import found in a harness's file: the servers omh can run, and
/// each one it cannot, with why.
#[derive(Debug, Default)]
pub struct Foreign {
    pub servers: BTreeMap<String, Server>,
    pub left: BTreeMap<String, String>,
}

/// [`parse`], for `omh settings mcp import`: each server read on its own, so
/// one omh cannot take is left — named, with why — and never fails the rest.
/// A remote server, an `url` and no `command`, failed the whole import with
/// "missing field `command`"; and one the harness has switched off was
/// imported on, into every session.
pub fn parse_foreign(format: Render, raw: &str) -> Result<Foreign> {
    const REMOTE: &str = "a remote server — omh's catalogue holds programs it runs";
    const NO_COMMAND: &str = "no command to run";
    let off = |harness: &str| format!("switched off in {harness} — `enabled = false`");
    let mut out = Foreign::default();
    match format {
        Render::McpJson => {
            let doc: serde_json::Value = serde_json::from_str(raw).context("parsing MCP JSON")?;
            if doc.get("mcpServers").is_none() && doc.get("projects").is_some() {
                // `parse`'s refusal, for the same reason.
                parse(format, raw)?;
            }
            let empty = serde_json::Map::new();
            let entries = doc
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
                .unwrap_or(&empty);
            for (name, entry) in entries {
                if entry.get("command").is_none() {
                    let remote = entry.get("url").is_some()
                        || matches!(
                            entry.get("type").and_then(serde_json::Value::as_str),
                            Some("http" | "sse")
                        );
                    out.left.insert(
                        name.clone(),
                        (if remote { REMOTE } else { NO_COMMAND }).into(),
                    );
                    continue;
                }
                let server: Server = serde_json::from_value(entry.clone())
                    .with_context(|| format!("reading server `{name}`"))?;
                out.servers.insert(name.clone(), server);
            }
        }
        Render::CodexToml => {
            let doc: toml::Table = toml::from_str(raw).context("parsing codex config.toml")?;
            let empty = toml::Table::new();
            let entries = doc
                .get("mcp_servers")
                .and_then(toml::Value::as_table)
                .unwrap_or(&empty);
            for (name, entry) in entries {
                if entry.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
                    out.left.insert(name.clone(), off("codex"));
                    continue;
                }
                if entry.get("command").is_none() {
                    let remote = entry.get("url").is_some();
                    out.left.insert(
                        name.clone(),
                        (if remote { REMOTE } else { NO_COMMAND }).into(),
                    );
                    continue;
                }
                let server: Server = entry
                    .clone()
                    .try_into()
                    .with_context(|| format!("reading server `{name}`"))?;
                out.servers.insert(name.clone(), server);
            }
        }
        Render::OpencodeJson => {
            let doc: serde_json::Value =
                serde_json::from_str(raw).context("parsing opencode.json")?;
            let empty = serde_json::Map::new();
            let entries = doc
                .get("mcp")
                .and_then(serde_json::Value::as_object)
                .unwrap_or(&empty);
            for (name, entry) in entries {
                if entry.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
                    out.left.insert(name.clone(), off("opencode"));
                    continue;
                }
                if entry.get("type").and_then(serde_json::Value::as_str) == Some("remote") {
                    out.left.insert(name.clone(), REMOTE.into());
                    continue;
                }
                let one = serde_json::json!({ "mcp": { name: entry } }).to_string();
                match parse(format, &one) {
                    Ok(mut parsed) => out
                        .servers
                        .extend(parsed.remove(name).map(|s| (name.clone(), s))),
                    // An empty command is the one shape `parse` refuses here.
                    Err(_)
                        if entry
                            .get("command")
                            .and_then(serde_json::Value::as_array)
                            .is_none_or(|c| c.is_empty()) =>
                    {
                        out.left.insert(name.clone(), NO_COMMAND.into());
                    }
                    Err(e) => return Err(e).with_context(|| format!("reading server `{name}`")),
                }
            }
        }
        other => anyhow::bail!("`{other:?}` is not an MCP format and cannot be imported"),
    }
    Ok(out)
}
