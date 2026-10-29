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
    /// Checked as it is deserialized — see [`Name`]. This field keys
    /// `paths.creds` and `paths.staging`, so an unchecked one is a path.
    #[serde(deserialize_with = "crate::adapter::de_name")]
    pub name: Name,
    /// Executable to invoke inside the container.
    pub bin: String,
    /// Command that installs `bin` into the image. May interpolate the pinned
    /// `version` as `{{version}}`.
    pub install: String,
    /// The harness version this adapter pins. `install` must interpolate it as
    /// `{{version}}`, and a mismatch either way is refused when the adapter
    /// loads — a declared version that never reaches the command is a pin that
    /// does nothing, and a `{{version}}` with nothing to fill it builds an
    /// image that names a literal `{{version}}`.
    #[serde(default)]
    pub version: Option<String>,
    /// Per-target checksums the install verifies, keyed by `uname -m` output
    /// (`x86_64`, `aarch64`). For a harness delivered as a release binary
    /// rather than an npm package, where nothing else proves the download is
    /// the one the pin names.
    #[serde(default)]
    pub checksums: BTreeMap<String, String>,
    /// Where this harness writes its `.jsonl` transcripts, if it does. omh
    /// mounts a per-session directory over it, so `omh sNN` can read what the
    /// agent did and what it cost. `None` for a harness whose transcript shape
    /// omh has not read — the row then says *not recorded*, never a fabricated
    /// empty session.
    #[serde(default)]
    pub transcripts: Option<String>,
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
    /// The harness's own policy layer, filled by omh so a session cannot add a
    /// server the launch did not stage — compared by command and arguments,
    /// not `env` (risk 1b). Absent where a harness has none omh fills yet.
    #[serde(default)]
    pub policy: Option<Policy>,
    /// The keys of the harness's own config that are the user's preferences,
    /// kept per account and copied into each session (`prefs`) — so nothing
    /// else a session writes there reaches the next one.
    #[serde(default)]
    pub preferences: Option<Preferences>,
    /// The account's model provider: a file at the account's root, in the
    /// harness's own schema (`provider.rs`), rendered into a layer the
    /// session cannot write. Absent where a harness takes none yet.
    #[serde(default)]
    pub provider: Option<Provider>,
}

/// Where an account's model provider comes from and where a session reads it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub format: ProviderFormat,
    /// Where the harness keeps its own provider on the host, for `omh auth
    /// --import` to read. `$HOME` is the user's.
    pub import: String,
    /// The file the harness reads providers from, hidden behind the render,
    /// read-only — strictly inside one of `creds`' directories. Only for a
    /// format that is a file of its own: Codex's rides in its System layer.
    #[serde(default)]
    pub path: Option<String>,
    /// Files in the account that can carry a provider's endpoint into the
    /// next session and that the harness rebuilds: removed from the account
    /// before each launch. Each strictly inside one of `creds`' directories.
    #[serde(default)]
    pub drop: Vec<String>,
}

/// What a provider file is written as, which is a claim about one harness.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderFormat {
    /// `model_provider` and `[model_providers.*]` of Codex's `config.toml`.
    CodexConfig,
    /// omp's `models.yml`.
    OmpModels,
}

impl ProviderFormat {
    /// As an adapter spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::CodexConfig => "codex-config",
            Self::OmpModels => "omp-models",
        }
    }
}

/// A file the harness reads as policy — above anything a session can leave
/// behind — and the files a session must not be able to leave behind for the
/// next one.
///
/// Not a [`Binding`]: a policy has no layers, no selection, no import and
/// nothing to parse back. Its allowlist is derived from what the launch
/// stages; the rest of it is fixed per format (see `render::policy`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Guest path the harness reads its policy from: a literal, never
    /// expanded, under `/etc` in the image omh builds — see `check_policy`.
    pub path: String,
    pub format: PolicyFormat,
    /// Guest paths a session can write and the harness trusts on its next
    /// start, each strictly inside one of `creds`' directories. Each is hidden
    /// behind an empty read-only file. `$HOME` is expanded per image.
    #[serde(default)]
    pub mask: Vec<String>,
    /// Credential files each session gets a copy of, instead of the account's
    /// own: seeded from it without what `render::scrub` drops, and never
    /// written back. One session's writes then reach no other. Each is one of
    /// `creds`' *file* entries and a file its format knows how to seed
    /// ([`PolicyFormat::seeds`]) — see `check_policy`.
    #[serde(default)]
    pub copy: Vec<String>,
}

/// What a policy file is written as, which is a claim about one harness.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyFormat {
    /// Codex's `requirements.toml`.
    CodexRequirements,
    /// A Claude Code `managed-settings.d` drop-in.
    ClaudeManaged,
}

impl PolicyFormat {
    /// As an adapter spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::CodexRequirements => "codex-requirements",
            Self::ClaudeManaged => "claude-managed",
        }
    }

    /// The account files `render::scrub` knows how to seed a session's copy
    /// of, for this format — what `[policy] copy` may name. Beside the format
    /// rather than in `render`, which `adapter` does not reach.
    pub fn seeds(self) -> &'static [&'static str] {
        match self {
            Self::CodexRequirements => &[],
            Self::ClaudeManaged => &["$HOME/.claude.json"],
        }
    }
}

/// The keys of a harness's own config that are the user's preferences — a
/// model, a theme — kept per account and copied into each session, where the
/// harness writes them freely (`prefs`). Nothing else a session writes in
/// that file reaches another session.
///
/// **The rule a key is judged by:** a preference never names a program or a
/// capability, never grants a consent, a trust or a permission, and never
/// carries text the harness puts in front of the model. [`NEVER`] spells out
/// what that rules out, per harness, whatever a later release adds to it, and
/// `check_preferences` refuses an adapter declaring a key at, inside or
/// around one. Every list is explicit: a loose one is how a file read
/// through a planted link would be kept.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// How a session's copy reaches the harness.
    pub mount: PrefMount,
    /// Each file, as one of `creds`' paths, and its keys that are preferences:
    /// TOML dotted keys (`tui.theme`), a table declared whole by its name.
    pub files: BTreeMap<String, Vec<String>>,
    /// Bookkeeping the harness writes on its own — a notice dismissed —
    /// kept without asking. Judged by the same rule.
    #[serde(default)]
    pub quiet: BTreeMap<String, Vec<String>>,
}

