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
    /// How to ask the harness whether it is logged in, for the harnesses where
    /// no file can answer.
    ///
    /// `token` assumes credentials land somewhere omh can stat. omp keeps them
    /// in SQLite, so the only file that could be named is created by boot noise
    /// and every unauthenticated session would read as logged in — the exact
    /// false positive [`crate::auth::unfilled`] exists to prevent. The same
    /// move `verify`/`ready` already make for MCP: when a claim is about
    /// software omh did not write, ask that software.
    ///
    /// Mutually exclusive with `token` rather than a fallback for it. A harness
    /// that can be asked *and* stat'd would have two answers and no rule for
    /// which wins, and the one time that happened for MCP the stale answer was
    /// the confident one.
    #[serde(default, rename = "token-probe")]
    pub token_probe: Option<Probe>,
    /// What the user has to do once the harness opens. Every harness starts its
    /// login differently and none of them say so on the way in.
    #[serde(default)]
    pub login: Option<String>,
    /// How this harness names the things an agent does.
    ///
    /// Adapter-level, not per-capability: `edit`/`read`/`shell`/`search` is
    /// omh's answer to "what did the agent just do", and hooks were only the
    /// first thing to need it. The Agent Skills standard carries harness tool
    /// names in `allowed-tools` and subagent frontmatter carries them in
    /// `tools` — one vocabulary, one leak, and neither of those is a hook.
    #[serde(default)]
    pub tools: BTreeMap<crate::hook::Tool, String>,
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
    /// Host-side path `omh config mcp import` reads from. Distinct from
    /// `path`, which
    /// is where the *container* expects the file.
    #[serde(default)]
    pub import: Option<String>,
    /// This harness's own command for listing what it loaded, run inside the
    /// sandbox by `omh doctor`.
    ///
    /// `path` is a claim about external software, and the only thing that can
    /// settle it is the harness saying it read the file. Nothing host-side can:
    /// omh rendered a valid document and mounted it faithfully at
    /// `$HOME/.mcp.json` for as long as that binding existed, and Claude Code
    /// never read a byte of it. Every unit test was green throughout.
    #[serde(default)]
    pub verify: Option<String>,
    /// What `verify`'s output calls a server that is actually running.
    ///
    /// Separate from being *listed*, and the distinction is the whole check: a
    /// project-scoped document Claude Code has not been told to trust is listed
    /// in full and loaded not at all. Matching on the name alone would pass in
    /// exactly that state.
    #[serde(default)]
    pub ready: Option<String>,
    /// How this harness spells each moment omh knows about. An absent entry
    /// means it has no such moment, so the hooks wanting it are dropped by name
    /// and the rest still ship.
    #[serde(default)]
    pub events: BTreeMap<crate::hook::Event, String>,
    /// Where each canonical payload field lives in this harness's payload.
    ///
    /// Read in the renderer's own language, never one syntax for all three:
    /// jq paths for Claude Code, which hands a hook its payload on stdin, and
    /// JavaScript property names for both plugin renders, which receive an
    /// object. Calling this "the stdin schema" was true of one harness and
    /// wrong about the other two.
    #[serde(default)]
    pub fields: BTreeMap<crate::hook::Field, String>,
    /// This harness's protocol for putting text in the agent's context.
    #[serde(default)]
    pub inject: Option<Template>,
    /// This harness's protocol for **blocking** a call and saying why.
    ///
    /// Separate from `inject` because the two are different protocols wherever
    /// both exist — Claude Code takes `additionalContext` for one and
    /// `permissionDecision` for the other — and absent wherever a harness can
    /// advise but not block. An absent map drops the hook by name, the same
    /// degradation an unmapped moment or field gets.
    #[serde(default)]
    pub refuse: Option<Template>,
}

