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
    /// Paths holding credentials, captured by `omh auth` and remounted on
    /// launch. **A trailing slash declares a directory** — load-bearing, because
    /// a bind-mounted *file* cannot be replaced by rename and every tool saves
    /// its token by writing a temp file and renaming over it.
    #[serde(default)]
    pub creds: Vec<String>,
    /// The file(s) that prove a login happened. A harness fills its config
    /// directory just by starting, so nothing else distinguishes a token from
    /// telemetry.
    #[serde(default)]
    pub token: Vec<String>,
    /// What the user has to do once the harness opens. Every harness starts its
    /// login differently and none of them say so on the way in.
    #[serde(default)]
    pub login: Option<String>,
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
    /// Where this capability lives inside the catalogue.
    ///
    /// `rules` is a **directory of named files** rather than one `AGENTS.md`,
    /// which is what lets a repo take the ones that apply to it: `tdd.md`,
    /// `commit-style.md` and `rust-idiom.md` are separate things you hold. It
    /// also makes the catalogue uniform — every capability is now a directory of
    /// named entries, with `mcp.json` the lone exception because a server is a
    /// record rather than a file.
    pub fn source(&self) -> &'static str {
        match self {
            Self::Rules => "rules",
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

/// The capability's own name, which is also the key an adapter declares it
/// under. Derived from `source()` until an error message had to name one and
/// called the rules capability `AGENTS` — a filename is where a capability
/// lives, not what it is, and the two stop matching the moment either moves.
impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rules => "rules",
            Self::Skills => "skills",
            Self::Mcp => "mcp",
            Self::Commands => "commands",
            Self::Subagents => "subagents",
            Self::Hooks => "hooks",
        })
    }
}

/// Deliberately not `Default`: a binding always comes from an adapter file, and
/// a defaulted `render` would be a claim about a harness nobody made.
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
    /// How this harness spells each moment omh knows about. An absent entry
    /// means it has no such moment, so the hooks wanting it are dropped by name
    /// and the rest still ship.
    #[serde(default)]
    pub events: BTreeMap<crate::hook::Event, String>,
    /// How this harness names the tools each moment happens to.
    #[serde(default)]
    pub tools: BTreeMap<crate::hook::Tool, String>,
    /// Where each canonical payload field lives in this harness's stdin schema.
    #[serde(default)]
    pub fields: BTreeMap<crate::hook::Field, String>,
    /// This harness's protocol for putting text in the agent's context.
    #[serde(default)]
    pub inject: Option<Inject>,
}