/// How a session's copy of a preferences file reaches the harness.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PrefMount {
    /// The account's file is a link to the same name under
    /// [`crate::prefs::GUEST`], a directory each container mounts from its
    /// session's own copy — for a harness that saves by writing beside the
    /// file and renaming over it, which a single-file mount refuses with
    /// `EBUSY`, and that resolves the link before it saves.
    Link,
}

/// A key inside a preferences file, one segment per table.
pub type KeyPath = Vec<String>;

/// One declared preferences file, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// As the adapter names it, one of `creds`' paths.
    pub path: String,
    /// Its file name: the name it has under [`crate::prefs::GUEST`] and in
    /// the account's kept preferences.
    pub name: String,
    pub keys: Vec<KeyPath>,
    pub quiet: Vec<KeyPath>,
}

impl Declared {
    /// Every key kept, asked about or not.
    pub fn all(&self) -> impl Iterator<Item = &KeyPath> {
        self.keys.iter().chain(&self.quiet)
    }
}

impl Preferences {
    /// Each file and its keys, parsed — `check_preferences` has held every
    /// one to [`key_path`] already, so none is dropped here.
    pub fn declared(&self) -> Vec<Declared> {
        let parse = |keys: Option<&Vec<String>>| -> Vec<KeyPath> {
            keys.into_iter()
                .flatten()
                .filter_map(|k| key_path(k).ok())
                .collect()
        };
        self.files
            .iter()
            .map(|(path, keys)| Declared {
                path: path.clone(),
                name: path.rsplit('/').next().unwrap_or_default().to_string(),
                keys: parse(Some(keys)),
                quiet: parse(self.quiet.get(path)),
            })
            .collect()
    }
}

/// A TOML dotted key as TOML spells one: bare segments of letters, digits,
/// `_` and `-`, or quoted ones (`notice."hide_gpt-5.1-codex-max_migration_prompt"`),
/// with no escapes — or why it is not one.
pub fn key_path(s: &str) -> Result<KeyPath, String> {
    let refuse = || format!("`{s}` is not a key");
    let mut out = Vec::new();
    let mut rest = s.trim();
    loop {
        let (segment, after) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let inner = &rest[1..];
                let end = inner.find(q).ok_or_else(refuse)?;
                (&inner[..end], &inner[end + 1..])
            }
            _ => {
                let end = rest.find('.').unwrap_or(rest.len());
                let bare = rest[..end].trim();
                if !bare
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err(refuse());
                }
                (bare, &rest[end..])
            }
        };
        if segment.is_empty() || segment.contains('\\') {
            return Err(refuse());
        }
        out.push(segment.to_string());
        let after = after.trim_start();
        if after.is_empty() {
            return Ok(out);
        }
        rest = after.strip_prefix('.').ok_or_else(refuse)?.trim_start();
        if rest.is_empty() {
            return Err(refuse());
        }
    }
}

/// What is never a preference, per harness: each names a program or a
/// capability, grants a consent, a trust or a permission, or carries text the
/// harness puts in front of the model — or picks where traffic goes, which
/// the account's provider file (`provider.rs`) holds instead. A key at, inside
/// or around one is refused. Read in each harness's source at the version
/// `adapters/` pins; a table is listed whole where any of it would qualify.
pub const NEVER: &[(&str, &[&str])] = &[(
    "codex",
    &[
        "mcp_servers",
        "notify",
        "model_provider",
        "model_providers",
        "profile",
        "profiles",
        "features",
        "developer_instructions",
        "instructions",
        "model_instructions_file",
        "experimental_instructions_file",
        "projects",
        "hooks",
        "approval_policy",
        "sandbox_mode",
        "sandbox_workspace_write",
        "shell_environment_policy",
        "tools",
        "chatgpt_base_url",
        "openai_base_url",
        "notice.hide_full_access_warning",
        "notice.hide_world_writable_warning",
    ],
)];

/// [`NEVER`]'s keys for `harness`, parsed.
pub fn never_for(harness: &str) -> Vec<KeyPath> {
    NEVER
        .iter()
        .filter(|(h, _)| *h == harness)
        .flat_map(|(_, keys)| keys.iter())
        .filter_map(|k| key_path(k).ok())
        .collect()
}

/// Whether one key is the other, or holds it.
pub fn related(a: &KeyPath, b: &KeyPath) -> bool {
    let n = a.len().min(b.len());
    a[..n] == b[..n]
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
    /// The name a command line gives it — `omh use skills`, `[use] mcp = […]`.
    /// A `&'static str` for the callers that keep it, which `Display` cannot
    /// hand out.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Skills => "skills",
            Self::Mcp => "mcp",
            Self::Commands => "commands",
            Self::Subagents => "subagents",
            Self::Hooks => "hooks",
        }
    }

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
        f.write_str(self.key())
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
    /// Host-side path `omh settings mcp import` reads from. Distinct from
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
    /// What `verify`'s output calls a server the harness has taken up — as
    /// much as that harness can say. Claude Code's `Connected` means running;
    /// Codex's `enabled` means only that its parsed config holds the server.
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
    /// The escape hatch `fields` cannot express: a field that lives
    /// somewhere else on one particular tool. omp's `edit` is the reason
    /// this exists — its path is not a property at all, but a substring of
    /// one `input` string, so no single suffix appended to `event.input?.`
    /// can read it the way `fields` reads every other tool's.
    ///
    /// Unlike `fields`, an entry here is a **complete expression**, used
    /// verbatim rather than substituted into a template — `fields` cannot
    /// hold that shape without breaking every harness whose fields really
    /// are plain property names. A tool with no entry here falls back to
    /// `fields`, so this is additive: every binding that never needed it
    /// renders exactly as before.
    #[serde(default, rename = "fields-by-tool")]
    pub tool_fields: BTreeMap<crate::hook::Tool, BTreeMap<crate::hook::Field, String>>,
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
    /// Codex `[[hooks.<Event>]]` tables, for the **system** config layer.
    ///
    /// Claude Code's shape in TOML, with two differences the renderer owns.
    /// Codex runs a hook from the user or project layer only once a
    /// `trusted_hash` for it is recorded, and runs one from the system layer
    /// (`/etc/codex/config.toml`) as managed — so omh's hooks go there, and
    /// yours in `~/.codex` keep Codex's own trust. And Codex takes a shell
    /// exit status of 2 as a verdict at every moment, which at `Stop` it
    /// honours forever: see `codex_hooks`.
    CodexHooks,
}