impl Binding {
    /// The template for what this hook *means*, or what this harness cannot say.
    ///
    /// The pairing of action to protocol, in one place. It used to be written
    /// out in each renderer — two `match`es, two copies of the drop strings —
    /// so a harness that could advise but not block had two chances to be told
    /// the wrong thing, and `Wired`'s own doc already argues that drop reasons
    /// must be identical whichever renderer asked.
    ///
    /// `Ok(None)` is a `run`: it needs no protocol, which is different from a
    /// protocol this harness lacks.
    pub fn protocol(
        &self,
        action: &crate::hook::Action,
    ) -> std::result::Result<Option<&Template>, &'static str> {
        match action {
            crate::hook::Action::Run(_) => Ok(None),
            crate::hook::Action::Inject { .. } => {
                self.inject.as_ref().map(Some).ok_or("way to inject text")
            }
            crate::hook::Action::Refuse { .. } => {
                self.refuse.as_ref().map(Some).ok_or("way to refuse a call")
            }
        }
    }
}

/// The one piece of a harness's hook protocol that is a shape rather than a
/// name. `{{text}}` receives a shell word, `{{event}}` the harness's own word
/// for the moment — some protocols name the event back at you in the payload.
///
/// Named for what it *is* rather than for one of its two users. It was called
/// `Inject`, so `Binding.refuse` had type `Inject` — the type asserting that the
/// blocking protocol and the advisory one are the same thing, in the one place
/// where the design turns on their not being.
///
/// **Two newtypes were tried and removed.** `Advise(Template)` and
/// `Block(Template)` read as though they stopped the two being confused, and
/// they do not: both deserialize identically and both reach the renderer as a
/// template, so swapping the two *field types* still compiled. A type that
/// looks like a guarantee and is not is worse than the honest name, so the
/// protection lives where it can actually be checked — [`Binding::protocol`]
/// pairs an action with its protocol once, and a test holds it in both
/// directions.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub template: String,
}