/// The one piece of a harness's hook protocol that is a shape rather than a
/// name. `{{text}}` receives a shell word, `{{event}}` the harness's own word
/// for the moment — some protocols name the event back at you in the payload.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inject {
    pub template: String,
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
                if known.is_empty() {
                    "(none)".into()
                } else {
                    known.join(", ")
                },
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
        adapter.check_hook_maps(&path)?;
        Ok(adapter)
    }

    pub fn supports(&self, cap: Capability) -> Option<&Binding> {
        self.capabilities.get(&cap)
    }

    /// The hook maps belong to `hooks` and nowhere else, and a `hooks` binding
    /// without them says nothing.
    ///
    /// Both halves are the `deny_unknown_fields` rule one level in. A map on
    /// `rules` is read by nobody, which is how a key somebody swears they
    /// configured comes to do nothing; a `hooks` binding with no `events` can
    /// express no moment, so every hook is dropped and the harness gets an empty
    /// settings document — indistinguishable from a harness that declares no
    /// hooks, except that this one claimed to have them.
    fn check_hook_maps(&self, path: &Path) -> Result<()> {
        for (cap, binding) in &self.capabilities {
            let declares = !binding.events.is_empty()
                || !binding.tools.is_empty()
                || !binding.fields.is_empty()
                || binding.inject.is_some();
            match cap {
                Capability::Hooks if binding.events.is_empty() => anyhow::bail!(
                    "adapter {}: `hooks` declares no `events`, so it can express no \
                     moment and every hook would be dropped. Map at least one of \
                     session-start, turn-end, before-tool, after-tool — or omit the \
                     `hooks` capability, which is how a harness says it has none.",
                    path.display()
                ),
                Capability::Hooks => {}
                _ if declares => anyhow::bail!(
                    "adapter {}: `{cap}` declares hook maps (`events`, `tools`, \
                     `fields` or `inject`), which are read only under `hooks`. \
                     Nothing would use them.",
                    path.display()
                ),
                _ => {}
            }
        }
        Ok(())
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
        assert_eq!(
            all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["claude", "opencode"]
        );
    }

    #[test]
    fn capability_support_matches_reality() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        for cap in Capability::ALL {
            assert!(
                claude.supports(cap).is_some(),
                "claude should support {cap}"
            );
        }

        let oc = Adapter::find(Path::new(REAL), "opencode").unwrap();
        assert!(oc.supports(Capability::Skills).is_some());
        assert!(
            oc.supports(Capability::Hooks).is_none(),
            "opencode has no hooks"
        );
        assert!(
            oc.supports(Capability::Subagents).is_none(),
            "opencode has no subagents"
        );
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
        assert!(
            format!("{err:#}").contains("rules"),
            "must name the stray key: {err:#}"
        );
    }

    /// The hook maps say how a harness spells omh's hook vocabulary, so on any
    /// other capability they are read by nobody. That is the same failure
    /// `deny_unknown_fields` exists for, one level in: a key in the wrong place
    /// parses cleanly, does nothing, and reports success.
    #[test]
    fn hook_maps_only_appear_on_the_hooks_capability() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "odd.toml",
            r#"
            name = "odd"
            bin = "odd"
            install = "x"
            [capabilities.rules]
            path   = "/work/AGENTS.md"
            render = "concat"
            [capabilities.rules.events]
            turn-end = "Stop"
            "#,
        );
        let err = Adapter::find(d.path(), "odd").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("rules"), "must name the capability: {msg}");
        assert!(msg.contains("hooks"), "and where it belongs: {msg}");
    }

    /// A hooks binding with no `events` can express no moment, so every hook is
    /// dropped and the harness receives an empty settings document — which is
    /// indistinguishable from a harness that declares no hooks at all, except
    /// that this one claimed to have them.
    #[test]
    fn a_hooks_binding_that_names_no_moment_is_refused() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "mute.toml",
            r#"
            name = "mute"
            bin  = "mute"
            install = "x"
            [capabilities.hooks]
            path   = "$HOME/settings.json"
            render = "claude-settings"
            "#,
        );
        let err = Adapter::find(d.path(), "mute").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("events"), "must name what is missing: {msg}");
    }

    #[test]
    fn zero_capability_adapter_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "empty.toml",
            r#"name="e"
bin="e"
install="x""#,
        );
        let err = Adapter::find(d.path(), "empty").unwrap_err();
        assert!(err.to_string().contains("no capabilities"), "got: {err}");
    }

    #[test]
    fn unknown_harness_lists_known_ones() {
        let err = Adapter::find(Path::new(REAL), "nope").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("claude") && msg.contains("opencode"),
            "got: {msg}"
        );
    }

    /// `omh auth` drops you into the harness and it is not obvious what to do
    /// next — Claude Code just shows "Not logged in" in the status bar. An
    /// adapter that declares credentials has to say how to fill them.
    #[test]
    fn every_adapter_with_credentials_says_how_to_log_in() {
        for a in Adapter::load_dir(Path::new(REAL)).unwrap() {
            if !a.creds.is_empty() {
                assert!(a.login.is_some(), "{} does not say how to log in", a.name);
            }
        }
    }

    #[test]
    fn expand_substitutes_guest_home() {
        assert_eq!(
            expand("$HOME/.claude/skills", "/home/agent"),
            Path::new("/home/agent/.claude/skills")
        );
        assert_eq!(
            expand("/work/CLAUDE.md", "/home/agent"),
            Path::new("/work/CLAUDE.md")
        );
    }

    #[test]
    fn import_paths_expand_against_the_host_not_the_container() {
        let home = Path::new("/Users/me");
        let repo = Path::new("/Users/me/code/proj");

        assert_eq!(
            expand_host("$HOME/.config/opencode/opencode.json", home, repo),
            Path::new("/Users/me/.config/opencode/opencode.json")
        );
        assert_eq!(
            expand_host("$REPO/.mcp.json", home, repo),
            repo.join(".mcp.json")
        );
        assert_eq!(
            expand_host("/absolute/path", home, repo),
            Path::new("/absolute/path")
        );
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