/// A harness or editor name, checked to be a name rather than a path.
///
/// `Adapter::find` and `Editor::find` resolve a word by joining it into
/// `<dir>/<word>.toml`, and the word arrives from every command that names a
/// harness — `omh new`, `omh auth`, `omh doctor`, `omh eject`, `omh import` —
/// and from a `.harness` marker on disk, which
/// `session::harness_of` already distrusts on the grounds that "omh wrote this,
/// but a file on disk is not omh's word". Unchecked, `..` in that word named a
/// file outside the catalogue and omh loaded it: its `install` becomes a `RUN`
/// line in a host `docker build`, its `bin` becomes the sandbox's argv, and
/// before the sibling fix in `auth::dir` the same word also chose what was
/// bound over `/home/agent`.
///
/// Identifier shape, not merely "no separators". The predicate in
/// `session::harness_of` rejects `/`, `\`, newlines and control characters,
/// which is right for the marker it guards and **not** sufficient here: it
/// admits `..` and `.`, each of which still forms a path. (It does reject an
/// empty name, five lines before the character check.) Every adapter and
/// editor omh ships already satisfies this rule.
///
/// **Held by the field, not only checked at the door.** The first version
/// validated the typed word inside `Adapter::find` and left `Adapter.name`
/// alone — and `auth::dir` then keyed the credential directory on that field,
/// which swapped a checked string for an unchecked one. `paths.creds` and
/// `paths.staging` both join it, and the first is mounted **writable** over
/// the harness's config directory, so `name = "../../escaped"` put
/// `create_dir_all` outside `~/.omh/creds`. Parsing on the way out of serde is
/// what makes the two spellings one rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(name: &str) -> Result<Self> {
        anyhow::ensure!(!name.is_empty(), "a harness name cannot be empty");
        anyhow::ensure!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "`{name}` is not a harness name — names are letters, digits, `-` and `_`, \
             so they cannot reach outside the catalogue"
        );
        Ok(Self(name.to_string()))
    }
}

/// serde's hook for [`Name`], so a bad one is a parse error naming the file
/// rather than a value that reaches a path join. `deserialize_with` rather
/// than `try_from` on the struct: the container needs `deny_unknown_fields`,
/// and `#[serde(try_from)]` on the outer type would take that off.
pub fn de_name<'de, D>(d: D) -> std::result::Result<Name, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(d)?;
    Name::parse(&raw).map_err(serde::de::Error::custom)
}