/// A question put to the harness, and the answer that means yes.
///
/// The same shape as `verify`/`ready` on a [`Binding`], and deliberately the
/// same shape: "run this and look for that" is one idea, and it now has one
/// spelling. Both fields are required — a probe with no marker greps for
/// nothing and passes on any output at all, which is worse than no probe,
/// because it reports an answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    /// Run inside the sandbox, where the harness and its credentials are.
    pub run: String,
    /// What that command's output says when a login is real.
    pub ready: String,
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
    /// An opencode plugin: a TypeScript module, not a configuration file.
    ///
    /// The one render that emits a **program**. opencode has no declarative
    /// hook config — nor does any second harness omh looked at — so the phase
    /// that was scoped as "the three maps exercised elsewhere" turned out to
    /// need a code generator. The hook *bodies* stay shell; the module is glue.
    OpencodePlugin,
    /// An oh-my-pi hook module: a default-exported factory, not a config file.
    ///
    /// The second harness to express hooks as a program, which settles the
    /// question `OpencodePlugin`'s doc left open — declarative hook config is
    /// the exception, not the rule, and omh's vocabulary earns its keep by
    /// being the only place a hook is written once.
    ///
    /// Distinct from `OpencodePlugin` rather than shared with it: opencode
    /// registers one object of named handlers and reads a tool's arguments off
    /// `output`/`input` depending on the moment, while omp gives every hook
    /// its own `pi.on(...)` registration and hands each handler an `event`. The two
    /// emit different programs from the same maps, which is exactly what a
    /// `render` names.
    OmpPlugin,
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
        adapter.check_login(&path)?;
        adapter.check_hook_maps(&path)?;
        Ok(adapter)
    }

    /// A harness proves a login one way, not two.
    ///
    /// `token` is stat'd and `token-probe` is asked, and an adapter declaring
    /// both leaves omh with two answers and no rule for which wins. There *was*
    /// a rule — `credential_checks` dropped the probe whenever `token` was
    /// non-empty — but it lived in one consumer, went unmentioned, and
    /// `auth::decided_by_files` needed the same fact and could not see it.
    ///
    /// Refused here rather than modelled as an enum. An enum would make the
    /// state unrepresentable and would cost a `TryFrom` shim plus a rename at
    /// every `adapter.token` reader; this is the same trade `Template`'s doc
    /// records — put the check where it can actually be enforced, and hold it
    /// with a test, rather than build a type that looks like a guarantee.
    fn check_login(&self, path: &Path) -> Result<()> {
        if !self.token.is_empty() && self.token_probe.is_some() {
            anyhow::bail!(
                "adapter {} declares both `token` and `token-probe` — omh would \
                 have two answers to whether this harness is logged in and no \
                 rule for which wins. Keep `token` if a file proves the login, \
                 `token-probe` if only the harness can say.",
                path.display()
            );
        }
        Ok(())
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
        // The tool vocabulary is adapter-level, so its guard has to be too.
        // Both directions matter: hooks without a vocabulary drop every
        // tool-scoped hook and blame the harness, and a vocabulary without
        // hooks is a map read by nobody — the failure this whole function
        // exists for.
        let hooks = self.capabilities.contains_key(&Capability::Hooks);
        if hooks && self.tools.is_empty() {
            anyhow::bail!(
                "adapter {}: `hooks` is declared with no `[tools]`, so every hook that \
                 names a tool would be dropped and reported as unsupported. Map at \
                 least one of edit, read, shell, search.",
                path.display()
            );
        }
        if !hooks && !self.tools.is_empty() {
            anyhow::bail!(
                "adapter {}: `[tools]` is read by nobody here — only `hooks` uses it, \
                 and this adapter declares none.",
                path.display()
            );
        }
        for (cap, binding) in &self.capabilities {
            let declares = !binding.events.is_empty()
                || !binding.fields.is_empty()
                || binding.inject.is_some()
                || binding.refuse.is_some();
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
                    "adapter {}: `{cap}` declares hook maps (`events`, `fields` \
                     or `inject`), which are read only under `hooks`. Nothing \
                     would use them.",
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
/// purpose: reusing the guest home here would send `omh config mcp import`
/// looking
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

    /// Each action reaches for the protocol that action *means*, and a harness
    /// missing that one says which it lacked.
    ///
    /// The pairing lives here now because it used to live in both renderers —
    /// two matches, two copies of the drop strings — and a harness that could
    /// advise but not block had two chances to be told the wrong thing. Swapping
    /// the two arms turned every wall into a nudge, which six behavioural tests
    /// do catch; this is the guard on the single place they now share.
    #[test]
    fn each_action_asks_for_the_protocol_it_means() {
        let advise_only: Binding = toml::from_str(
            "path = \"/x\"\nrender = \"claude-settings\"\n[inject]\ntemplate = \"say {{text}}\"\n",
        )
        .unwrap();
        let block_only: Binding = toml::from_str(
            "path = \"/x\"\nrender = \"claude-settings\"\n[refuse]\ntemplate = \"deny {{text}}\"\n",
        )
        .unwrap();

        let inject = crate::hook::Action::Inject {
            capture: None,
            text: "t".into(),
        };
        let refuse = crate::hook::Action::Refuse { text: "t".into() };
        let run = crate::hook::Action::Run("x".into());

        assert_eq!(
            advise_only.protocol(&inject).unwrap().unwrap().template,
            "say {{text}}"
        );
        assert_eq!(
            advise_only.protocol(&refuse).err(),
            Some("way to refuse a call"),
            "a harness that can only advise must not block with the nudge protocol"
        );
        assert_eq!(
            block_only.protocol(&refuse).unwrap().unwrap().template,
            "deny {{text}}"
        );
        assert_eq!(
            block_only.protocol(&inject).err(),
            Some("way to inject text"),
            "and must not promote a nudge to a wall"
        );
        // A `run` needs no protocol, which is not the same as lacking one.
        assert!(advise_only.protocol(&run).unwrap().is_none());
    }

    #[test]
    fn shipped_adapters_parse() {
        let all = Adapter::load_dir(Path::new(REAL)).unwrap();
        assert_eq!(
            all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["claude", "omp", "opencode"]
        );
    }

    /// Every path here is a claim about oh-my-pi, read out of its own source at
    /// the version this adapter was written against — `v17.3.3`, tag-pinned
    /// rather than `main`, because a doc read off the default branch describes
    /// software nobody is running yet.
    ///
    /// The user roots come from `packages/coding-agent/src/discovery/builtin.ts`
    /// rather than from the prose: the docs name `.omp/skills` for a project and
    /// leave the user root to `getAgentDir()`, which resolves to
    /// `~/.omp/agent` — and to `~/.omp/profiles/<name>/agent` when a profile is
    /// active, which is why omh's own `--profile` and omp's are not the same
    /// word twice.
    #[test]
    fn omp_reads_where_its_documentation_says() {
        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        let path = |c: Capability| omp.supports(c).map(|b| b.path.as_str());
        assert_eq!(path(Capability::Skills), Some("$HOME/.omp/agent/skills"));
        assert_eq!(
            path(Capability::Commands),
            Some("$HOME/.omp/agent/commands")
        );
        assert_eq!(
            path(Capability::Subagents),
            Some("$HOME/.omp/agent/agents"),
            "`~/.omp/agent/agents/*.md` per docs/task-agent-discovery.md; the \
             singular `agent` is the config root, not the agent directory"
        );
        assert_eq!(
            path(Capability::Mcp),
            Some("$HOME/.omp/agent/mcp.json"),
            "user scope; the project files omp also reads are `.omp/mcp.json` \
             and `.omp/.mcp.json`, and neither is where omh mounts yours"
        );
        // AGENTS.md, not CLAUDE.md: omp prefers the former and reads the latter
        // only as a fallback flavour.
        assert_eq!(path(Capability::Rules), Some("/work/AGENTS.md"));
    }

    /// omp's tool vocabulary, and the one word of omh's it cannot spell.
    ///
    /// `search` is absent for the same reason it is absent on opencode: `grep`
    /// and `glob` are separate tools there, omh has one word for both, and a
    /// hook narrowing to `search` is dropped by name rather than silently
    /// matching half of what it asked for.
    #[test]
    fn omp_spells_the_tools_it_has_and_no_others() {
        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        assert_eq!(omp.tools[&crate::hook::Tool::Shell], "bash");
        assert_eq!(omp.tools[&crate::hook::Tool::Read], "read");
        assert_eq!(omp.tools[&crate::hook::Tool::Edit], "edit");
        assert!(
            !omp.tools.contains_key(&crate::hook::Tool::Search),
            "omp has grep and glob as separate tools; half a claim is worse than none"
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
        for cap in Capability::ALL {
            assert!(oc.supports(cap).is_some(), "opencode should support {cap}");
        }
        // Hooks were the last absent one, and they are absent no longer:
        // opencode grew a plugin system, so omh generates a module rather than
        // a config file. All three shipped adapters express all six
        // capabilities, which is the capability floor `decisions.md` asks for
        // reached rather than merely approached.
        // opencode *does* have subagents — agent markdown files under
        // `~/.config/opencode/agents/`, with `mode: subagent` in the
        // frontmatter. This adapter said otherwise, so omh dropped them at
        // every opencode launch and reported it as correct degradation:
        // `decisions.md` calls that violating the capability floor — "omh must
        // never cost you a feature you already had".
        assert!(
            oc.supports(Capability::Subagents).is_some(),
            "opencode has agent files; dropping them costs a feature the user had"
        );

        // The floor is a floor, not a claude-and-opencode fact. omp expresses
        // all six too, and the one that took work is `hooks`: it has no
        // declarative hook config either, so declaring it meant a second code
        // generator rather than a second path.
        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        for cap in Capability::ALL {
            assert!(omp.supports(cap).is_some(), "omp should support {cap}");
        }
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

    /// The paths a harness reads, pinned to what its documentation says.
    ///
    /// These are the claims CONTRIBUTING puts at the top: a wrong one does not
    /// crash anything — the harness starts and simply never sees your profile,
    /// and `omh doctor` cannot tell, because its `dir` check verifies that the
    /// directory *omh mounted* holds what omh put there. It passes identically
    /// whether the path is right or wrong.
    ///
    /// So the value is pinned here and the source is cited, which is the most a
    /// test can do: it cannot verify the claim, but it can stop one drifting
    /// silently, and it makes the citation reviewable.
    ///
    /// opencode names these directories in the plural — `agents/`, `commands/`,
    /// `skills/` — per https://opencode.ai/docs/agents/ and
    /// https://opencode.ai/docs/commands/, read 2026-08-12. Singular is
    /// accepted as a legacy alias; omh follows the documented spelling.
    #[test]
    fn opencode_reads_where_its_documentation_says() {
        let oc = Adapter::find(Path::new(REAL), "opencode").unwrap();
        let path = |c: Capability| oc.supports(c).map(|b| b.path.as_str());
        assert_eq!(
            path(Capability::Skills),
            Some("$HOME/.config/opencode/skills")
        );
        assert_eq!(
            path(Capability::Commands),
            Some("$HOME/.config/opencode/commands"),
            "plural — omh spelled this `command` and opencode documents `commands`, \
             so every custom command was mounted where nothing reads"
        );
        assert_eq!(
            path(Capability::Subagents),
            Some("$HOME/.config/opencode/agents")
        );
    }

    /// The tool vocabulary belongs to the harness, not to its hooks.
    ///
    /// `edit`/`read`/`shell`/`search` is omh's answer to "what did the agent
    /// just do", and hooks were simply the first thing to need it. The Agent
    /// Skills standard puts harness tool names in `allowed-tools`, and subagent
    /// frontmatter puts them in `tools` — same vocabulary, same leak, and
    /// neither is a hook. Declared under `[capabilities.hooks.tools]` it was
    /// reachable only by the one consumer that happened to arrive first.
    #[test]
    fn the_tool_vocabulary_is_declared_once_for_the_harness() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        assert_eq!(claude.tools[&crate::hook::Tool::Shell], "Bash");
        assert_eq!(
            claude.tools[&crate::hook::Tool::Edit],
            "Edit|Write|MultiEdit"
        );
    }

    /// A harness with hook moments and no tool vocabulary drops every hook that
    /// names a tool, and blames the harness for it.
    ///
    /// This guard did not move when the map did. `check_hook_maps` refuses a
    /// `hooks` binding with no `events` because a binding that can express no
    /// moment "claimed to have them" — and forgetting to lift `[tools]` to the
    /// top level has the same shape: `graph-first` and `graph-read` are both
    /// dropped, and the user reads "dropped 2 hooks (unsupported)", which is
    /// never a true statement about any harness.
    #[test]
    fn a_harness_with_hooks_must_spell_the_tools() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "wordless.toml",
            r#"
            name = "wordless"
            bin  = "wordless"
            install = "x"
            [capabilities.hooks]
            path   = "$HOME/settings.json"
            render = "claude-settings"
            [capabilities.hooks.events]
            before-tool = "PreToolUse"
            "#,
        );
        let err = format!("{:#}", Adapter::find(d.path(), "wordless").unwrap_err());
        assert!(err.contains("tools"), "must name what is missing: {err}");
    }

    /// And the other direction: a vocabulary nothing can read.
    #[test]
    fn a_harness_without_hooks_has_no_use_for_a_tool_vocabulary() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "odd.toml",
            r#"
            name = "odd"
            bin  = "odd"
            install = "x"
            [tools]
            shell = "Bash"
            [capabilities.rules]
            path   = "/work/AGENTS.md"
            render = "concat"
            "#,
        );
        let err = format!("{:#}", Adapter::find(d.path(), "odd").unwrap_err());
        assert!(err.contains("tools"), "read by nobody, and said so: {err}");
    }

    /// And it is refused inside a capability, where the map it replaced used to
    /// live — otherwise both spellings work and they drift.
    #[test]
    fn a_tool_map_inside_a_capability_is_refused() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "old.toml",
            r#"
            name = "old"
            bin  = "old"
            install = "x"
            [capabilities.hooks]
            path   = "$HOME/settings.json"
            render = "claude-settings"
            [capabilities.hooks.events]
            turn-end = "Stop"
            [capabilities.hooks.tools]
            shell = "Bash"
            "#,
        );
        let err = format!("{:#}", Adapter::find(d.path(), "old").unwrap_err());
        assert!(err.contains("tools"), "must name the key: {err}");
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

    /// Every hook map, not just `events`.
    ///
    /// `fields` and `inject` are legitimate `Binding` members on every
    /// capability, so `deny_unknown_fields` does not catch them in the wrong
    /// place — only this check does, and only `events` was asserted. Dropping
    /// either of the other two disjuncts left the suite green while an adapter
    /// declaring `[capabilities.rules.inject]` loaded cleanly and did nothing.
    #[test]
    fn every_hook_map_is_refused_outside_the_hooks_capability() {
        for map in [
            "[capabilities.rules.events]\nturn-end = \"Stop\"",
            "[capabilities.rules.fields]\ntool-file = \".tool_input.file_path\"",
            "[capabilities.rules.inject]\ntemplate = \"echo {{text}}\"",
        ] {
            let d = tempfile::tempdir().unwrap();
            write(
                d.path(),
                "odd.toml",
                &format!(
                    "name = \"odd\"\nbin = \"odd\"\ninstall = \"x\"\n\
                     [capabilities.rules]\npath = \"/work/AGENTS.md\"\n\
                     render = \"concat\"\n{map}\n"
                ),
            );
            let err = format!("{:#}", Adapter::find(d.path(), "odd").unwrap_err());
            assert!(err.contains("rules"), "{map} must be refused: {err}");
        }
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
            [tools]
            shell = "Bash"
            [capabilities.hooks]
            path   = "$HOME/settings.json"
            render = "claude-settings"
            "#,
        );
        let err = Adapter::find(d.path(), "mute").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("events"), "must name what is missing: {msg}");
    }

    /// An adapter cannot both stat a token and ask the harness.
    ///
    /// `token_probe`'s doc calls the two "mutually exclusive rather than a
    /// fallback", and for a while nothing enforced it: `credential_checks`
    /// resolved the pair with a `.filter(|_| adapter.token.is_empty())`, which
    /// *is* a rule for which wins, applied without a word. An adapter declaring
    /// both parsed, loaded, launched, and quietly lost its probe — a claim the
    /// adapter made, dropped in silence, which is the failure
    /// `deny_unknown_fields` exists one level up to prevent.
    ///
    /// Refused where the zero-capability check already lives, so the rule holds
    /// for every consumer rather than for the one that happened to encode it.
    #[test]
    fn an_adapter_cannot_both_stat_a_token_and_ask_the_harness() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "greedy.toml",
            r#"
