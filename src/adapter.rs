//! An adapter is data, not code. Four facts per harness: where it reads rules,
//! where it reads skills, where its MCP config lives and in what shape, and how
//! to install it. Adding a harness is a TOML file, never a recompile.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Adapter {
    pub name: String,
    /// Executable to invoke inside the container.
    pub bin: String,
    /// Command that installs `bin` into the image.
    pub install: String,
    pub rules: Rules,
    pub skills: Skills,
    pub mcp: Mcp,
    /// Paths holding credentials, captured by `omh auth` and remounted on launch.
    #[serde(default)]
    pub creds: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Rules {
    /// Primary rules file this harness reads.
    pub path: String,
    /// Additional paths to point at the same content.
    #[serde(default)]
    pub also: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Skills {
    /// Directory this harness scans for SKILL.md folders.
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Mcp {
    pub path: String,
    pub format: McpFormat,
}

/// The only place a harness difference costs more than a bind mount.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum McpFormat {
    /// `{ "mcpServers": { name: { command, args, env } } }` nested under a project key.
    ClaudeJson,
    /// Bare `{ "mcpServers": { ... } }`.
    McpJson,
    /// `[mcp_servers.name]` tables.
    CodexToml,
    /// `{ "mcp": { name: { type, command } } }`.
    OpencodeJson,
}

impl Adapter {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading adapter {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing adapter {}", path.display()))
    }

    /// Load every `*.toml` in `dir`, ignoring a missing directory.
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                out.push(Self::load(&path)?);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn find(dir: &Path, name: &str) -> Result<Self> {
        let path = dir.join(format!("{name}.toml"));
        if !path.exists() {
            let known: Vec<_> = Self::load_dir(dir)?.into_iter().map(|a| a.name).collect();
            anyhow::bail!(
                "unknown harness `{name}`\nknown: {}\nadd one by dropping {}",
                if known.is_empty() { "(none)".into() } else { known.join(", ") },
                path.display()
            );
        }
        Self::load(&path)
    }
}

/// Adapters write `$HOME` because the container's home differs per image.
pub fn expand(template: &str, home: &str) -> PathBuf {
    PathBuf::from(template.replace("$HOME", home))
}