impl TryFrom<String> for Name {
    type Error = anyhow::Error;
    fn try_from(name: String) -> Result<Self> {
        Self::parse(&name)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl Adapter {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading adapter {}", path.display()))?;
        let adapter: Self =
            toml::from_str(&raw).with_context(|| format!("parsing adapter {}", path.display()))?;
        // Validate the pin as the adapter loads, so a build never embeds a
        // literal `{{version}}` or silently ignores a declared one. The
        // dockerfile builders below rely on this having run.
        adapter
            .install_command()
            .with_context(|| format!("adapter {}", path.display()))?;
        // A checksum verified but never declared is a check against nothing:
        // the install would `sha256sum -c` a file it also wrote, or fail on a
        // sum that is not there. If the command checks one, the field carries
        // it — where `omh doctor` reads it too.
        anyhow::ensure!(
            // `sha256sum -c`, the verification form, not a bare mention: an
            // echoed or commented `sha256sum` should not trip this.
            !adapter.install.contains("sha256sum -c") || !adapter.checksums.is_empty(),
            "adapter {}: the install verifies a checksum but declares none",
            path.display()
        );
        Ok(adapter)
    }

    /// The install command with the pinned version filled in.
    ///
    /// Refuses the two ways a pin can be inert: a `version` the command has no
    /// `{{version}}` to receive, and a `{{version}}` with no `version` to fill
    /// it. Both compile and both ship an image nobody meant — the first
    /// installs `latest` while claiming a pin, the second installs a package
    /// literally called `{{version}}`.
    pub fn install_command(&self) -> Result<String> {
        let templated = self.install.contains("{{version}}");
        match (&self.version, templated) {
            (Some(v), true) => Ok(self.install.replace("{{version}}", v)),
            (Some(_), false) => anyhow::bail!(
                "declares a version but its install command has no `{{{{version}}}}` to put it in"
            ),
            (None, true) => anyhow::bail!(
                "install command interpolates `{{{{version}}}}` but declares no version"
            ),
            (None, false) => Ok(self.install.clone()),
        }
    }

    /// Load every `*.toml` in `dir`, ignoring a missing directory.
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>> {
        // **Absent is empty; unreadable is an error.** A catalogue with no
        // entries yet has no directory, and refusing there would refuse every
        // command on a fresh install. Every other error is omh unable to look,
        // and answering `[]` to that put "nothing is installed" and "nobody
        // could read this" into the same word — which `omh inspect` lists and
        // `tool_hint` prints as `available:`.
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(anyhow::Error::new(e))
                    .with_context(|| format!("reading {}", dir.display()))
            }
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
        let name = &Name::parse(name)?;
        let path = dir.join(format!("{name}.toml"));
        if !path.exists() {
            let known: Vec<_> = Self::load_dir(dir)?
                .into_iter()
                .map(|a| a.name.to_string())
                .collect();
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
        adapter.check_policy(&path)?;
        adapter.check_preferences(&path)?;
        adapter.check_provider(&path)?;
        Ok(adapter)
    }

    /// A provider render hides a file the account keeps, so it lands strictly
    /// inside one of `creds`' directories, where `auth::prepare` lays its
    /// mountpoint — and only for a format that is a file of its own. A dropped
    /// file is removed from the account, so it is one the account holds too.
    /// Neither walks with `..`: both are joined into the account directory on
    /// the host. An import reads the user's own home.
    fn check_provider(&self, path: &Path) -> Result<()> {
        let Some(provider) = &self.provider else {
            return Ok(());
        };
        let shown = path.display();
        let format = provider.format.name();
        let dirs: Vec<&str> = self
            .creds
            .iter()
            .filter(|c| c.ends_with('/'))
            .map(String::as_str)
            .collect();
        let inside = |p: &str| {
            dirs.iter()
                .any(|d| p.strip_prefix(d).is_some_and(|rest| !rest.is_empty()))
        };
        let walks = |p: &str| {
            Path::new(p)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        };
        match (provider.format, &provider.path) {
            (ProviderFormat::OmpModels, None) => anyhow::bail!(
                "adapter {shown}: `[provider]` format {format} needs a `path` — the file omp \
                 reads providers from, which the render hides"
            ),
            (ProviderFormat::CodexConfig, Some(_)) => anyhow::bail!(
                "adapter {shown}: `[provider]` format {format} takes no `path` — it is \
                 rendered into Codex's System layer"
            ),
            (_, Some(at)) => {
                anyhow::ensure!(
                    inside(at),
                    "adapter {shown}: `[provider] path = {at:?}` must be strictly inside one of \
                     this adapter's credential directories ({})",
                    dirs.join(", ")
                );
                anyhow::ensure!(
                    !walks(at),
                    "adapter {shown}: `[provider] path = {at:?}` has a `..` — it is joined \
                     into the account directory on the host"
                );
            }
            (_, None) => {}
        }
        let import = &provider.import;
        anyhow::ensure!(
            import.starts_with("$HOME/") && !walks(import),
            "adapter {shown}: `[provider] import = {import:?}` must be under $HOME/, with no \
             `..` — it is the user's own file"
        );
        for drop in &provider.drop {
            anyhow::ensure!(
                inside(drop),
                "adapter {shown}: `[provider]` drop entry {drop:?} must be strictly inside one \
                 of this adapter's credential directories ({})",
                dirs.join(", ")
            );
            anyhow::ensure!(
                !walks(drop),
                "adapter {shown}: `[provider]` drop entry {drop:?} has a `..` — it is joined \
                 into the account directory on the host"
            );
        }
        Ok(())
    }

    /// The policy is above everything a session leaves behind only if a
    /// session cannot replace it: a file under `/etc`, the image's admin layer,
    /// which none of the mounts omh makes writable reaches (`/work`, the
    /// harness's home, `/omh`), and not under one this adapter makes writable
    /// (`creds`, `transcripts`) or at a capability's own path. A mask is the
    /// other way round — it hides a file the account keeps, strictly inside a
    /// `creds` directory, where `auth::prepare` lays its mountpoint. Neither
    /// may walk with `..`: `auth::prepare` joins a mask into the account
    /// directory on the host, and `omh eject` joins the path under `system/`.
    fn check_policy(&self, path: &Path) -> Result<()> {
        let Some(policy) = &self.policy else {
            return Ok(());
        };
        let walks = |p: &str| {
            Path::new(p)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        };
        let at = &policy.path;
        let shown = path.display();
        anyhow::ensure!(
            at.starts_with('/'),
            "adapter {shown}: `[policy] path = {at:?}` must be absolute — it is a literal \
             path in the image, never expanded"
        );
        anyhow::ensure!(
            !walks(at),
            "adapter {shown}: `[policy] path = {at:?}` has no `..` to walk — name the file"
        );
        // The image's admin layer. The mounts omh makes writable are `/work`,
        // the harness's home and `/omh`, and none of them is under `/etc` —
        // unless this adapter puts one there, which is refused next.
        let file = Path::new(at);
        anyhow::ensure!(
            file.starts_with("/etc") && file.components().count() > 2,
            "adapter {shown}: `[policy] path = {at:?}` must name a file under /etc/ — a \
             session writes /work, the harness's home and omh's own mounts"
        );
        // Compared as written: the path is a literal under `/etc`, so only an
        // absolute template can name it — one under `$HOME` never does.
        let writable = self
            .creds
            .iter()
            .map(|c| c.trim_end_matches('/'))
            .chain(self.transcripts.as_deref());
        for mount in writable {
            anyhow::ensure!(
                !file.starts_with(mount),
                "adapter {shown}: `[policy] path = {at:?}` is under a path omh mounts \
                 writable ({mount}) — a session could replace it"
            );
        }
        let bound = self
            .capabilities
            .values()
            .flat_map(|b| std::iter::once(&b.path).chain(b.also.iter()));
        for target in bound {
            anyhow::ensure!(
                Path::new(target) != file,
                "adapter {shown}: `[policy] path = {at:?}` is a capability's path too — \
                 two mounts at one path"
            );
        }
        // A mask hides a file the account keeps — inside one of the directories
        // `creds` mounts writable, never one of them whole, never outside.
        let dirs: Vec<&str> = self
            .creds
            .iter()
            .filter(|c| c.ends_with('/'))
            .map(String::as_str)
            .collect();
        for mask in &policy.mask {
            anyhow::ensure!(
                dirs.iter()
                    .any(|d| mask.strip_prefix(d).is_some_and(|rest| !rest.is_empty())),
                "adapter {shown}: `[policy]` mask entry {mask:?} must be strictly inside one \
                 of this adapter's credential directories ({})",
                dirs.join(", ")
            );
            anyhow::ensure!(
                !walks(mask),
                "adapter {shown}: `[policy]` mask entry {mask:?} has a `..` — it is joined \
                 into the account directory on the host"
            );
        }
        // A copy replaces one credential file's mount, so it names one —
        // never a directory, never a path no credential mount covers.
        let files: Vec<&str> = self
            .creds
            .iter()
            .filter(|c| !c.ends_with('/'))
            .map(String::as_str)
            .collect();
        for copy in &policy.copy {
            anyhow::ensure!(
                files.contains(&copy.as_str()),
                "adapter {shown}: `[policy]` copy entry {copy:?} must be one of this adapter's \
                 credential files ({})",
                files.join(", ")
            );
        }
        // And one `render::scrub` knows: a file it has no list for would be
        // seeded unscrubbed, and an absolute one would not be the session's.
        for copy in &policy.copy {
            anyhow::ensure!(
                policy.format.seeds().contains(&copy.as_str()),
                "adapter {shown}: `[policy]` copy entry {copy:?} is not a file {} knows how \
                 to seed ({})",
                policy.format.name(),
                policy.format.seeds().join(", ")
            );
        }
        Ok(())
    }

    /// A preferences file is one the account keeps, strictly inside one of
    /// `creds`' directories, where its link is laid; TOML, the one format
    /// `prefs` reads so far; with no `..` to walk, since it is joined into the
    /// account directory on the host. Its keys are TOML keys, each declared
    /// once, none inside another, and none at, inside or around a [`NEVER`]
    /// key.
    fn check_preferences(&self, path: &Path) -> Result<()> {
        let Some(prefs) = &self.preferences else {
            return Ok(());
        };
        let shown = path.display();
        let dirs: Vec<&str> = self
            .creds
            .iter()
            .filter(|c| c.ends_with('/'))
            .map(String::as_str)
            .collect();
        let never = never_for(self.name.as_str());
        for (file, keys) in &prefs.files {
            anyhow::ensure!(
                dirs.iter()
                    .any(|d| file.strip_prefix(d).is_some_and(|rest| !rest.is_empty())),
                "adapter {shown}: preferences file {file:?} must be strictly inside one of this \
                 adapter's credential directories ({})",
                dirs.join(", ")
            );
            anyhow::ensure!(
                !Path::new(file)
                    .components()
                    .any(|c| c == std::path::Component::ParentDir),
                "adapter {shown}: preferences file {file:?} has a `..` — it is joined into the \
                 account directory on the host"
            );
            anyhow::ensure!(
                file.ends_with(".toml"),
                "adapter {shown}: preferences file {file:?} — only TOML preferences are read so far"
            );
            anyhow::ensure!(
                !keys.is_empty(),
                "adapter {shown}: preferences file {file:?} declares no keys"
            );
        }
        for file in prefs.quiet.keys() {
            anyhow::ensure!(
                prefs.files.contains_key(file),
                "adapter {shown}: quiet file {file:?} is not one of `[preferences.files]`"
            );
        }
        for (file, keys) in &prefs.files {
            let quiet = prefs.quiet.get(file).into_iter().flatten();
            let mut seen: Vec<(&str, KeyPath)> = Vec::new();
            for raw in keys.iter().chain(quiet) {
                let key = key_path(raw).map_err(|e| anyhow::anyhow!("adapter {shown}: {e}"))?;
                if let Some(n) = never.iter().find(|n| related(&key, n)) {
                    anyhow::bail!(
                        "adapter {shown}: `{raw}` is never a preference — `{}` names a program, \
                         grants a consent or a trust, or carries text before the model",
                        n.join(".")
                    );
                }
                for (other, at) in &seen {
                    anyhow::ensure!(
                        at != &key,
                        "adapter {shown}: `{raw}` is declared twice in {file:?}"
                    );
                    let (outer, inner) = match at.len() < key.len() {
                        true => (*other, raw.as_str()),
                        false => (raw.as_str(), *other),
                    };
                    anyhow::ensure!(
                        !related(at, &key),
                        "adapter {shown}: `{inner}` is inside `{outer}` — declare the table or \
                         its keys, not both"
                    );
                }
                seen.push((raw.as_str(), key));
            }
        }
        Ok(())
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
/// purpose: reusing the guest home here would send `omh settings mcp import`
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
            ["claude", "codex", "omp", "opencode"]
        );
    }