name    = "greedy"
bin     = "greedy"
install = "true"
token   = ["$HOME/.greedy/token.json"]

[token-probe]
run   = "greedy whoami"
ready = "account"

[capabilities.rules]
path   = "/work/AGENTS.md"
render = "concat"
"#,
        );
        let err = Adapter::find(d.path(), "greedy").unwrap_err().to_string();
        assert!(
            err.contains("token") && err.contains("token-probe"),
            "the refusal must name both halves so it can be acted on: {err}"
        );
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
                "{name} must say where `omh config mcp import` should look"
            );
        }
    }

    /// **Rules are imported from the personal file, never the repo's.**
    ///
    /// `rules::compose` already puts this project's own `CLAUDE.md`/`AGENTS.md`
    /// into every session — that is what makes a repo's rules reach the agent
    /// at all. Importing the same file into the catalogue would deliver it
    /// twice: once as the project's, once as yours, in one prompt. And it would
    /// keep doing so in every *other* repo you opened afterwards, since the
    /// catalogue travels.
    ///
    /// Asserted as *not under the repo* rather than as a literal path, so an
    /// adapter that changed where it looked would still have to look somewhere
    /// personal.
    #[test]
    fn rules_are_imported_from_your_own_file_not_this_projects() {
        let home = Path::new("/Users/me");
        let repo = Path::new("/repo");
        for name in ["claude", "opencode"] {
            let a = Adapter::find(Path::new(REAL), name).unwrap();
            let binding = a.supports(Capability::Rules).unwrap();
            let template = binding
                .import
                .as_deref()
                .unwrap_or_else(|| panic!("{name} must say where your own rules are"));
            let from = expand_host(template, home, repo);
            assert!(
                !from.starts_with(repo),
                "{name} imports rules from {}, which `rules::compose` already \
                 composes — the agent would be handed the same prose twice",
                from.display()
            );
            assert!(
                from.starts_with(home),
                "{name}: your own rules live in your home, not at {}",
                from.display()
            );
        }
    }

    /// **`import` is never derived from `path`**, even where the two are
    /// textually identical.
    ///
    /// They answer different questions — `path` is where the *container* reads,
    /// `import` where the host keeps yours — and they expand through different
    /// functions, which `guest_and_host_expansion_disagree_on_purpose` guards.
    /// Deriving one from the other happens to work for skills and commands and
    /// is wrong for MCP, where Claude's import source is `$REPO/.mcp.json` and
    /// its mount is `$HOME/.mcp.json`; and wrong for rules, where the mount is
    /// a guest path in `/work` that does not exist on the host at all.
    ///
    /// So the duplication in the adapter files is deliberate, and this is what
    /// stops somebody removing it.
    #[test]
    fn where_a_harness_keeps_yours_is_not_where_omh_mounts_omhs() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        for cap in [Capability::Rules, Capability::Mcp] {
            let binding = claude.supports(cap).unwrap();
            assert_ne!(
                binding.import.as_deref(),
                Some(binding.path.as_str()),
                "{cap}: deriving `import` from `path` would be wrong here"
            );
        }
    }

    /// Every capability omh can copy says where to copy it from.
    ///
    /// Iterated over the capabilities and the adapters rather than spot-checked:
    /// the one that is missing will be the one somebody added last, and its
    /// absence reads as "this harness has none" rather than as an omission.
    #[test]
    fn every_importable_capability_says_where_yours_live() {
        for name in ["claude", "opencode"] {
            let a = Adapter::find(Path::new(REAL), name).unwrap();
            for cap in [
                Capability::Rules,
                Capability::Skills,
                Capability::Commands,
                Capability::Subagents,
            ] {
                let binding = a
                    .supports(cap)
                    .unwrap_or_else(|| panic!("{name} declares no {cap}"));
                assert!(
                    binding.import.is_some(),
                    "{name}/{cap} can be rendered to and not read from"
                );
            }
        }
    }

    /// A harness whose hooks are a **file it reads** says where they are; one
    /// whose hooks are a program omh *generates* does not, and must not.
    ///
    /// opencode's hooks are a TypeScript plugin omh writes — there is nothing
    /// of the user's in it to import, and an `import` key pointing at it would
    /// have `omh import hooks opencode` read omh's own output back and offer to
    /// import it. The absence is the correct answer, so it is asserted rather
    /// than left as an omission somebody later "fixes".
    #[test]
    fn a_harness_says_where_its_hooks_are_only_when_they_are_its_own() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        assert!(
            claude.supports(Capability::Hooks).unwrap().import.is_some(),
            "claude keeps hooks in a file it reads, so omh can import them"
        );

        let opencode = Adapter::find(Path::new(REAL), "opencode").unwrap();
        let hooks = opencode.supports(Capability::Hooks).unwrap();
        assert_eq!(
            hooks.render,
            Render::OpencodePlugin,
            "this test's reasoning rests on opencode's hooks being generated"
        );
        assert!(
            hooks.import.is_none(),
            "omh generates this file — importing it would read omh's own output \
             back and offer it to you as yours"
        );
    }
}
