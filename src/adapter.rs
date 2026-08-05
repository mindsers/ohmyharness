//! An adapter is data, not code.
//!
//! The profile carries the *superset* of what any harness can do. An adapter
//! declares which of those capabilities its harness can express, and where each
//! one lives. An absent key means "this harness cannot do this" — so graceful
//! degradation is a missing map entry rather than special-case logic.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `deny_unknown_fields` matters more than it looks: without it a stale or
/// misspelled adapter parses cleanly with zero capabilities, and every harness
/// silently degrades to nothing instead of failing loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub name: String,
    /// Executable to invoke inside the container.
    pub bin: String,
    /// Command that installs `bin` into the image.
    pub install: String,
    /// Paths holding credentials, captured by `omh auth` and remounted on launch.
    #[serde(default)]
    pub creds: Vec<String>,
    #[serde(default)]
    pub capabilities: BTreeMap<Capability, Binding>,
}

/// The capability classes a profile can carry. Ordered as declared, which is
/// also the order they are staged and reported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Rules,
    Skills,
    Mcp,
    Commands,
    Subagents,
    Hooks,
}

impl Capability {
    /// Where this capability lives inside a profile layer.
    pub fn source(&self) -> &'static str {
        match self {
            Self::Rules => "AGENTS.md",
            Self::Skills => "skills",
            Self::Mcp => "mcp.json",
            Self::Commands => "commands",
            Self::Subagents => "subagents",
            Self::Hooks => "hooks",
        }
    }

    pub const ALL: [Capability; 6] = [
        Self::Rules,
        Self::Skills,
        Self::Mcp,
        Self::Commands,
        Self::Subagents,
        Self::Hooks,
    ];
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.source().trim_end_matches(".md").trim_end_matches(".json"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Guest path this harness reads. `$HOME` is expanded per image.
    pub path: String,
    /// Extra guest paths pointed at the same content.
    #[serde(default)]
    pub also: Vec<String>,
    pub render: Render,
    /// Host-side path `omh mcp import` reads from. Distinct from `path`, which
    /// is where the *container* expects the file.
    #[serde(default)]
    pub import: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Render {
    /// Union layers by entry name, mount the result read-only.
    Dir,
    /// Join layers (global first) and write into the worktree.
    Concat,
    /// `{ "mcpServers": { ... } }`.
    McpJson,
    /// `[mcp_servers.name]` tables.
    CodexToml,
    /// `{ "mcp": { name: { type, command } } }`.
    OpencodeJson,
    /// Claude Code `settings.json` hook shape.
    ClaudeSettings,
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
        let adapter = Self::load(&path)?;
        if adapter.capabilities.is_empty() {
            anyhow::bail!(
                "adapter {} declares no capabilities — it would launch a harness \
                 that can see none of your profile",
                path.display()
            );
        }
        Ok(adapter)
    }

    pub fn supports(&self, cap: Capability) -> Option<&Binding> {
        self.capabilities.get(&cap)
    }
}

/// Adapters write `$HOME` because the container's home differs per image.
pub fn expand(template: &str, home: &str) -> PathBuf {
    PathBuf::from(template.replace("$HOME", home))
}

/// Expand an import path against the **host**. Separate from `expand` on
/// purpose: reusing the guest home here would send `omh mcp import` looking
/// inside a container filesystem that does not exist yet.
pub fn expand_host(template: &str, home: &Path, repo: &Path) -> PathBuf {
    PathBuf::from(
        template
            .replace("$HOME", &home.to_string_lossy())
            .replace("$REPO", &repo.to_string_lossy()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    #[test]
    fn shipped_adapters_parse() {
        let all = Adapter::load_dir(Path::new(REAL)).unwrap();
        assert_eq!(all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["claude", "opencode"]);
    }

    #[test]
    fn capability_support_matches_reality() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        for cap in Capability::ALL {
            assert!(claude.supports(cap).is_some(), "claude should support {cap}");
        }

        let oc = Adapter::find(Path::new(REAL), "opencode").unwrap();
        assert!(oc.supports(Capability::Skills).is_some());
        assert!(oc.supports(Capability::Hooks).is_none(), "opencode has no hooks");
        assert!(oc.supports(Capability::Subagents).is_none(), "opencode has no subagents");
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    /// Regression: the pre-capability adapter format parsed cleanly and yielded
    /// zero capabilities, so every harness silently degraded to nothing while
    /// reporting success. Silent total degradation is the worst possible failure
    /// for a tool whose promise is "your setup is already there".
    #[test]
    fn stale_flat_format_is_rejected_loudly() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "old.toml",
            r#"
            name = "old"
            bin = "old"
            install = "x"
            rules = { path = "/work/OLD.md" }
            "#,
        );
        let err = Adapter::find(d.path(), "old").unwrap_err();
        assert!(format!("{err:#}").contains("rules"), "must name the stray key: {err:#}");
    }

    #[test]
    fn zero_capability_adapter_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "empty.toml", r#"name="e"
bin="e"
install="x""#);
        let err = Adapter::find(d.path(), "empty").unwrap_err();
        assert!(err.to_string().contains("no capabilities"), "got: {err}");
    }

    #[test]
    fn unknown_harness_lists_known_ones() {
        let err = Adapter::find(Path::new(REAL), "nope").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("claude") && msg.contains("opencode"), "got: {msg}");
    }

    #[test]
    fn expand_substitutes_guest_home() {
        assert_eq!(expand("$HOME/.claude/skills", "/home/agent"), Path::new("/home/agent/.claude/skills"));
        assert_eq!(expand("/work/CLAUDE.md", "/home/agent"), Path::new("/work/CLAUDE.md"));
    }

    #[test]
    fn import_paths_expand_against_the_host_not_the_container() {
        let home = Path::new("/Users/me");
        let repo = Path::new("/Users/me/code/proj");

        assert_eq!(
            expand_host("$HOME/.config/opencode/opencode.json", home, repo),
            Path::new("/Users/me/.config/opencode/opencode.json")
        );
        assert_eq!(expand_host("$REPO/.mcp.json", home, repo), repo.join(".mcp.json"));
        assert_eq!(expand_host("/absolute/path", home, repo), Path::new("/absolute/path"));
    }

    /// The trap: `expand` targets the container home, `expand_host` the real one.
    /// Confusing them sends import looking in a filesystem that does not exist.
    #[test]
    fn guest_and_host_expansion_disagree_on_purpose() {
        let home = Path::new("/Users/me");
        let repo = Path::new("/repo");
        assert_ne!(
            expand_host("$HOME/.claude", home, repo),
            expand("$HOME/.claude", "/home/agent")
        );
    }

    #[test]
    fn shipped_adapters_declare_where_to_import_mcp_from() {
        for name in ["claude", "opencode"] {
            let a = Adapter::find(Path::new(REAL), name).unwrap();
            assert!(
                a.supports(Capability::Mcp).unwrap().import.is_some(),
                "{name} must say where `omh mcp import` should look"
            );
        }
    }
}