    /// A declared version must reach the install command, and a `{{version}}`
    /// must have a version to fill it. Both refusals, and the two that pass.
    #[test]
    fn a_version_that_never_reaches_the_install_command_is_refused() {
        let mut a = Adapter::find(Path::new(REAL), "claude").unwrap();

        a.version = Some("9.9.9".into());
        a.install = "npm install -g claude@{{version}}".into();
        assert_eq!(a.install_command().unwrap(), "npm install -g claude@9.9.9");

        a.install = "npm install -g claude".into();
        assert!(
            a.install_command().is_err(),
            "a version nothing interpolates is a pin that does nothing"
        );

        a.version = None;
        a.install = "npm install -g claude@{{version}}".into();
        assert!(
            a.install_command().is_err(),
            "a `{{version}}` with no version installs a package called `{{version}}`"
        );

        a.install = "npm install -g claude".into();
        assert_eq!(a.install_command().unwrap(), "npm install -g claude");
    }

    /// Every bundled adapter pins the harness it installs, and the pin reaches
    /// the command. Read from the shipped files, not from a fixture.
    #[test]
    fn every_bundled_adapter_pins_the_harness_it_installs() {
        for adapter in Adapter::load_dir(Path::new(REAL)).unwrap() {
            assert!(
                adapter.version.is_some(),
                "{} installs an unpinned harness",
                adapter.name
            );
            let command = adapter.install_command().unwrap();
            assert!(
                command.contains(adapter.version.as_deref().unwrap()),
                "{}: the pin does not reach the install command: {command}",
                adapter.name
            );
        }
    }

