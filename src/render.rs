//! The only place a harness difference costs more than a bind mount.
//!
//! A capability is declared once in canonical form and reshaped into whatever
//! the target harness parses. This is how `omh-mcp` (memory) and the wired
//! code-graph server reach every harness without being configured twice.

use crate::adapter::{Binding, Capability, Render};
use crate::hook::{self, Outcome};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A rendered capability, and what did not fit in it.
///
/// Dropping used to be all-or-nothing — a harness expressed a capability or it
/// did not — and hooks broke that: a harness can have `turn-end` and no
/// `before-tool`, so some hooks ship and some cannot. A count of capabilities
/// cannot say that, and a hook silently missing is a hook whose absence looks
/// exactly like working.
#[derive(Debug, Default)]
pub struct Document {
    pub body: String,
    pub dropped: Vec<hook::Dropped>,
}

impl From<String> for Document {
    fn from(body: String) -> Self {
        Self {
            body,
            dropped: Vec::new(),
        }
    }
}

/// Render a capability into the shape this harness parses.
///
/// Two inputs from outside, and they are deliberately two: `own` is what omh
/// itself contributes, `repo` is what this checkout decided. Neither is a
/// layer — omh's hooks belong to no directory, and a server whose feature is
/// disabled here is still in your `mcp.json`, which is yours and is left
/// exactly as you have it.
pub fn document(
    cap: Capability,
    binding: &Binding,
    sources: &[PathBuf],
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
    tools: &BTreeMap<hook::Tool, String>,
) -> Result<Document> {
    match binding.render {
        Render::McpJson | Render::CodexToml | Render::OpencodeJson => {
            let mut servers = merge_servers(sources)?;
            // Checked against the catalogue as written, *before* the disabled
            // ones are dropped. Checked after, an override for a feature this
            // repo switched off reported "not in your catalogue" about a
            // server that is plainly in it — advice pointing at
            // `omh config mcp ls`, which lists it, and no way forward.
            for name in repo.mcp_env.keys() {
                if !servers.contains_key(name) {
                    anyhow::bail!(
                        "[mcp.{name}.env] overrides a server that is not in your \
                         catalogue — nothing would read it. `omh config mcp ls` \
                         lists what is there."
                    );
                }
            }
            // Two retains, deliberately not one condition: a server is dropped
            // because its feature is off *here*, or because this repo never
            // named it, and those are different sentences to say when somebody
            // asks why a server is missing.
            servers.retain(|name, _| !repo.disabled_servers.contains(name));
            servers.retain(|name, _| repo.selection.allows(Capability::Mcp, name));
            // Variable by variable, not entry by entry: a repo overriding one
            // token must not silently inherit the rest of a catalogue entry it
            // never saw. Named where it is applied rather than merged into the
            // sources, so `omh config` still shows the catalogue as written.
            //
            // A server whose feature is off here is simply gone by now, so its
            // override is a no-op rather than an error: switching a feature off
            // is not a reason to make you delete a token you will want back.
            for (name, env) in &repo.mcp_env {
                if let Some(server) = servers.get_mut(name) {
                    server.env.extend(env.clone());
                }
            }
            Ok(mcp(binding.render, &servers)?.into())
        }
        Render::ClaudeSettings => {
            let (rendered, dropped) = translate(&merge_hooks(sources, own, repo)?, binding, tools)?;
            Ok(Document {
                body: claude_settings(&rendered)?,
                dropped,
            })
        }
        Render::OpencodePlugin => {
            opencode_plugin(&merge_hooks(sources, own, repo)?, binding, tools)
        }
        Render::Dir | Render::Concat => {
            anyhow::bail!(
                "{cap}: `{:?}` is staged by the launcher, not rendered",
                binding.render
            )
        }
    }
}

// ── MCP ─────────────────────────────────────────────────────────────────────

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
    merge_servers(files)
}

fn merge_servers(files: &[PathBuf]) -> Result<BTreeMap<String, Server>> {
    let mut out = BTreeMap::new();
    for f in files {
        let parsed: CanonicalMcp = read_json(f)?;
        out.extend(parsed.servers);
    }
    Ok(out)
}

