//! The one place a harness difference costs more than a bind mount.
//!
//! Servers are declared once in the canonical `mcp.json` and rendered into
//! whatever shape the target harness parses. This is how `omh-mcp` (memory) and
//! the wired code-graph server reach every harness without being configured
//! anywhere twice.

use crate::adapter::McpFormat;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

pub type Servers = BTreeMap<String, Server>;

#[derive(Deserialize)]
struct Canonical {
    #[serde(rename = "mcpServers", default)]
    servers: Servers,
}

/// Merge by server name; later files win.
pub fn merge(files: &[impl AsRef<Path>]) -> Result<Servers> {
    let mut out = Servers::new();
    for f in files {
        let raw = std::fs::read_to_string(f.as_ref())
            .with_context(|| format!("reading {}", f.as_ref().display()))?;
        let parsed: Canonical = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", f.as_ref().display()))?;
        out.extend(parsed.servers);
    }
    Ok(out)
}

pub fn render(servers: &Servers, format: McpFormat) -> Result<String> {
    let json = |v: serde_json::Value| serde_json::to_string_pretty(&v).map_err(Into::into);
    match format {
        McpFormat::McpJson | McpFormat::ClaudeJson => {
            json(serde_json::json!({ "mcpServers": servers }))
        }
        McpFormat::OpencodeJson => {
            let mcp: BTreeMap<_, _> = servers
                .iter()
                .map(|(name, s)| {
                    let mut command = vec![s.command.clone()];
                    command.extend(s.args.iter().cloned());
                    (
                        name.clone(),
                        serde_json::json!({
                            "type": "local",
                            "command": command,
                            "environment": s.env,
                            "enabled": true,
                        }),
                    )
                })
                .collect();
            json(serde_json::json!({ "$schema": "https://opencode.ai/config.json", "mcp": mcp }))
        }
        McpFormat::CodexToml => {
            let mut out = String::new();
            for (name, s) in servers {
                out.push_str(&format!("[mcp_servers.{name}]\n"));
                out.push_str(&format!("command = {}\n", toml_str(&s.command)));
                let args: Vec<String> = s.args.iter().map(|a| toml_str(a)).collect();
                out.push_str(&format!("args = [{}]\n", args.join(", ")));
                if !s.env.is_empty() {
                    out.push_str(&format!("\n[mcp_servers.{name}.env]\n"));
                    for (k, v) in &s.env {
                        out.push_str(&format!("{k} = {}\n", toml_str(v)));
                    }
                }
                out.push('\n');
            }
            Ok(out)
        }
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