    /// An install that checks a sum but declares none is refused as it loads —
    /// a `sha256sum -c` against sums that are not there fails, or checks a file
    /// against itself.
    #[test]
    fn an_install_that_checks_a_sum_must_declare_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("x.toml"),
            "name = \"x\"\nbin = \"x\"\ninstall = \"curl -o x url && sha256sum -c && chmod +x x\"\n",
        )
        .unwrap();
        let err = Adapter::load(&d.path().join("x.toml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("verifies a checksum but declares none"),
            "got: {err:#}"
        );
    }

    /// The sums the omp install verifies inline are the ones the `[checksums]`
    /// map declares, arch for arch — so the map `omh doctor` reads and the
    /// value `sha256sum -c` checks cannot silently disagree, and the arch
    /// mapping is not transposed. The install labels arches `x64`/`arm64`; the
    /// map keys them by `uname -m` (`x86_64`/`aarch64`).
    #[test]
    fn the_omp_inline_sums_match_the_declared_map() {
        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        let command = omp.install_command().unwrap();
        for (uname, _label) in [("x86_64", "x64"), ("aarch64", "arm64")] {
            let sum = omp
                .checksums
                .get(uname)
                .unwrap_or_else(|| panic!("no sum for {uname}"));
            assert!(
                command.contains(sum.as_str()),
                "the map's {uname} sum does not appear in the install command"
            );
        }
        // And the two arches carry different sums, so a match is not accidental.
        assert_ne!(omp.checksums["x86_64"], omp.checksums["aarch64"]);
    }

    /// omp is a release binary, so its install verifies the download against a
    /// per-arch checksum before it makes it executable — a `chmod +x` on an
    /// unverified download is exactly the supply-chain gap a pin is meant to
    /// close.
    #[test]
    fn an_omp_install_verifies_before_it_chmods() {
        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        let command = omp.install_command().unwrap();
        let verify = command.find("sha256sum -c").expect("it checks a sum");
        let chmod = command.find("chmod +x").expect("it makes omp executable");
        assert!(
            verify < chmod,
            "the check comes before the chmod: {command}"
        );
        assert!(
            !omp.checksums.is_empty(),
            "and the sums it checks are declared: {:?}",
            omp.checksums
        );
    }

    /// Every path here is a claim about oh-my-pi, read out of its own source at
    /// the version this adapter was written against — `v17.3.3`, re-read at
    /// `v18.2.3` and `v18.10.4` for the bumps — tag-pinned
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
        // `write` rewrites a file whole, so a guard on edits that a write walks
        // past guards nothing — `a_guard_on_edits_sees_a_write`.
        assert_eq!(omp.tools[&crate::hook::Tool::Edit], "edit|write");
        assert!(
            !omp.tools.contains_key(&crate::hook::Tool::Search),
            "omp has grep and glob as separate tools; half a claim is worse than none"
        );
    }

    /// Claude Code has no search tool to name either, since the native builds.
    ///
    /// They replaced `Grep` and `Glob` with an embedded `ugrep` and `bfs`
    /// the agent reaches through `Bash`, and npm now installs that native
    /// binary. `search = "Grep|Glob"` matched nothing from then on, so the
    /// hooks narrowing to `search` shipped and never fired. Absent, they are
    /// dropped by name, which is the honest version of the same outcome.
    #[test]
    fn claude_spells_no_search_tool_it_does_not_have() {
        let claude = Adapter::find(Path::new(REAL), "claude").unwrap();
        assert!(
            !claude.tools.contains_key(&crate::hook::Tool::Search),
            "native Claude Code searches through Bash; `Grep|Glob` never matches"
        );
    }

    /// `verify` without `ready`, or `ready` without `verify`, is a check
    /// `omh doctor` skips without a word — it runs only when both are set.
    /// And codex's pair is the one measured: `codex mcp list` reports a server
    /// its config holds as `enabled`.
    #[test]
    fn verify_and_ready_come_as_a_pair() {
        for adapter in Adapter::load_dir(Path::new(REAL)).unwrap() {
            for (cap, binding) in &adapter.capabilities {
                assert_eq!(
                    binding.verify.is_some(),
                    binding.ready.is_some(),
                    "{} {cap:?}: half a check is no check",
                    adapter.name
                );
            }
        }
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        let mcp = codex.supports(Capability::Mcp).expect("codex has mcp");
        assert_eq!(mcp.verify.as_deref(), Some("codex mcp list"));
        assert_eq!(mcp.ready.as_deref(), Some("enabled"));
    }

    /// Codex writes a rollout per session under `$CODEX_HOME/sessions`, dated
    /// directories deep — measured in 0.154.0 as
    /// `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl` — and `omh sNN` reads
    /// what it can only from a transcripts mount.
    #[test]
    fn codex_transcripts_are_where_codex_writes_them() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        assert_eq!(codex.transcripts.as_deref(), Some("$HOME/.codex/sessions"));
    }

    /// Codex's skills land beside its own, not over them.
    ///
    /// Codex installs its system skills into `~/.codex/skills/.system`, and a
    /// read-only mount over `~/.codex/skills` hid them without a word —
    /// measured with `codex debug prompt-input` in the 0.154.0 image. The
    /// assertion is on the invariant, so a later path that reintroduces it
    /// under another spelling fails too.
    #[test]
    fn codex_skills_never_cover_codexs_own() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        let skills = codex
            .supports(Capability::Skills)
            .expect("codex has skills");
        let path = crate::adapter::expand(&skills.path, "/home/agent");
        assert!(
            !Path::new("/home/agent/.codex/skills/.system").starts_with(&path),
            "{} covers Codex's system skills",
            path.display()
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

    /// A policy lives where no session writes, and a mask hides something in
    /// the harness's home — the one place a session's write reaches the later
    /// sessions of every repo. Neither walks with `..`: both are joined into
    /// host paths.
    #[test]
    fn a_policy_a_session_could_reach_is_refused() {
        // Each rule refuses in words no other rule uses — a shared message
        // would let any one rule stand in for all.
        const CREDS: &str = "creds = [\"$HOME/.p/\"]";
        for (top, policy, names) in [
            (CREDS, "path = \"etc/x.toml\"", "must be absolute"),
            (CREDS, "path = \"$HOME/x.toml\"", "must be absolute"),
            (CREDS, "path = \"/etc/../work/x.toml\"", "no `..`"),
            (CREDS, "path = \"/work/x.toml\"", "a file under /etc/"),
            (CREDS, "path = \"/work\"", "a file under /etc/"),
            (CREDS, "path = \"/home/agent/x.toml\"", "a file under /etc/"),
            (CREDS, "path = \"/omh/memory/x.toml\"", "a file under /etc/"),
            (CREDS, "path = \"/etc\"", "a file under /etc/"),
            (
                "creds = [\"/etc/p/\"]",
                "path = \"/etc/p/x.toml\"",
                "under a path omh mounts writable",
            ),
            (
                "transcripts = \"/etc/p\"",
                "path = \"/etc/p/x.toml\"",
                "under a path omh mounts writable",
            ),
            (
                CREDS,
                "path = \"/etc/x.toml\"\n\n[capabilities.mcp]\npath = \"/etc/x.toml\"\nrender = \"codex-toml\"",
                "is a capability's path too",
            ),
            (
                CREDS,
                "path = \"/etc/x.toml\"\nmask = [\"/tmp/cache\"]",
                "strictly inside one of this adapter's credential directories",
            ),
            (
                CREDS,
                "path = \"/etc/x.toml\"\nmask = [\"$HOME/elsewhere/cache\"]",
                "strictly inside one of this adapter's credential directories",
            ),
            (
                CREDS,
                "path = \"/etc/x.toml\"\nmask = [\"$HOME/.p\"]",
                "strictly inside one of this adapter's credential directories",
            ),
            (
                CREDS,
                "path = \"/etc/x.toml\"\nmask = [\"$HOME/.p/../../tmp/x\"]",
                "mask entry \"$HOME/.p/../../tmp/x\" has a `..`",
            ),
            (
                "creds = [\"$HOME/.p/\", \"$HOME/.p.json\"]",
                "path = \"/etc/x.toml\"\ncopy = [\"$HOME/.p/\"]",
                "copy entry \"$HOME/.p/\" must be one of this adapter's credential files",
            ),
            (
                "creds = [\"$HOME/.p/\", \"$HOME/.p.json\"]",
                "path = \"/etc/x.toml\"\ncopy = [\"$HOME/.other.json\"]",
                "copy entry \"$HOME/.other.json\" must be one of this adapter's credential files",
            ),
            // A copy is seeded by `render::scrub`, which knows one file per
            // format: a credential file it does not know — or any for a
            // format that copies nothing — has no seed, and an absolute one
            // would not even be the session's.
            (
                "creds = [\"$HOME/.p/\", \"$HOME/.p.json\"]",
                "format = \"codex-requirements\"\npath = \"/etc/x.toml\"\ncopy = [\"$HOME/.p.json\"]",
                "is not a file codex-requirements knows how to seed",
            ),
            (
                "creds = [\"$HOME/.p/\", \"$HOME/.p.json\"]",
                "path = \"/etc/x.toml\"\ncopy = [\"$HOME/.p.json\"]",
                "is not a file claude-managed knows how to seed",
            ),
            (
                "creds = [\"$HOME/.p/\", \"/abs/.claude.json\"]",
                "path = \"/etc/x.toml\"\ncopy = [\"/abs/.claude.json\"]",
                "is not a file claude-managed knows how to seed",
            ),
        ] {
            let d = tempfile::tempdir().unwrap();
            let format = match policy.contains("format =") {
                true => "",
                false => "format = \"claude-managed\"\n",
            };
            write(
                d.path(),
                "p.toml",
                &format!(
                    "name = \"p\"\nbin = \"p\"\ninstall = \"true\"\n{top}\n\n\
                     [capabilities.rules]\npath = \"/work/AGENTS.md\"\nrender = \"concat\"\n\n\
                     [policy]\n{format}{policy}\n"
                ),
            );
            let err = Adapter::find(d.path(), "p").unwrap_err().to_string();
            assert!(err.contains(names), "{top} / {policy}: {err}");
        }
    }

    /// A provider render hides a file the account keeps, so it lands strictly
    /// inside a `creds` directory, where `auth::prepare` lays its mountpoint;
    /// so does every file dropped before a launch. Neither walks with `..`, and
    /// an import reads the user's own home.
    #[test]
    fn a_provider_omh_could_not_mount_or_drop_safely_is_refused() {
        // Each rule refuses in words no other rule uses.
        for (provider, names) in [
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"",
                "omp-models needs a `path`",
            ),
            (
                "format = \"codex-config\"\nimport = \"$HOME/.p/c.toml\"\npath = \"$HOME/.p/m.yml\"",
                "codex-config takes no `path`",
            ),
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"\npath = \"/etc/m.yml\"",
                "`[provider] path = \"/etc/m.yml\"` must be strictly inside",
            ),
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"\npath = \"$HOME/.p\"",
                "`[provider] path = \"$HOME/.p\"` must be strictly inside",
            ),
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"\npath = \"$HOME/.p/../x\"",
                "`[provider] path = \"$HOME/.p/../x\"` has a `..`",
            ),
            (
                "format = \"omp-models\"\nimport = \"/etc/m.yml\"\npath = \"$HOME/.p/m.yml\"",
                "`[provider] import = \"/etc/m.yml\"` must be under $HOME/",
            ),
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"\npath = \"$HOME/.p/m.yml\"\n\
                 drop = [\"$HOME/elsewhere/c.db\"]",
                "drop entry \"$HOME/elsewhere/c.db\" must be strictly inside",
            ),
            (
                "format = \"omp-models\"\nimport = \"$HOME/.p/m.yml\"\npath = \"$HOME/.p/m.yml\"\n\
                 drop = [\"$HOME/.p/../c.db\"]",
                "drop entry \"$HOME/.p/../c.db\" has a `..`",
            ),
        ] {
            let d = tempfile::tempdir().unwrap();
            write(
                d.path(),
                "p.toml",
                &format!(
                    "name = \"p\"\nbin = \"p\"\ninstall = \"true\"\ncreds = [\"$HOME/.p/\"]\n\n\
                     [capabilities.rules]\npath = \"/work/AGENTS.md\"\nrender = \"concat\"\n\n\
                     [provider]\n{provider}\n"
                ),
            );
            let err = Adapter::find(d.path(), "p").unwrap_err().to_string();
            assert!(err.contains(names), "{provider}: {err}");
        }
    }

    /// Codex and omp each take a provider file, imported from where the
    /// harness keeps its own on the host; omp's is rendered over the file it
    /// reads providers from, and its model cache — which can carry a
    /// provider's endpoint (`model-registry.ts`, v18.10.4) — is dropped. Claude
    /// and opencode take none yet (#197 §6, "after").
    #[test]
    fn the_shipped_providers_are_where_each_harness_reads_its_own() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        let p = codex
            .provider
            .as_ref()
            .expect("codex takes a provider file");
        assert_eq!(p.format, ProviderFormat::CodexConfig);
        assert_eq!(p.import, "$HOME/.codex/config.toml");
        assert!(p.path.is_none() && p.drop.is_empty());

        let omp = Adapter::find(Path::new(REAL), "omp").unwrap();
        let p = omp.provider.as_ref().expect("omp takes a provider file");
        assert_eq!(p.format, ProviderFormat::OmpModels);
        assert_eq!(p.path.as_deref(), Some("$HOME/.omp/agent/models.yml"));
        assert_eq!(p.import, "$HOME/.omp/agent/models.yml");
        for db in ["models.db", "models.db-wal", "models.db-shm"] {
            assert!(
                p.drop.contains(&format!("$HOME/.omp/agent/{db}")),
                "{db}: {:?}",
                p.drop
            );
        }

        for name in ["claude", "opencode"] {
            let a = Adapter::find(Path::new(REAL), name).unwrap();
            assert!(a.provider.is_none(), "{name}");
        }
    }

    /// Codex's preferences are the issue's list, explicit, each a key Codex
    /// 0.162.1 reads (its source), none of them a program, a consent, a trust
    /// or text before the model — and the two tables declared whole are named
    /// here, so a pin bump's diff touches them and they are read again.
    #[test]
    fn codexs_preferences_are_its_own_list_and_never_a_never_key() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        let prefs = codex
            .preferences
            .as_ref()
            .expect("codex declares preferences");
        assert_eq!(prefs.mount, PrefMount::Link);
        let declared = prefs.declared();
        let [file] = declared.as_slice() else {
            panic!("one file: {declared:?}");
        };
        assert_eq!(file.path, "$HOME/.codex/config.toml");
        assert_eq!(file.name, "config.toml");
        let dotted = |k: &KeyPath| k.join(".");
        let keys: Vec<String> = file.keys.iter().map(dotted).collect();
        assert!(keys.contains(&"model".to_string()), "{keys:?}");
        let quiet: Vec<String> = file.quiet.iter().map(dotted).collect();
        assert!(
            quiet.contains(&"notice.hide_gpt-5.1-codex-max_migration_prompt".to_string()),
            "{quiet:?}"
        );
        for key in file.keys.iter().chain(&file.quiet) {
            for never in never_for("codex") {
                assert!(
                    !related(key, &never),
                    "{} is near never-key {}",
                    dotted(key),
                    never.join(".")
                );
            }
        }
        const WHOLE_TABLES: &[&str] = &["tui.keymap", "notice.model_migrations"];
        for table in WHOLE_TABLES {
            assert!(
                keys.iter().chain(&quiet).any(|k| k == table),
                "{table} is no longer declared — drop it from this list"
            );
        }
    }

    /// What a preference never is, refused by name at load — and the shapes a
    /// preferences table cannot take, each in words of its own.
    #[test]
    fn a_preference_that_is_not_one_is_refused_by_name() {
        const CREDS: &str = "creds = [\"$HOME/.codex/\"]";
        const FILE: &str = "\"$HOME/.codex/config.toml\"";
        for (top, body, names) in [
            (CREDS, format!("[preferences.files]\n{FILE} = [\"developer_instructions\"]"), "`developer_instructions` is never a preference"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"mcp_servers.x\"]"), "`mcp_servers.x` is never a preference"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"notice\"]"), "`notice` is never a preference"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"model\"]\n[preferences.quiet]\n{FILE} = [\"notice.hide_full_access_warning\"]"), "`notice.hide_full_access_warning` is never a preference"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"a..b\"]"), "`a..b` is not a key"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"tui.\\\"x\"]"), "is not a key"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"tui\", \"tui.theme\"]"), "`tui.theme` is inside `tui`"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"model\"]\n[preferences.quiet]\n{FILE} = [\"model\"]"), "`model` is declared twice"),
            (CREDS, format!("[preferences.files]\n{FILE} = [\"model\"]\n[preferences.quiet]\n\"$HOME/.codex/other.toml\" = [\"x\"]"), "quiet file \"$HOME/.codex/other.toml\" is not one of"),
            (CREDS, "[preferences.files]\n\"$HOME/elsewhere.toml\" = [\"model\"]".to_string(), "strictly inside one of this adapter's credential directories"),
            (CREDS, "[preferences.files]\n\"$HOME/.codex/x/../../y.toml\" = [\"model\"]".to_string(), "has a `..`"),
            (CREDS, "[preferences.files]\n\"$HOME/.codex/config.json\" = [\"model\"]".to_string(), "only TOML"),
            (CREDS, format!("[preferences.files]\n{FILE} = []"), "declares no keys"),
        ] {
            let d = tempfile::tempdir().unwrap();
            write(
                d.path(),
                "codex.toml",
                &format!(
                    "name = \"codex\"\nbin = \"codex\"\ninstall = \"true\"\n{top}\n\n\
                     [capabilities.rules]\npath = \"/work/AGENTS.md\"\nrender = \"concat\"\n\n\
                     [preferences]\nmount = \"link\"\n\n{body}\n"
                ),
            );
            let err = Adapter::find(d.path(), "codex").unwrap_err().to_string();
            assert!(err.contains(names), "{body}: {err}");
        }
    }

    #[test]
    fn a_key_path_reads_as_toml_spells_it() {
        assert_eq!(key_path("model").unwrap(), vec!["model"]);
        assert_eq!(key_path("tui.theme").unwrap(), vec!["tui", "theme"]);
        assert_eq!(
            key_path("notice.\"hide_gpt-5.1-codex-max_migration_prompt\"").unwrap(),
            vec!["notice", "hide_gpt-5.1-codex-max_migration_prompt"]
        );
        assert_eq!(key_path("a.'b.c'").unwrap(), vec!["a", "b.c"]);
        for bad in [
            "",
            ".",
            "a.",
            ".a",
            "a..b",
            "a b",
            "\"x",
            "a.\"\"",
            "a.\"b\\\"c\"",
        ] {
            assert!(key_path(bad).is_err(), "{bad:?}");
        }
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

    /// Codex renders its MCP servers into the `[mcp_servers.<name>]` TOML its
    /// config uses, the shape the adapter declares — not the JSON the others
    /// write.
    #[test]
    fn codex_mcp_renders_to_the_shape_the_adapter_declares() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        let binding = codex.supports(Capability::Mcp).unwrap();
        assert_eq!(binding.render, Render::CodexToml, "codex config is TOML");
        assert!(
            binding.path.ends_with(".toml"),
            "into a config file codex reads: {}",
            binding.path
        );
    }

    /// No file is mounted inside `~/.codex`. Codex rewrites
    /// `~/.codex/config.toml` itself — a directory's trust, a notice it was
    /// told to hide, the model picked — by writing a temporary file and
    /// renaming it over, which a bind-mounted file refuses. The interactive
    /// app failed to start with "failed to persist config.toml" until the
    /// MCP document moved out.
    #[test]
    fn nothing_is_mounted_over_a_file_codex_rewrites() {
        let codex = Adapter::find(Path::new(REAL), "codex").unwrap();
        for (cap, binding) in &codex.capabilities {
            if binding.render == Render::Dir {
                continue;
            }
            for target in std::iter::once(&binding.path).chain(&binding.also) {
                assert!(
                    !expand(target, "/home/agent").starts_with("/home/agent/.codex"),
                    "{cap:?} mounts a file at {target}, inside the directory Codex rewrites"
                );
            }
        }
    }

    #[test]
    fn shipped_adapters_declare_where_to_import_mcp_from() {
        for name in ["claude", "codex", "opencode"] {
            let a = Adapter::find(Path::new(REAL), name).unwrap();
            assert!(
                a.supports(Capability::Mcp).unwrap().import.is_some(),
                "{name} must say where `omh settings mcp import` should look"
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
        for name in ["claude", "codex", "opencode"] {
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