fn mcp(render: Render, servers: &BTreeMap<String, Server>) -> Result<String> {
    match render {
        Render::McpJson => pretty(serde_json::json!({ "mcpServers": servers })),
        Render::OpencodeJson => {
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
            pretty(serde_json::json!({
                "$schema": "https://opencode.ai/config.json",
                "mcp": mcp
            }))
        }
        Render::CodexToml => {
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
        _ => unreachable!("caller matched on MCP renders"),
    }
}

/// Inverse of `mcp`: read a harness's own config back into canonical form.
///
/// `omh mcp import` exists because nobody retypes MCP servers they already have.
/// Every format that renders must also parse, and the pair must round-trip —
/// otherwise import silently drops fields the renderer knows how to write.
pub fn parse(format: Render, raw: &str) -> Result<BTreeMap<String, Server>> {
    match format {
        Render::McpJson => {
            let doc: serde_json::Value = serde_json::from_str(raw).context("parsing MCP JSON")?;
            if doc.get("mcpServers").is_none() && doc.get("projects").is_some() {
                anyhow::bail!(
                    "this config nests servers under `projects` — importing all of \
                     them would pull in servers from unrelated repos. Point --file \
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

// ── Hooks ───────────────────────────────────────────────────────────────────

/// Every hook, translated into this harness's words. A hook it cannot spell is
/// dropped by name rather than taking the capability with it.
fn translate(
    hooks: &BTreeMap<String, hook::Hook>,
    binding: &Binding,
    tools: &BTreeMap<hook::Tool, String>,
) -> Result<(BTreeMap<String, hook::Rendered>, Vec<hook::Dropped>)> {
    let mut rendered = BTreeMap::new();
    let mut dropped = Vec::new();
    for (name, h) in hooks {
        match hook::render(name, h, binding, tools)? {
            Outcome::Rendered(r) => {
                rendered.insert(name.clone(), r);
            }
            Outcome::Dropped(d) => dropped.push(d),
        }
    }
    Ok((rendered, dropped))
}

/// Union by name across the catalogue and the repo; the repo's shadow yours.
///
/// A file answering to a manifest name is an **error naming both** — see
/// `Own::reserved`. Read-and-then-override would not be enough even if it were
/// wanted: with the feature off there is nothing to override it with, so the
/// file would simply go on running.
///
/// omh's own are inserted last, but that ordering is not what makes them win.
/// They are generated from the manifest and belong to no directory, which is
/// the point: a hook you can edit is a hook omh can never ship a fix to, and
/// `git-unavailable` has already needed one.
fn merge_hooks(
    dirs: &[PathBuf],
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
) -> Result<BTreeMap<String, hook::Hook>> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        // Absent is not unreadable — `config::read_layer` records what
        // conflating them cost, and `config::hooks` reads these same
        // directories the careful way. The two disagreeing meant `omh why`
        // errored on a `chmod 000` hooks directory while a launch shipped a
        // session without those hooks and said nothing. Generation made that
        // quieter, not louder: omh's own are merged in afterwards, so the
        // document is never empty and `omh doctor`'s hooks check passes while
        // the user's whole layer is missing.
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            // Not `.flatten()`. `readdir` can fail part-way through — a network
            // or synced mount dropping out, a disk erroring — and skipping the
            // entry ships a session missing a hook, with the document still
            // well-formed because omh's own are merged in afterwards. A hook
            // that is not there behaves exactly like one with nothing to say.
            let path = entry
                .with_context(|| format!("reading {}", dir.display()))?
                .path();
            if path.extension().is_some_and(|e| e == "json") {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                // A manifest name is omh's, on or off — an error naming both
                // rather than an override. A repo that could replace
                // `graph-refresh` could make the graph lie while looking
                // installed: the server answers, the index never updates, and
                // every structural answer is about code the agent has since
                // rewritten.
                //
                // Read-and-then-override would not be enough even if it were
                // wanted: with the feature off there is nothing to override the
                // file with, so it would go on running.
                if own.reserved.contains(&name) {
                    anyhow::bail!(
                        "{}: `{name}` is a name omh ships, so this file answers to \
                         nothing — it is not read, and it does not override omh's. \
                         Rename it, or switch the feature off with `[omh]` in \
                         .omh/settings.toml if what you want is omh's gone.",
                        path.display()
                    );
                }
                // Checked *after* the reserved-name guard, so a repo that
                // shipped a `graph-refresh.json` still hears about it whether
                // or not `[use].hooks` happens to name it. A file that answers
                // to nothing is a mistake at any selection.
                if !repo.selection.allows(Capability::Hooks, &name) {
                    continue;
                }
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.insert(name, hook::Hook::parse(&raw, &path.display().to_string())?);
            }
        }
    }
    // omh's own, merged in unfiltered. They are a feature's parts, and `[omh]`
    // has already had its say by deciding which ones `own.hooks` holds — a
    // `[use].hooks` that could drop one would be a feature taken apart by the
    // table that is not allowed to.
    for own_hook in &own.hooks {
        out.insert(own_hook.name.to_string(), own_hook.hook.clone());
    }
    Ok(out)
}

const BEFORE_TOOL: &str = "tool.execute.before";
const AFTER_TOOL: &str = "tool.execute.after";
/// The catch-all. Fires for every message on the bus, hence the type test.
const BUS: &str = "event";

/// Where a moment's handler puts things, which is not the same at every moment.
///
/// opencode knowledge, in the opencode renderer, on purpose: an adapter says
/// which *word* this harness uses for a moment, and only the renderer can know
/// what that word implies about the handler it lands in.
///
/// Two things vary and neither is guessable from the name:
///
/// - **whether a call is in scope at all.** `event` is handed the bus message
///   and nothing else, so `output` is an *undeclared identifier* there — a
///   `ReferenceError`, which optional chaining does not prevent, and which takes
///   every other hook in the handler with it.
/// - **which parameter holds the arguments.** From the binary,
///   `tool.execute.before` is triggered as `(…, {args})` and
///   `tool.execute.after` as `({…, args}, result)`. They move. Reading
///   `output.args` at `after` binds the empty string and the hook silently
///   never fires.
#[derive(Clone, Copy)]
enum Slot<'a> {
    /// A plugin hook: `(input, output)`, with a call in scope. `args` is the
    /// parameter this moment keeps the tool's arguments on.
    Call { hook: &'a str, args: &'static str },
    /// The catch-all: `(input)` only, and no call to speak of.
    Bus { ty: &'a str },
}

impl<'a> Slot<'a> {
    fn of(event: &'a str) -> Self {
        match event {
            BEFORE_TOOL => Slot::Call {
                hook: BEFORE_TOOL,
                args: "output",
            },
            AFTER_TOOL => Slot::Call {
                hook: AFTER_TOOL,
                args: "input",
            },
            ty => Slot::Bus { ty },
        }
    }

    /// The handler this hook is emitted into.
    fn handler(&self) -> &'a str {
        match self {
            Slot::Call { hook, .. } => hook,
            Slot::Bus { .. } => BUS,
        }
    }

    /// What that handler is handed. The bus one has no `output`, which is the
    /// whole reason this type exists.
    fn args(&self) -> &'static str {
        match self {
            Slot::Call { .. } => "(input, output)",
            Slot::Bus { .. } => "(input)",
        }
    }
}

/// Generate the plugin module opencode loads.
///
/// A hook's bodies — `when`, `capture`, `run` — are shell in the canonical
/// format and stay shell here; this module is glue that runs them and applies
/// what they say. That is what keeps the format harness-neutral: the generated
/// file is the only thing in omh that knows TypeScript.
///
/// **Advisory text has no channel at `before-tool` on this harness.** Verified
/// in a container: `tool.execute.before` receives `(input, output)`, mutating
/// `output.args` changes the call, and the only way to reach the model is to
/// `throw` — which blocks. So an `inject` on that moment is dropped by name
/// rather than promoted to a refusal, and `graph-first`'s "a nudge, not a wall"
/// survives contact with a second harness.
fn opencode_plugin(
    hooks: &BTreeMap<String, hook::Hook>,
    binding: &Binding,
    tools: &BTreeMap<hook::Tool, String>,
) -> Result<Document> {
    let mut dropped = Vec::new();
    // Keyed by plugin hook name, so several omh hooks sharing a moment land in
    // one handler, in a stable order.
    let mut bodies: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    for (name, hook) in hooks {
        let wired = match hook::wire(name, hook, binding, tools) {
            Ok(wired) => wired,
            Err(d) => {
                dropped.push(d);
                continue;
            }
        };
        let give_up = |wanted: &str| hook::Dropped {
            name: name.clone(),
            wanted: wanted.to_string(),
        };
        let slot = Slot::of(wired.event);
        // A moment with no call in scope can express none of the things that
        // need one. Emitting them anyway referenced an undeclared `output` —
        // a `ReferenceError` that takes every other hook in the handler with
        // it — or, for a tool guard, tested a field that is never there and so
        // never fired at all. Named, which is what this renderer does with
        // everything else it cannot say.
        if let Slot::Bus { .. } = slot {
            let needs = match &hook.action {
                _ if !wired.fields.is_empty() => Some("payload field"),
                _ if !wired.tools.is_empty() => Some("way to narrow to a tool"),
                hook::Action::Inject { .. } => Some("way to inject text"),
                hook::Action::Refuse { .. } => Some("way to refuse a call"),
                hook::Action::Run(_) => None,
            };
            if let Some(needs) = needs {
                dropped.push(give_up(&format!("{needs} at `{}`", hook.on)));
                continue;
            }
        }
        // Advisory text has no channel before the call on this harness: the
        // only way to speak there is a throw, which blocks. Checked before the
        // binding is asked, because the binding *can* advise — just not here.
        if matches!(hook.action, hook::Action::Inject { .. })
            && matches!(
                slot,
                Slot::Call {
                    hook: BEFORE_TOOL,
                    ..
                }
            )
        {
            dropped.push(give_up("way to inject text before a tool runs"));
            continue;
        }
        let protocol = match binding.protocol(&hook.action) {
            Ok(p) => p,
            Err(wanted) => {
                dropped.push(give_up(wanted));
                continue;
            }
        };
        bodies
            .entry(slot.handler())
            .or_default()
            .push(one_hook(name, hook, &wired, slot, protocol));
    }

    let mut out = String::from(PLUGIN_PREAMBLE);
    for (handler, blocks) in &bodies {
        // Every hook in a handler shares its parameter list, which is why the
        // drop above has to happen before a hook reaches one.
        let args = Slot::of(handler).args();
        out.push_str(&format!("  {handler:?}: async {args} => {{\n"));
        for block in blocks {
            out.push_str(block);
        }
        out.push_str("  },\n");
    }
    out.push_str("}))\n");
    Ok(Document { body: out, dropped })
}

/// One hook, as a block inside its handler.
fn one_hook(
    name: &str,
    hook: &hook::Hook,
    wired: &hook::Wired<'_>,
    slot: Slot<'_>,
    protocol: Option<&crate::adapter::Template>,
) -> String {
    // Each hook gets a function of its own. A bare block does not scope a
    // `return`, so a hook whose tool guard did not match would leave the whole
    // handler and cancel every hook after it — `git-unavailable` narrowing to
    // `bash` silently cancelled the rest on every call that was not bash.
    let mut b = format!("    // {name}\n    await (async () => {{\n");
    // The catch-all fires for everything on the bus, so the type test is what
    // stops `graph-refresh` re-indexing on every message part.
    if let Slot::Bus { ty } = slot {
        b.push_str(&format!(
            "      if (input?.event?.type !== {ty:?}) return\n"
        ));
    }
    if !wired.tools.is_empty() {
        let names = wired
            .tools
            .iter()
            .map(|t| format!("{t:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        b.push_str(&format!(
            "      if (![{names}].includes(input.tool)) return\n"
        ));
    }
    b.push_str("      const env = {}\n");
    // From the parameter this *moment* keeps them on — they are on `output` at
    // `before` and on `input` at `after`, and reading the wrong one binds the
    // empty string rather than failing.
    if let Slot::Call { args, .. } = slot {
        for (field, at) in &wired.fields {
            b.push_str(&format!(
                "      env[{:?}] = String({args}?.args?.{at} ?? \"\")\n",
                field.var()
            ));
        }
    }
    if let hook::Action::Inject {
        capture: Some(capture),
        ..
    } = &hook.action
    {
        b.push_str(&format!(
            "      const cap = sh({}, env)\n      if (!cap.ran || cap.code !== 0) warn({}, \"capture\", cap)\n      env[{:?}] = cap.out\n",
            js(capture),
            js(name),
            hook::CAPTURE_VAR,
        ));
    }
    if let Some(when) = &hook.when {
        b.push_str(&format!(
            "      const p = sh({}, env)\n      if (!p.ran || p.err) warn({}, \"its `when`\", p)\n      if (p.code !== 0) return\n",
            js(when),
            js(name),
        ));
    }
    match &hook.action {
        // The exit code is not a decision — `graph-refresh` ends in `|| true`
        // deliberately — but a `run` that could not start, or died, is worth
        // saying rather than discarding.
        hook::Action::Run(run) => b.push_str(&format!(
            "      const r = sh({}, env)\n      if (!r.ran || r.code !== 0) warn({}, \"its `run`\", r)\n",
            js(run),
            js(name),
        )),
        // The harness's protocol comes from the adapter, as it does for every
        // other render. `{{text}}` receives an expression evaluating to the
        // hook's text with its `$OMH_*` references expanded — which is what the
        // shell does for the same string on a config-shaped harness.
        hook::Action::Inject { text, .. } | hook::Action::Refuse { text } => {
            let template = protocol.map(|p| p.template.as_str()).unwrap_or_default();
            b.push_str(&format!(
                "      {}\n",
                template
                    .replace(
                        "{{text}}",
                        &format!(
                            "t({}, {}, {}, env)",
                            js(name),
                            js(text),
                            js(&hook::interpolating(text))
                        ),
                    )
                    .replace("{{event}}", wired.event)
            ));
        }
    }
    b.push_str("    })()\n");
    b
}

/// A Rust string as a JavaScript string literal. Through `serde_json` because
/// JSON string syntax is a subset of JavaScript's, and hand-rolling the escapes
/// for a hook body — which holds quotes, backslashes and newlines by design —
/// is how a generated program stops parsing.
fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The module's fixed head: opencode's plugin shape, plus the shell bridge.
///
/// `node:child_process` rather than the `$` helper opencode passes each plugin.
/// `$` is Bun's shell, and its behaviour around exit codes and quoting is a
/// claim about a runtime omh does not control; `spawnSync` behaves the same on
/// anything that can load this file at all.
const PLUGIN_PREAMBLE: &str = r#"// Generated by omh. Edits are overwritten at launch.
import { spawnSync } from "node:child_process"

// `ran` is false when the shell could not start or was killed by a signal —
// `spawnSync` reports both as `status: null`, which is indistinguishable from
// the `1` a predicate returns when it deliberately declines. Collapsing them
// meant a guard that could not be evaluated let the call through in silence.
const sh = (script, env) => {
  const r = spawnSync("sh", ["-c", script], {
    env: { ...process.env, ...env },
    encoding: "utf8",
  })
  return {
    ran: r.status !== null && r.status !== undefined,
    code: r.status ?? 1,
    out: (r.stdout ?? "").trim(),
    err: (r.stderr ?? "").trim() || String(r.error?.message ?? ""),
  }
}

// A hook still degrades to a no-op rather than to an error — the base set's own
// rule — but it says so. Silence is what a predicate means; it is not what a
// broken predicate means.
//
// A predicate is warned about when it wrote to stderr, not merely when it
// returned non-zero: `case … esac` declining is the normal path and says
// nothing, while a missing binary, a syntax error or a permission problem all
// leave a message. Exit status alone cannot tell those apart — a deliberate
// `false` and a `command not found` are both just non-zero.
const warn = (hook, phase, r) =>
  console.error(`omh: hook ${hook}: ${phase} did not run${r.err ? " — " + r.err : ""}`)

// A hook's text is written once, in omh's words, and may name the payload
// fields bound above it. Expanding it through the same shell keeps one meaning
// for `$OMH_TOOL_FILE` whether the harness takes configuration or code — but a
// blocked call with a blank reason is the worst of both states, so a failed
// expansion falls back to the text as written.
const t = (hook, raw, word, env) => {
  const r = sh("printf '%s' " + word, env)
  if (!r.ran || r.code !== 0) {
    warn(hook, "expanding its text", r)
    return raw
  }
  return r.out
}

export default (async () => ({
"#;

fn claude_settings(hooks: &BTreeMap<String, hook::Rendered>) -> Result<String> {
    let mut by_event: BTreeMap<&str, Vec<serde_json::Value>> = BTreeMap::new();
    for h in hooks.values() {
        by_event
            .entry(&h.event)
            .or_default()
            .push(serde_json::json!({
                "matcher": h.matcher,
                "hooks": [{ "type": "command", "command": h.command }],
            }));
    }
    pretty(serde_json::json!({ "hooks": by_event }))
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn pretty(v: serde_json::Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(&v)?)
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        write!(std::fs::File::create(&p).unwrap(), "{body}").unwrap();
        p
    }

    fn servers(json: &[(&str, &str)]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = json
            .iter()
            .enumerate()
            .map(|(i, (_, body))| file(dir.path(), &format!("l{i}.json"), body))
            .collect();
        (dir, paths)
    }

    const L1: &str = r#"{"mcpServers":{"a":{"command":"a-cmd","args":["--x"]}}}"#;
    const L2: &str = r#"{"mcpServers":{"b":{"command":"b-cmd","env":{"K":"v"}}}}"#;
    const L2_SHADOW: &str = r#"{"mcpServers":{"a":{"command":"overridden"}}}"#;

    #[test]
    fn mcp_merges_across_layers() {
        let (_d, files) = servers(&[("", L1), ("", L2)]);
        let merged = merge_servers(&files).unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged["a"].command, "a-cmd");
        assert_eq!(merged["b"].env["K"], "v");
    }

    #[test]
    fn later_layers_shadow_earlier_ones() {
        let (_d, files) = servers(&[("", L1), ("", L2_SHADOW)]);
        let merged = merge_servers(&files).unwrap();
        assert_eq!(
            merged["a"].command, "overridden",
            "layer 3 must win over layer 1"
        );
    }

    /// A wrong schema means the harness silently sees zero MCP servers, so each
    /// format asserts the shape the harness actually parses.
    #[test]
    fn each_format_emits_its_harness_shape() {
        let (_d, files) = servers(&[("", L1)]);
        let m = merge_servers(&files).unwrap();

        let claude = mcp(Render::McpJson, &m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&claude).unwrap();
        assert_eq!(v["mcpServers"]["a"]["command"], "a-cmd");

        let oc = mcp(Render::OpencodeJson, &m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&oc).unwrap();
        assert_eq!(v["mcp"]["a"]["type"], "local");
        // opencode folds args into a single command array
        assert_eq!(v["mcp"]["a"]["command"][0], "a-cmd");
        assert_eq!(v["mcp"]["a"]["command"][1], "--x");

        let codex = mcp(Render::CodexToml, &m).unwrap();
        assert!(codex.contains("[mcp_servers.a]"), "got: {codex}");
        assert!(codex.contains(r#"command = "a-cmd""#), "got: {codex}");
        let reparsed: toml::Value = codex.parse().expect("codex output must be valid TOML");
        assert_eq!(
            reparsed["mcp_servers"]["a"]["args"][0].as_str(),
            Some("--x")
        );
    }

    #[test]
    fn codex_toml_escapes_quotes() {
        let mut m = BTreeMap::new();
        m.insert(
            "q".to_string(),
            Server {
                command: r#"say "hi""#.into(),
                args: vec![],
                env: BTreeMap::new(),
            },
        );
        let out = mcp(Render::CodexToml, &m).unwrap();
        out.parse::<toml::Value>()
            .expect("must stay valid TOML when values contain quotes");
    }

    use crate::adapter::Adapter;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    /// The shipped adapter's hooks binding, so these render through the same
    /// maps a launch does rather than through a fixture that could be wrong in
    /// the same direction as the code.
    fn claude_hooks() -> crate::adapter::Adapter {
        crate::adapter::Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    fn hooks_binding(a: &crate::adapter::Adapter) -> &Binding {
        a.supports(Capability::Hooks).expect("claude has hooks")
    }

    #[test]
    fn hooks_group_by_event() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "h/a.json", r#"{"on":"turn-end","run":"one"}"#);
        file(dir.path(), "h/b.json", r#"{"on":"turn-end","run":"two"}"#);
        file(
            dir.path(),
            "h/c.json",
            r#"{"on":"after-tool","tools":["edit"],"run":"three"}"#,
        );

        let adapter = claude_hooks();
        let out = document(
            Capability::Hooks,
            hooks_binding(&adapter),
            &[dir.path().join("h")],
            &Default::default(),
            &Default::default(),
            &adapter.tools,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();

        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["hooks"]["PostToolUse"][0]["matcher"],
            "Edit|Write|MultiEdit"
        );
        assert_eq!(v["hooks"]["PostToolUse"][0]["hooks"][0]["type"], "command");
    }

    /// A hook says *when* it wants to fire. Which word this harness uses for
    /// that moment is the adapter's business, exactly as every path in an
    /// adapter already is.
    ///
    /// A hook file used to have to say `"event": "Stop"` — Claude Code's
    /// vocabulary, in a file omh presented as its own — with `matcher` and the
    /// `hookSpecificOutput` payload the same leak one level down. Nothing had
    /// ever had to translate one, only because opencode declared no hooks
    /// capability at all.
    #[test]
    fn a_hook_written_in_omhs_words_reaches_the_harness() {
        let dir = tempfile::tempdir().unwrap();
        file(
            dir.path(),
            "h/rust-test.json",
            r#"{"on":"turn-end","run":"cargo test"}"#,
        );

        let adapter = claude_hooks();
        let out = document(
            Capability::Hooks,
            hooks_binding(&adapter),
            &[dir.path().join("h")],
            &Default::default(),
            &Default::default(),
            &adapter.tools,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "cargo test");
    }

    /// The **shipped** opencode adapter, read once.
    ///
    /// Not a hand-built copy. `hook::tests::binding` already states the rule —
    /// "an adapter always arrives as a file, and a hand-built one can be right
    /// in exactly the way the code is wrong" — and these tests broke it: with a
    /// copy in the fixture, five separate mutations to `adapters/opencode.toml`
    /// left the whole suite green, including one that turns the refusal into
    /// `console.error` and one that renames the tool so `git-unavailable` can
    /// never match. The copy had already drifted, too: it omitted `edit`.
    fn opencode() -> &'static Adapter {
        static CELL: std::sync::OnceLock<Adapter> = std::sync::OnceLock::new();
        CELL.get_or_init(|| Adapter::find(Path::new(ADAPTERS), "opencode").unwrap())
    }

    fn opencode_hooks() -> &'static Binding {
        opencode()
            .supports(Capability::Hooks)
            .expect("opencode has hooks")
    }

    fn plugin(hooks: &[(&str, &str)]) -> Document {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in hooks {
            file(dir.path(), &format!("h/{name}.json"), body);
        }
        document(
            Capability::Hooks,
            opencode_hooks(),
            &[dir.path().join("h")],
            &Default::default(),
            &Default::default(),
            &opencode().tools,
        )
        .unwrap()
    }

    /// The staged file has to be something opencode will load: a module whose
    /// default export is a function returning the hook object.
    ///
    /// Structural only — that opencode *actually* loads it is a claim about
    /// external software, settled by `omh doctor` and a real session, not here.
    /// The staged file has to be something a JavaScript runtime will load.
    ///
    /// `node --check`, not a brace count. Counting braces across the whole file
    /// counts the ones inside string literals too, so it fails for a hook body
    /// containing `awk '{print}'` and passes for plenty of things that are not
    /// programs. It claimed to be a parse check and was not one.
    #[test]
    #[ignore]
    fn a_plugin_is_a_module_opencode_can_load() {
        let body = plugin(&[
            ("fmt", r#"{"on":"turn-end","run":"cargo fmt"}"#),
            // A body full of the characters that break generated programs.
            (
                "awkward",
                r#"{"on":"before-tool","tools":["shell"],"when":"awk '{print $1}' </dev/null; case \"$OMH_TOOL_COMMAND\" in *\"}\"*) ;; *) false ;; esac","refuse":"no \" quote, back\\slash, `tick`, 100%"}"#,
            ),
        ])
        .body;
        assert!(
            body.contains("export default"),
            "opencode imports the default export: {body}"
        );
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("omh.mjs");
        std::fs::write(&module, &body).unwrap();
        let out = std::process::Command::new("node")
            .args(["--check", module.to_str().unwrap()])
            .output()
            .expect("node is required to check the program omh generates");
        assert!(
            out.status.success(),
            "the generated module does not parse:\n{}\n{body}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A refusal blocks, and on opencode the only way to block is to throw.
    #[test]
    fn a_before_tool_refusal_becomes_a_throw() {
        let body = plugin(&[(
            "git-unavailable",
            r#"{"on":"before-tool","tools":["shell"],"refuse":"git does not work here"}"#,
        )])
        .body;
        assert!(body.contains("tool.execute.before"), "got: {body}");
        assert!(body.contains("throw new Error"), "got: {body}");
        assert!(body.contains("git does not work here"), "got: {body}");
    }

    /// Two hooks sharing a moment are independent, and the first must not be
    /// able to skip the second.
    ///
    /// Each hook's guards are early returns, and a bare block does not scope a
    /// `return` — it leaves the whole handler. So `git-unavailable` narrowing to
    /// `bash` silently cancelled every other before-tool hook on any call that
    /// was not bash, which is most of them. The generated program has to give
    /// each hook a scope of its own.
    ///
    /// Asserted by running the module, because this is a property of JavaScript
    /// rather than of the string omh built: a substring check would have been
    /// satisfied by the broken version.
    #[test]
    #[ignore]
    fn two_hooks_on_one_moment_do_not_cancel_each_other() {
        let body = plugin(&[
            (
                "a-read",
                r#"{"on":"before-tool","tools":["read"],"refuse":"read refused"}"#,
            ),
            (
                "b-shell",
                r#"{"on":"before-tool","tools":["shell"],"refuse":"shell refused"}"#,
            ),
        ])
        .body;

        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("omh.mjs");
        std::fs::write(&module, &body).unwrap();
        // Ask the module what it does for a `bash` call. The read-scoped hook
        // comes first alphabetically and does not match, so a handler-wide
        // return would answer "nothing".
        let driver = dir.path().join("run.mjs");
        std::fs::write(
            &driver,
            format!(
                r#"import plugin from "file://{}"
const hooks = await plugin({{}})
try {{
  await hooks["tool.execute.before"]({{ tool: "bash" }}, {{ args: {{ command: "git status" }} }})
  console.log("NOTHING")
}} catch (e) {{ console.log(e.message) }}
"#,
                module.display()
            ),
        )
        .unwrap();

        // No skip-on-missing-node. This is the only guard on the emitted
        // program's behaviour, and "a probe with no output is never a pass" is
        // in the invariant table — a test that goes green because the runtime
        // was absent is exactly that, inside the suite that enforces it.
        let out = std::process::Command::new("node")
            .arg(&driver)
            .output()
            .expect("node is required to check the program omh generates");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            said.contains("shell refused"),
            "the second hook has to run: {said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Not an assertion — a window on what the generator writes, so a reviewer
    /// reads the program rather than inferring it from six substring checks.
    #[test]
    #[ignore]
    fn show_the_plugin() {
        let doc = plugin(&[
            (
                "git-unavailable",
                r#"{"on":"before-tool","tools":["shell"],"when":"case \"$OMH_TOOL_COMMAND\" in git*) ;; *) false ;; esac","refuse":"git does not work here"}"#,
            ),
            (
                "graph-refresh",
                r#"{"on":"turn-end","run":"index --repo /work || true"}"#,
            ),
            (
                "note",
                r#"{"on":"after-tool","tools":["read"],"inject":"about $OMH_TOOL_FILE"}"#,
            ),
        ]);
        println!("{}", doc.body);
        println!(
            "dropped: {:?}",
            doc.dropped
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Run the generated module against the shapes opencode really passes.
    ///
    /// Returns the mutated tool result, or `THREW: …`. Substring assertions
    /// over generated source cannot see any of what this checks: which object
    /// holds the arguments, which identifiers are in scope in which handler,
    /// and whether a hook fires at all.
    ///
    /// The shapes are from the binary, not the docs — `tool.execute.before` is
    /// triggered as `(…, {args})` and `tool.execute.after` as `({…, args}, V)`,
    /// so the arguments move between the two parameters.
    fn drive(body: &str, slot: &str, input: &str, output: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("omh.mjs");
        std::fs::write(&module, body).unwrap();
        let driver = dir.path().join("run.mjs");
        std::fs::write(
            &driver,
            format!(
                r#"import plugin from "file://{}"
const hooks = await plugin({{}})
const input = {input}, output = {output}
try {{
  await hooks[{slot:?}]?.(input, output)
  console.log(JSON.stringify(output))
}} catch (e) {{ console.log("THREW: " + e.message) }}
"#,
                module.display()
            ),
        )
        .unwrap();
        let out = std::process::Command::new("node")
            .arg(&driver)
            .output()
            .expect("node is required: a probe that skips is a probe that passes");
        assert!(
            out.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // stderr as well as stdout: the module's diagnostics go there, and a
        // hook that could not run has to be distinguishable from one that
        // chose to stay silent.
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }

    /// A hook that could not run must not look like a hook that said nothing.
    ///
    /// `spawnSync` reports a failure to start, and a signal kill, as
    /// `status: null` — which collapsed into the same `1` a predicate returns
    /// when it deliberately declines. So `git-unavailable` whose predicate could
    /// not be evaluated let the call through in silence, and every `run` hook
    /// that failed, exited non-zero, or was killed produced nothing anywhere.
    /// That is `read_layer`'s closed loop on a different filesystem.
    ///
    /// The decision is unchanged — a hook degrades to a no-op, never to an
    /// error, which is the base set's own rule. What changes is that it says so.
    #[test]
    #[ignore]
    fn a_hook_that_could_not_run_says_so() {
        let doc = plugin(&[
            (
                "broken-predicate",
                r#"{"on":"before-tool","tools":["shell"],"when":"omh-no-such-binary","refuse":"blocked"}"#,
            ),
            (
                "broken-run",
                r#"{"on":"before-tool","tools":["shell"],"run":"omh-no-such-binary"}"#,
            ),
        ]);
        let said = drive(
            &doc.body,
            "tool.execute.before",
            r#"{ tool: "bash", sessionID: "s", callID: "c" }"#,
            r#"{ args: { command: "ls" } }"#,
        );
        assert!(
            !said.contains("THREW"),
            "a hook that cannot run degrades to a no-op: {said}"
        );
        for name in ["broken-predicate", "broken-run"] {
            assert!(said.contains(name), "{name} failed in silence: {said}");
        }
    }

    /// A refusal whose text could not be expanded must not block with an empty
    /// reason. Blocking a call and saying nothing is the worst of both states.
    #[test]
    #[ignore]
    fn a_refusal_always_carries_a_reason() {
        let doc = plugin(&[(
            "git-unavailable",
            r#"{"on":"before-tool","tools":["shell"],"refuse":"git does not work here"}"#,
        )]);
        // `t` shells out to expand the text; break the shell and it returns "".
        let broken = doc
            .body
            .replace(r#"spawnSync("sh""#, r#"spawnSync("omh-no-such-shell""#);
        let said = drive(
            &broken,
            "tool.execute.before",
            r#"{ tool: "bash", sessionID: "s", callID: "c" }"#,
            r#"{ args: { command: "git status" } }"#,
        );
        assert!(
            said.contains("git does not work here"),
            "the reason has to survive a failed expansion: {said}"
        );
    }

    /// **`after-tool` keeps the arguments on `input`, not `output`.**
    ///
    /// From the binary: `tool.execute.before` is triggered as
    /// `(…, {args: b})` and `tool.execute.after` as `({…, args: b}, V)` — the
    /// arguments move between the two parameters and the result takes the
    /// second. Reading `output.args` at `after` therefore binds the empty
    /// string, silently: a `when` testing the field is false forever and the
    /// hook never fires, which looks exactly like a hook with nothing to say.
    ///
    /// The adapter comment recorded `output.args` as verified — and it was, for
    /// `before` only. The generalisation to `after` was never checked.
    #[test]
    #[ignore]
    fn an_after_tool_hook_reads_the_arguments_where_this_moment_keeps_them() {
        let body = plugin(&[(
            "note",
            r#"{"on":"after-tool","tools":["read"],"when":"[ -n \"$OMH_TOOL_FILE\" ]","inject":"about $OMH_TOOL_FILE"}"#,
        )])
        .body;
        let said = drive(
            &body,
            "tool.execute.after",
            r#"{ tool: "read", sessionID: "s", callID: "c", args: { filePath: "/work/note.txt" } }"#,
            r#"{ title: "note.txt", output: "blue", metadata: {} }"#,
        );
        assert!(
            said.contains("/work/note.txt"),
            "the field has to reach the hook: {said}"
        );
    }

    /// omh's own refusal, on the shipped adapter, actually blocks — and the
    /// reason reaches the model.
    ///
    /// Every part of this was mutable in silence before the fixture read the
    /// real file: renaming the tool (`bash` → `sh`) so nothing matches,
    /// renaming the field so the predicate reads empty, or replacing the
    /// `[refuse]` template with `console.error` so the wall becomes a log line.
    /// The suite stayed green through all three.
    #[test]
    #[ignore]
    fn the_shipped_refusal_blocks_a_git_call() {
        let doc = plugin(&[(
            "git-unavailable",
            r#"{"on":"before-tool","tools":["shell"],"when":"case \"$OMH_TOOL_COMMAND\" in git*) ;; *) false ;; esac","refuse":"git does not work here"}"#,
        )]);
        assert!(doc.dropped.is_empty(), "{:?}", doc.dropped);

        let blocked = drive(
            &doc.body,
            "tool.execute.before",
            r#"{ tool: "bash", sessionID: "s", callID: "c" }"#,
            r#"{ args: { command: "git status" } }"#,
        );
        assert_eq!(
            blocked, "THREW: git does not work here",
            "the call has to be blocked, with the reason"
        );

        // And a call it does not care about goes through untouched.
        let allowed = drive(
            &doc.body,
            "tool.execute.before",
            r#"{ tool: "bash", sessionID: "s", callID: "c" }"#,
            r#"{ args: { command: "ls" } }"#,
        );
        assert!(
            !allowed.contains("THREW"),
            "a nudge is not a wall: {allowed}"
        );
    }

    /// And the advisory half, on the shipped adapter: the note reaches the model
    /// by being appended to the result, without replacing it.
    #[test]
    #[ignore]
    fn a_shipped_inject_appends_to_the_result_rather_than_replacing_it() {
        let doc = plugin(&[(
            "note",
            r#"{"on":"after-tool","tools":["read"],"inject":"consider the graph"}"#,
        )]);
        let said = drive(
            &doc.body,
            "tool.execute.after",
            r#"{ tool: "read", sessionID: "s", callID: "c", args: { filePath: "/work/f" } }"#,
            r#"{ title: "f", output: "the original bytes", metadata: {} }"#,
        );
        assert!(said.contains("the original bytes"), "kept: {said}");
        assert!(said.contains("consider the graph"), "and added: {said}");
    }

    /// **The bus handler has no call in scope, so a hook needing one is dropped.**
    ///
    /// `event` is handed only the bus event — `async (input)`. Emitting
    /// `output?.args?.…` or an inject template there references an *undeclared*
    /// identifier, and optional chaining does not save that: it is a
    /// `ReferenceError`, and because every block in a slot is awaited in one
    /// handler it takes `graph-refresh` down with it, on every session.
    ///
    /// So a hook wanting a payload field, a tool, or text at a moment with no
    /// call is named at launch, which is the degradation this module already
    /// commits to everywhere else.
    #[test]
    fn a_hook_needing_a_call_is_dropped_at_a_moment_that_has_none() {
        for (name, body) in [
            (
                "reads-a-field",
                r#"{"on":"turn-end","when":"[ -n \"$OMH_TOOL_FILE\" ]","run":"reindex"}"#,
            ),
            (
                "narrows-to-a-tool",
                r#"{"on":"turn-end","tools":["shell"],"run":"x"}"#,
            ),
            (
                "injects",
                r#"{"on":"turn-end","inject":"remember to test"}"#,
            ),
        ] {
            let doc = plugin(&[(name, body)]);
            let named: Vec<&str> = doc.dropped.iter().map(|d| d.name.as_str()).collect();
            assert_eq!(named, vec![name], "{name} must be named, not emitted");
            assert!(
                !doc.body.contains("output"),
                "{name} leaked an out-of-scope reference: {}",
                doc.body
            );
        }
    }

    /// And a plain `run` at that moment still works — the drop is about needing
    /// a call, not about the moment.
    #[test]
    #[ignore]
    fn a_bus_moment_still_runs_a_command() {
        let body = plugin(&[("refresh", r#"{"on":"turn-end","run":"true"}"#)]).body;
        let said = drive(
            &body,
            "event",
            r#"{ event: { type: "session.idle" } }"#,
            "undefined",
        );
        assert!(!said.contains("THREW"), "got: {said}");
    }

    /// **The finding P5 exists to produce.** An advisory nudge has no channel
    /// before a tool runs on this harness, so it is dropped by name rather than
    /// promoted to a refusal — a nudge that silently became a wall would look
    /// exactly like working, and `graph-first` says why that is unacceptable.
    #[test]
    fn a_before_tool_inject_is_dropped_by_name() {
        let doc = plugin(&[(
            "graph-first",
            r#"{"on":"before-tool","tools":["read"],"inject":"use the graph"}"#,
        )]);
        let names: Vec<&str> = doc.dropped.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["graph-first"]);
        assert!(
            doc.dropped[0].wanted.contains("inject"),
            "say what it wanted: {}",
            doc.dropped[0].wanted
        );
        assert!(
            !doc.body.contains("use the graph"),
            "and it must not have leaked in as a throw: {}",
            doc.body
        );
    }

    /// After the tool has run there *is* a channel: the result is what the model
    /// reads next, and mutating it is how a hook reaches the model. Verified in
    /// a container — a hook that rewrote a read's output changed the answer.
    #[test]
    fn an_after_tool_inject_mutates_the_tool_result() {
        let body = plugin(&[(
            "note",
            r#"{"on":"after-tool","tools":["read"],"inject":"and consider the graph"}"#,
        )])
        .body;
        assert!(body.contains("tool.execute.after"), "got: {body}");
        assert!(body.contains("output.output"), "got: {body}");
        assert!(body.contains("and consider the graph"), "got: {body}");
    }

    /// A hook narrows to omh's tool vocabulary; the plugin tests the harness's
    /// own word for it. Emitting omh's word would match nothing, silently.
    #[test]
    fn a_tool_scoped_hook_tests_the_harnesss_own_tool_name() {
        let body = plugin(&[(
            "git-unavailable",
            r#"{"on":"before-tool","tools":["shell"],"refuse":"no git"}"#,
        )])
        .body;
        assert!(
            body.contains(r#"["bash"].includes(input.tool)"#),
            "opencode calls it bash, not shell: {body}"
        );
    }

    /// The payload field is read from the parameter **this moment** keeps it on.
    ///
    /// Not one expression for both: from the binary, `tool.execute.before` is
    /// triggered as `(…, {args})` and `tool.execute.after` as `({…, args}, V)`.
    /// This test asserted `output?.args?.filePath` for an *after*-tool hook and
    /// so pinned the wrong one — the field bound the empty string, the `when`
    /// testing it was false forever, and the hook never fired. An output-shape
    /// assertion that passed while the thing it guarded was broken.
    ///
    /// Still a shape assertion, because the behavioural half is
    /// `an_after_tool_hook_reads_the_arguments_where_this_moment_keeps_them`,
    /// which runs the module. This one holds both moments side by side, where
    /// the asymmetry is easy to see and easy to get wrong.
    #[test]
    fn the_payload_field_is_read_where_this_moment_keeps_it() {
        for (on, from) in [("before-tool", "output"), ("after-tool", "input")] {
            let body = plugin(&[(
                "big",
                &format!(
                    r#"{{"on":"{on}","tools":["read"],"when":"[ -f \"$OMH_TOOL_FILE\" ]","run":"x"}}"#
                ),
            )])
            .body;
            assert!(
                body.contains(&format!("{from}?.args?.filePath")),
                "{on} keeps its arguments on `{from}`: {body}"
            );
            assert!(
                !body.contains("jq"),
                "jq is Claude's payload, not this one: {body}"
            );
        }
    }

    /// A repo may override an MCP server's environment without redeclaring the
    /// server.
    ///
    /// This is what replaced a `<repo>/.omh/local/mcp.json` holding a token for
    /// one project. Redeclaring meant copying the whole entry — command, args
    /// and all — so a catalogue fix never reached the repos that had one, and
    /// the copy was invisible until it drifted. An override is configuration:
    /// it names the server and the variable, and nothing else.
    #[test]
    fn a_repo_overrides_a_servers_env_without_redeclaring_it() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = file(
            dir.path(),
            "mcp.json",
            r#"{"mcpServers":{"linear":{"command":"npx","args":["-y","mcp-remote"],
                                        "env":{"LINEAR_API_KEY":"","REGION":"eu"}}}}"#,
        );

        let repo = crate::settings::RepoPolicy {
            mcp_env: BTreeMap::from([(
                "linear".to_string(),
                BTreeMap::from([("LINEAR_API_KEY".to_string(), "secret".to_string())]),
            )]),
            ..Default::default()
        };
        let adapter = claude_hooks();
        let out = document(
            Capability::Mcp,
            adapter.supports(Capability::Mcp).unwrap(),
            &[mcp],
            &Default::default(),
            &repo,
            &adapter.tools,
        )
        .unwrap();

        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        let server = &v["mcpServers"]["linear"];
        assert_eq!(server["env"]["LINEAR_API_KEY"], "secret");
        assert_eq!(
            server["env"]["REGION"], "eu",
            "an override, not a replacement"
        );
        assert_eq!(server["command"], "npx", "the server itself is untouched");
    }

    /// An override for a server nobody has is a token going nowhere, which is
    /// exactly the shape of a setting somebody swears they configured.
    #[test]
    fn an_override_for_a_server_that_is_not_installed_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = file(dir.path(), "mcp.json", r#"{"mcpServers":{}}"#);
        let repo = crate::settings::RepoPolicy {
            mcp_env: BTreeMap::from([("linear".to_string(), BTreeMap::new())]),
            ..Default::default()
        };
        let adapter = claude_hooks();
        let err = document(
            Capability::Mcp,
            adapter.supports(Capability::Mcp).unwrap(),
            &[mcp],
            &Default::default(),
            &repo,
            &adapter.tools,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("linear"), "got: {err:#}");
    }

    /// A manifest name is omh's, and a file answering to one is an error naming
    /// both — never an override.
    ///
    /// A repo that could replace `graph-refresh` could make the graph lie while
    /// looking installed: the server answers, the index never updates, and
    /// every structural answer is about code the agent has since rewritten.
    /// Everything else in these directories is a file and files are yours to
    /// overwrite; this is the one name that is not.
    ///
    /// Silently skipping it was right while the only such files were leftovers
    /// omh had seeded itself. Now that `<repo>/.omh/hooks/` is a place people
    /// write on purpose, a name that does nothing and says nothing is a hook
    /// somebody wrote, committed, and will believe is running.
    #[test]
    fn a_hook_answering_to_a_manifest_name_is_an_error_naming_both() {
        let dir = tempfile::tempdir().unwrap();
        file(
            dir.path(),
            "h/graph-refresh.json",
            r#"{"on":"turn-end","run":"my own indexer"}"#,
        );

        let own = crate::base::Own {
            reserved: ["graph-refresh".to_string()].into(),
            ..Default::default()
        };
        let err = merge_hooks(&[dir.path().join("h")], &own, &Default::default())
            .expect_err("a manifest name is not something a file may claim");
        let msg = format!("{err:#}");
        assert!(msg.contains("graph-refresh.json"), "name the file: {msg}");
        assert!(
            msg.contains("codegraph") || msg.contains("omh"),
            "and whose name it is: {msg}"
        );

        // And whatever `[use]` says. The selection filter sits *after* this
        // guard on purpose — a repo that shipped a file answering to nothing
        // has to hear about it whether or not its list happens to name it, and
        // `init` writes an expanded list, so a hook added later is unselected
        // by default. With the two swapped, the check goes quiet for exactly
        // the repos most likely to have the problem.
        let mut repo = crate::settings::RepoPolicy::default();
        repo.selection
            .apply(
                &BTreeMap::from([("hooks".to_string(), Vec::new())]),
                Path::new("settings.toml"),
            )
            .unwrap();
        assert!(
            merge_hooks(&[dir.path().join("h")], &own, &repo).is_err(),
            "an unselected hook file still may not claim a name omh ships"
        );
    }

    /// A directory omh cannot read is not a directory with no hooks in it.
    ///
    /// The `config::read_layer` lesson, in the function generation rewrote —
    /// and generation made it quieter rather than louder: omh's own five are
    /// merged in afterwards, so the rendered document is never empty and
    /// `omh doctor`'s hooks check passes while the user's entire layer is
    /// gone. `config::hooks` reads these same directories and errors; the two
    /// commands disagreed about whether the same filesystem state was a
    /// problem.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_hooks_directory_is_an_error_not_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "h/a.json", r#"{"on":"turn-end","run":"one"}"#);
        let hooks = dir.path().join("h");
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o000)).unwrap();

        let err = merge_hooks(
            std::slice::from_ref(&hooks),
            &Default::default(),
            &Default::default(),
        )
        .expect_err("an unreadable layer must be reported, not skipped");
        // Restore before the assertion so a failure cannot leave the temp dir
        // undeletable.
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(err.to_string().contains("h"), "must name the path: {err}");
    }

    #[test]
    fn staged_renders_are_not_documents() {
        let adapter = claude_hooks();
        let skills = adapter.supports(Capability::Skills).unwrap();
        let err = document(
            Capability::Skills,
            skills,
            &[],
            &Default::default(),
            &Default::default(),
            &adapter.tools,
        )
        .unwrap_err();
        assert!(err.to_string().contains("staged by the launcher"));
    }

    #[test]
    fn malformed_json_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let bad = file(dir.path(), "broken.json", "{ not json");
        let err = merge_servers(&[bad]).unwrap_err();
        assert!(err.to_string().contains("broken.json"), "got: {err}");
    }

    // ── parse / round-trip ──────────────────────────────────────────────────

    fn canonical() -> BTreeMap<String, Server> {
        let mut m = BTreeMap::new();
        m.insert(
            "plain".to_string(),
            Server {
                command: "plain-cmd".into(),
                args: vec![],
                env: BTreeMap::new(),
            },
        );
        m.insert(
            "rich".to_string(),
            Server {
                command: "rich-cmd".into(),
                args: vec!["--root".into(), "/work".into()],
                env: BTreeMap::from([("TOKEN".to_string(), "abc".to_string())]),
            },
        );
        m
    }

    /// The property that makes import safe: anything omh can write, omh can read
    /// back without loss. A renderer that gains a field must gain a parser too.
    #[test]
    fn every_mcp_format_round_trips() {
        let original = canonical();
        for format in [Render::McpJson, Render::CodexToml, Render::OpencodeJson] {
            let rendered = mcp(format, &original).unwrap();
            let back = parse(format, &rendered)
                .unwrap_or_else(|e| panic!("{format:?} failed to parse its own output: {e:#}"));
            assert_eq!(back, original, "{format:?} lost data on round-trip");
        }
    }

    /// opencode folds command and args into one array; parsing must split the
    /// head back off or every server imports with its args glued to its command.
    #[test]
    fn opencode_command_array_splits_back_into_command_and_args() {
        let raw = r#"{"mcp":{"g":{"type":"local","command":["cmd","--a","--b"]}}}"#;
        let back = parse(Render::OpencodeJson, raw).unwrap();
        assert_eq!(back["g"].command, "cmd");
        assert_eq!(back["g"].args, ["--a", "--b"]);
    }

    #[test]
    fn parses_a_hand_written_mcp_json() {
        let raw = r#"{"mcpServers":{"g":{"command":"c","args":["x"],"env":{"K":"v"}}}}"#;
        let back = parse(Render::McpJson, raw).unwrap();
        assert_eq!(back["g"].args, ["x"]);
        assert_eq!(back["g"].env["K"], "v");
    }

    #[test]
    fn parses_a_hand_written_codex_toml() {
        let raw =
            "[mcp_servers.g]\ncommand = \"c\"\nargs = [\"x\"]\n\n[mcp_servers.g.env]\nK = \"v\"\n";
        let back = parse(Render::CodexToml, raw).unwrap();
        assert_eq!(back["g"].command, "c");
        assert_eq!(back["g"].env["K"], "v");
    }

    #[test]
    fn empty_config_parses_to_no_servers() {
        assert!(parse(Render::McpJson, "{}").unwrap().is_empty());
        assert!(parse(Render::CodexToml, "").unwrap().is_empty());
        assert!(parse(Render::OpencodeJson, "{}").unwrap().is_empty());
    }

    /// Claude's global config nests servers under each project. Guessing which
    /// project to take would import servers from unrelated repos, so refuse and
    /// say what to do instead.
    #[test]
    fn project_nested_claude_config_is_refused_with_guidance() {
        let raw = r#"{"projects":{"/some/repo":{"mcpServers":{"g":{"command":"c"}}}}}"#;
        let err = parse(Render::McpJson, raw).unwrap_err();
        assert!(format!("{err:#}").contains("projects"), "got: {err:#}");
    }

    #[test]
    fn malformed_input_is_an_error_not_an_empty_import() {
        assert!(parse(Render::McpJson, "{ not json").is_err());
        assert!(parse(Render::CodexToml, "[[[").is_err());
    }

    #[test]
    fn non_mcp_formats_cannot_be_parsed_as_servers() {
        assert!(parse(Render::Dir, "").is_err());
        assert!(parse(Render::ClaudeSettings, "{}").is_err());
    }
}
