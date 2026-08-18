//! The only place a harness difference costs more than a bind mount.
//!
//! A capability is declared once in canonical form and reshaped into whatever
//! the target harness parses. This is how `omh-mcp` (memory) and the wired
//! code-graph server reach every harness without being configured twice.

use crate::adapter::{Binding, Capability, Render};
use crate::hook::{self, Outcome};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    // `resolves`: what the image this session will run has been measured to
    // contain, from `facts::Facts::about`. A program absent from it is one
    // nobody probed, which suppresses nothing — see `suppressed_by_probe`.
    resolves: &BTreeMap<String, bool>,
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
        // Both hook renders suppress before translating, and neither may skip
        // it: a rule applied on one harness and not the other is not a rule
        // about this repo, it is a rule about which harness you happened to
        // launch.
        Render::ClaudeSettings => {
            let mut hooks = merge_hooks(sources, own, repo)?;
            let mut dropped = suppressed_by_probe(&mut hooks, resolves);
            let (rendered, unspellable) = translate(&hooks, binding, tools)?;
            dropped.extend(unspellable);
            Ok(Document {
                body: claude_settings(&rendered)?,
                dropped,
            })
        }
        Render::OpencodePlugin => {
            let mut hooks = merge_hooks(sources, own, repo)?;
            let mut dropped = suppressed_by_probe(&mut hooks, resolves);
            let mut doc = opencode_plugin(&hooks, binding, tools)?;
            dropped.extend(std::mem::take(&mut doc.dropped));
            doc.dropped = dropped;
            Ok(doc)
        }
        Render::OmpPlugin => {
            let mut hooks = merge_hooks(sources, own, repo)?;
            let mut dropped = suppressed_by_probe(&mut hooks, resolves);
            let mut doc = omp_plugin(&hooks, binding, tools)?;
            dropped.extend(std::mem::take(&mut doc.dropped));
            doc.dropped = dropped;
            Ok(doc)
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

/// Remove the hooks the image has been **measured** unable to run, and name
/// them.
///
/// `.omh/hooks/` is the repo's statement about itself — committed, shared, the
/// same for everybody who clones. Whether `cargo` resolves is a fact about one
/// image. So a missing toolchain never edits the repo; it stops a hook firing
/// against this image, and a repo whose sandbox gains the program gets the hook
/// back with nothing to un-configure.
///
/// **Measured, never declared.** This used to read `[toolchain]`, a table
/// `init` filled from a question — which asked somebody to configure around a
/// sandbox that was broken, and recorded the answer in a committed file where
/// it outlived the breakage. Provisioning removed the question; what is left is
/// the measurement, and a measurement needs no consent to be right.
///
/// Reported as [`hook::Dropped`], the same channel a hook a harness cannot
/// spell already uses, so an absence is never silent — the failure this whole
/// feature replaces was a hook that ran and said `cargo: not found`, and a hook
/// that vanishes without a word is not obviously an improvement on it.
///
/// Keyed on the command, never on the stack that produced it: a hook somebody
/// wrote by hand obeys the same answer, which is what makes this a rule about
/// programs rather than a rule about the four stacks omh happens to detect.
/// Which ecosystem each hook file in these directories names, if any.
///
/// Read **without** the selection filter, deliberately: this is what decides
/// what a selection may contain, and applying the selection first would be
/// circular — a hook could never be offered because it was never selected
/// because it was never offered.
///
/// A file that will not parse contributes `None`, which means *belongs
/// everywhere* and so stays offered. That is the safe direction: this feeds
/// what `init` writes into `[use]` and what the launcher reports as unselected,
/// and quietly dropping a name from both because its file has a typo would hide
/// the hook and the typo together. `merge_hooks` is where a bad hook file is an
/// error, at the point it would actually run.
pub fn declared_stacks(dirs: &[PathBuf]) -> Result<BTreeMap<String, Option<String>>> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let path = entry
                .with_context(|| format!("reading {}", dir.display()))?
                .path();
            if !path.extension().is_some_and(|e| e == "json") {
                continue;
            }
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let stack = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| hook::Hook::parse(&raw, &path.display().to_string()).ok())
                .and_then(|h| h.stack);
            // Later directories win, matching `merge_hooks`: a repo hook
            // shadowing a catalogue name decides what that name belongs to.
            out.insert(name, stack);
        }
    }
    Ok(out)
}

/// Which of this repo's hooks the image cannot run, without rendering anything.
///
/// `init` reports these and a launch drops them, and both go through here so
/// they cannot disagree — a setup report that named a different set from the
/// one the session ships would be worse than no report, because it is the one
/// people read to find out what they got.
pub fn held_back(
    dirs: &[PathBuf],
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
    resolves: &BTreeMap<String, bool>,
) -> Result<Vec<hook::Dropped>> {
    let mut hooks = merge_hooks(dirs, own, repo)?;
    Ok(suppressed_by_probe(&mut hooks, resolves))
}

fn suppressed_by_probe(
    hooks: &mut BTreeMap<String, hook::Hook>,
    resolves: &BTreeMap<String, bool>,
) -> Vec<hook::Dropped> {
    let mut dropped = Vec::new();
    hooks.retain(|name, hook| {
        // Two cannot-tells, and neither is a licence to switch a hook off:
        // `detect::program` answers `None` for a command it could not read, and
        // a program absent from the map is one nobody has probed.
        let blocked = hook.runs().into_iter().find_map(|cmd| {
            let p = crate::detect::program(cmd)?;
            (resolves.get(p) == Some(&false)).then(|| p.to_string())
        });
        match blocked {
            Some(program) => {
                dropped.push(hook::Dropped {
                    name: name.clone(),
                    wanted: format!("`{program}` — not installed in this repo's sandbox"),
                });
                false
            }
            None => true,
        }
    });
    dropped
}

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
pub fn merge_hooks(
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

/// Every program the hooks this repo would actually ship name, as one set.
///
/// The union suppression has to be built from. A stack's `needs` says what a
/// *stack* declared; this says what a *hook* will hand to a shell, and the two
/// are not the same list — a hand-written `shellcheck` hook is in no `needs`.
/// Asking about only one of them ships a hook into a sandbox that cannot run
/// it, which is the failure this design opens by describing.
///
/// Built on `merge_hooks` so it is the same set the renderer ships: the
/// selection, the reserved names and the repo's shadowing are all applied
/// already. A second notion of *which hooks count* is how a probe starts
/// answering about a hook nobody runs.
///
/// A command `detect::program` cannot read contributes nothing. That is
/// *cannot tell*, and this is the direction where cannot-tell is cheap: a
/// missing question costs one confusing hook error, an invented one reports a
/// gap in a repo whose toolchain is fine.
pub fn hook_programs(
    dirs: &[PathBuf],
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
) -> Result<BTreeSet<String>> {
    Ok(merge_hooks(dirs, own, repo)?
        .values()
        .flat_map(hook::Hook::runs)
        .filter_map(crate::detect::program)
        .map(str::to_string)
        .collect())
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

    let mut out = String::from(SHELL_BRIDGE) + PLUGIN_PREAMBLE;
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

/// The four moments of oh-my-pi's that omh has a word for.
///
/// The two tool moments carry a call; the other two do not. Spelled out rather
/// than "everything else", so an event name omp has not got is answerable as
/// such instead of being classified as a moment without a call — see
/// [`Moment::of`]. These duplicate the values in the omp adapter's
/// `[capabilities.hooks.events]`, and the pairing is held by
/// `every_moment_omp_has_registers_itself_under_the_right_name`.
const OMP_TOOL_CALL: &str = "tool_call";
const OMP_TOOL_RESULT: &str = "tool_result";
const OMP_SESSION_START: &str = "session_start";
const OMP_TURN_END: &str = "turn_end";

/// What an omp moment can express, which is decided by whether a call is in
/// scope at it.
///
/// The same distinction opencode's `Slot` draws, drawn on different evidence:
/// there it is which handler a moment lives on, here it is simply which of the
/// four `pi.on` events it is. A moment with no call has no `event.toolName` to
/// narrow on and no `event.input` to read a field off, and — for `tool_call` —
/// a call in scope is still not an advisory channel.
#[derive(Clone, Copy, PartialEq)]
enum Moment {
    /// `tool_call`: can block a call, cannot advise, has the arguments.
    Before,
    /// `tool_result`: can rewrite what the model reads next, cannot block.
    After,
    /// `session_start` and `turn_end`: a `run` and nothing else.
    Bare,
}

impl Moment {
    /// `None` for an event oh-my-pi has not got.
    ///
    /// This fell through to `Bare`, which reads as "a moment with no call in
    /// scope" — so an adapter typo came back as a statement about the harness:
    /// `after-tool = "toolresult"` dropped every injecting hook with *"no way
    /// to inject text at `after-tool`"*, which is false of omp and sends the
    /// reader to the wrong software. `Slot::of` can fall through honestly
    /// because `Slot::Bus` carries the unrecognised name forward into a real
    /// opencode mechanism; `Bare` discards it. The two look symmetric and are
    /// not.
    fn of(event: &str) -> Option<Self> {
        match event {
            OMP_TOOL_CALL => Some(Moment::Before),
            OMP_TOOL_RESULT => Some(Moment::After),
            OMP_SESSION_START | OMP_TURN_END => Some(Moment::Bare),
            _ => None,
        }
    }
}

/// oh-my-pi hook module: `pi.on(...)` registrations inside one factory.
///
/// Structurally the twin of [`opencode_plugin`] and deliberately not shared
/// with it. The two agree on the shell bridge and on nothing else: opencode
/// keeps a tool's arguments on `output`/`input` depending on the moment and
/// dispatches its bus events through one catch-all, while every omp handler
/// receives an `event` and every omp hook registers itself. A single
/// generator over both would be a `match` on the harness in every line.
fn omp_plugin(
    hooks: &BTreeMap<String, hook::Hook>,
    binding: &Binding,
    tools: &BTreeMap<hook::Tool, String>,
) -> Result<Document> {
    let mut dropped = Vec::new();
    // One `pi.on` per hook, in hook-name order.
    //
    // They were grouped by moment first, one registration holding every hook
    // that shared it, and that was wrong in a way nothing reported: the first
    // hook to return ended the handler, so a second injecting hook at the same
    // moment simply never ran. It was not dropped and not warned about.
    //
    // Separate registrations hand the ordering back to omp, whose rules are
    // per-handler and documented: `tool_call` takes the first block, and
    // `tool_result` handlers chain, each seeing the last one's edits. omh
    // cannot reproduce that from inside one handler and has no business
    // trying.
    let mut out = String::from(SHELL_BRIDGE) + OMP_PREAMBLE;

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
        let Some(moment) = Moment::of(wired.event) else {
            anyhow::bail!(
                "adapter maps `{}` to `{}`, which is not a moment oh-my-pi has —                  omh would report every hook there as a capability the harness                  lacks. Expected one of: {OMP_SESSION_START}, {OMP_TURN_END},                  {OMP_TOOL_CALL}, {OMP_TOOL_RESULT}.",
                hook.on,
                wired.event,
            )
        };
        // A moment with no call in scope can express none of the things that
        // need one. Named rather than emitted and left to fail at runtime: a
        // handler referencing an `event.input` that is not there binds the
        // empty string, so the hook would not fire and nothing would say why.
        if moment == Moment::Bare {
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
        // Advisory text has no channel before the call on this harness either:
        // `tool_call` returns a block or it returns nothing. Checked before the
        // binding is asked, because the binding *can* advise — just not here.
        if matches!(hook.action, hook::Action::Inject { .. }) && moment == Moment::Before {
            dropped.push(give_up("way to inject text before a tool runs"));
            continue;
        }
        // A field the adapter maps for the harness, on a tool that has not got
        // it. omp's `edit` takes one `input` string with the path inside a
        // `[PATH#TAG]` payload, so `event.input.path` is never there — while
        // `read`, which the same map serves correctly, does have it.
        //
        // Emitting it anyway bound `""`, and a hook that then guards on the
        // value simply never fired: in the module, in `doctor`'s name list, not
        // in `dropped`, indistinguishable from a hook with nothing to say.
        //
        // The knowledge is omp's and lives in omp's renderer because the schema
        // has no way to say "this field exists on these tools and not those" —
        // `fields` is one map per harness. That is a real limit of the adapter
        // format and is recorded in `adapters/omp.toml` beside the map itself.
        if let Some(edit) = tools.get(&hook::Tool::Edit) {
            let wants_file = wired
                .fields
                .iter()
                .any(|(f, _)| *f == hook::Field::ToolFile);
            if wants_file && wired.tools.iter().any(|t| t == edit) {
                dropped.push(give_up(&format!("`tool-file` on `{edit}`")));
                continue;
            }
        }
        // There is deliberately no mirror of that check for a `refuse` at
        // `after-tool`. It reads like the obvious counterpart and would be dead
        // code: omh refuses that pairing when the hook is *parsed*, so no such
        // hook reaches a renderer — which is where the rule belongs, because it
        // is true of every harness rather than of this one.
        let protocol = match binding.protocol(&hook.action) {
            Ok(p) => p,
            Err(wanted) => {
                dropped.push(give_up(wanted));
                continue;
            }
        };
        out.push_str(&omp_one_hook(name, hook, &wired, moment, protocol));
    }

    out.push_str("}\n");
    Ok(Document { body: out, dropped })
}

/// One hook, as its own `pi.on` registration.
fn omp_one_hook(
    name: &str,
    hook: &hook::Hook,
    wired: &hook::Wired<'_>,
    moment: Moment,
    protocol: Option<&crate::adapter::Template>,
) -> String {
    // The handler *is* the hook's function scope, so a `return` ends this hook
    // and nothing else — no IIFE, and no name mangled into an identifier to
    // hold its result. opencode needs both because its harness gives one
    // handler per moment and omh has to share it; omp does not.
    let mut b = format!(
        "  // {name}\n  pi.on({:?}, async (event, ctx) => {{\n",
        wired.event
    );
    if !wired.tools.is_empty() {
        let names = wired
            .tools
            .iter()
            .map(|t| format!("{t:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        b.push_str(&format!(
            "    if (![{names}].includes(event.toolName)) return\n"
        ));
    }
    b.push_str("    const env = {}\n");
    // Both tool moments keep the call's arguments on `event.input` — unlike
    // opencode, where the parameter they hang off changes with the moment.
    if moment != Moment::Bare {
        for (field, at) in &wired.fields {
            b.push_str(&format!(
                "    env[{:?}] = String(event.input?.{at} ?? \"\")\n",
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
            "    const cap = sh({}, env)\n    if (!cap.ran || cap.code !== 0) warn({}, \"capture\", cap)\n    env[{:?}] = cap.out\n",
            js(capture),
            js(name),
            hook::CAPTURE_VAR,
        ));
    }
    if let Some(when) = &hook.when {
        b.push_str(&format!(
            "    const p = sh({}, env)\n    if (!p.ran || p.err) warn({}, \"its `when`\", p)\n",
            js(when),
            js(name),
        ));
        // A guard that could not be evaluated is not a guard that declined.
        // Both leave `p.code` at 1, and returning from `tool_call` means allow
        // — so a refusal whose predicate could not run let the call through,
        // which is the one degradation this renderer must never make. An
        // `inject` or a `run` in the same state still falls through to silence.
        if let (hook::Action::Refuse { .. }, Some(t)) = (&hook.action, protocol) {
            b.push_str(&format!(
                "    if (!p.ran) {}\n",
                t.template.replace(
                    "{{text}}",
                    &js(&format!(
                        "omh: refusing — `{name}` could not evaluate its guard, so this call is blocked rather than allowed unchecked"
                    )),
                ),
            ));
        }
        b.push_str("    if (p.code !== 0) return\n");
    }
    match &hook.action {
        hook::Action::Run(run) => b.push_str(&format!(
            "    const r = sh({}, env)\n    if (!r.ran || r.code !== 0) warn({}, \"its `run`\", r)\n",
            js(run),
            js(name),
        )),
        hook::Action::Inject { text, .. } | hook::Action::Refuse { text } => {
            let template = protocol.map(|p| p.template.as_str()).unwrap_or_default();
            b.push_str(&format!(
                "    {}\n",
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
    b.push_str("  })\n");
    b
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

/// opencode's module shape, emitted after [`SHELL_BRIDGE`].
///
/// Split from the bridge when a second harness needed the same bridge under a
/// different shape — this half is the part that differs.
const PLUGIN_PREAMBLE: &str = "export default (async () => ({\n";

/// oh-my-pi's hook-factory shape, emitted after [`SHELL_BRIDGE`].
///
/// A default-exported function receiving `pi`, rather than opencode's object of
/// named handlers. Each hook registers itself inside it with `pi.on(...)`.
///
/// Nothing here emits opencode's `Slot::Bus` type test, because `pi.on` has
/// already done it: opencode dispatches its bus moments through one catch-all
/// handler, so the generated program must check which event it got. [`Moment`]
/// is the Rust-side equivalent and is load-bearing — it decides what a moment
/// can express — but it never reaches the emitted module.
const OMP_PREAMBLE: &str = "export default function (pi) {\n";

/// The shell bridge both generated modules share.
///
/// One copy, because a fix to how a hook's text expands is a fix to what omh
/// *means*, and a harness that missed it would mean something else.
///
/// `node:child_process` rather than the `$` helper opencode passes each plugin.
/// `$` is Bun's shell, and its behaviour around exit codes and quoting is a
/// claim about a runtime omh does not control; `spawnSync` behaves the same on
/// anything that can load this file at all.
const SHELL_BRIDGE: &str = r#"// Generated by omh. Edits are overwritten at launch.
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
const warn = (hook, phase, r) => {
  // "did not run" was printed for both, and it is false of the second: a
  // `turn-end` hook running a failing test suite reported that its `run` did
  // not run. `out` is carried too — a tool that writes its diagnostics to
  // stdout left the old message with no evidence at all.
  const what = r.ran ? `exited ${r.code}` : "could not run"
  const said = [r.err, r.out].filter(Boolean).join(" ").slice(0, 300)
  console.error(`omh: hook ${hook}: ${phase} ${what}${said ? " — " + said : ""}`)
}

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
    // Half of the `mcp` binding, and it has to live here because this is the
    // only file this harness reads it from. A project-scoped MCP document is
    // *listed* without it and *loaded* with it — the sandbox otherwise starts
    // with servers sitting at "pending approval", waiting for a dialog nobody
    // is there to answer.
    //
    // Blanket rather than per-server, because the only project document in
    // there is the one omh mounted: the mount covers the worktree's own
    // `.mcp.json`, so this approves omh's rendering of what the profile
    // already decided, not whatever a checkout happened to ship.
    pretty(serde_json::json!({
        "hooks": by_event,
        "enableAllProjectMcpServers": true,
    }))
}

/// Read a harness's own hook configuration back into omh's words.
///
/// The inverse of [`claude_settings`], and the rule `parse` already follows for
/// MCP: *every format that renders must also parse, and the pair must round
/// trip*. Without it omh can export somebody's setup and never meet them where
/// they already are, which is the whole of what `omh import` is for.
///
/// **Nothing is imported half-way.** Anything omh cannot say whole comes back
/// as [`hook::Dropped`] — named, and left in the file it came from — rather
/// than as a hook with the part omh understood. The case that decides the
/// shape is `if`: a handler's permission gate. A `command` imported without
/// its `if` is a hook that fired on one narrow case now firing on every call,
/// which is not a smaller version of what somebody wrote.
///
/// That is why the handler check is an **allowlist**. `args`, an unknown
/// `type`, a key omh has never heard of — each changes what runs, and a list of
/// keys to reject says nothing about the one the next release adds.
pub fn parse_hooks(
    raw: &str,
    vocab: &hook::Vocabulary,
) -> Result<(BTreeMap<String, hook::Hook>, Vec<hook::Dropped>)> {
    #[derive(Deserialize)]
    struct Document {
        #[serde(default)]
        hooks: BTreeMap<String, Vec<Entry>>,
    }
    #[derive(Deserialize)]
    struct Entry {
        #[serde(default)]
        matcher: String,
        #[serde(default)]
        hooks: Vec<serde_json::Value>,
    }

    let doc: Document = serde_json::from_str(raw).context("parsing this harness's hooks")?;
    let mut out: BTreeMap<String, hook::Hook> = BTreeMap::new();
    let mut residue = Vec::new();
    let mut note = |what: &str, wanted: String| {
        residue.push(hook::Dropped {
            name: what.to_string(),
            wanted,
        })
    };

    for (theirs, entries) in &doc.hooks {
        for (i, entry) in entries.iter().enumerate() {
            let at = format!("{theirs}[{i}]");
            let Some(on) = vocab.event(theirs) else {
                note(&at, format!("`{theirs}` is a moment omh has no word for"));
                continue;
            };
            // Claude's matcher is an unanchored regex, and at `SessionStart` it
            // is not a tool at all — `startup|resume|clear` is a different axis
            // entirely. Only a `|`-separated list of words this harness
            // declared as tools reads as a narrowing; everything else is a
            // narrowing omh would be inventing.
            let tools = match read_matcher(&entry.matcher, on, vocab) {
                Ok(tools) => tools,
                Err(why) => {
                    note(&at, why);
                    continue;
                }
            };
            for handler in &entry.hooks {
                match read_handler(handler) {
                    Ok(command) => {
                        let name = name_for(&out, on, &command);
                        out.insert(
                            name,
                            hook::Hook {
                                on,
                                stack: None,
                                tools: tools.clone(),
                                when: None,
                                action: hook::Action::Run(command),
                            },
                        );
                    }
                    Err(why) => note(&at, why),
                }
            }
        }
    }
    Ok((out, residue))
}

/// A matcher as a list of tools, or why it is not one.
fn read_matcher(
    matcher: &str,
    on: hook::Event,
    vocab: &hook::Vocabulary,
) -> std::result::Result<Vec<hook::Tool>, String> {
    if matcher.trim().is_empty() {
        // Not a narrowing: every tool this moment has, which is what omh's own
        // empty `tools` means.
        return Ok(Vec::new());
    }
    if on == hook::Event::SessionStart {
        return Err(format!(
            "`{matcher}` narrows a session start, which is not a tool — omh has \
             no word for that axis"
        ));
    }
    // **Not `split('|')`.** A harness's word for one omh tool may itself be an
    // alternation — Claude spells `edit` as `Edit|Write|MultiEdit` — so a
    // matcher is a `|`-join of spellings, each of which may contain `|`.
    // Splitting first turns one tool into three words that name none.
    //
    // Longest match first, so a spelling that is a prefix of another cannot
    // claim the shorter reading and strand the rest.
    let mut rest = matcher.trim();
    let mut tools = Vec::new();
    while !rest.is_empty() {
        let matched = vocab
            .spellings()
            .filter(|(word, _)| {
                rest.strip_prefix(*word)
                    .is_some_and(|after| after.is_empty() || after.starts_with('|'))
            })
            .max_by_key(|(word, _)| word.len());
        let Some((word, tool)) = matched else {
            return Err(format!(
                "`{matcher}` is not a list of tools omh knows — it does not \
                 continue with one at `{rest}`"
            ));
        };
        tools.push(tool);
        rest = rest[word.len()..].trim_start_matches('|');
    }
    Ok(tools)
}

/// A handler as the command it runs, or why omh cannot say it whole.
fn read_handler(handler: &serde_json::Value) -> std::result::Result<String, String> {
    let Some(object) = handler.as_object() else {
        return Err("a handler that is not an object".to_string());
    };
    for key in object.keys() {
        if key != "type" && key != "command" {
            return Err(format!(
                "a handler with `{key}`, which omh cannot express — importing \
                 the command without it would change what the hook does"
            ));
        }
    }
    match object.get("type").and_then(serde_json::Value::as_str) {
        Some("command") => {}
        Some(other) => return Err(format!("a `{other}` handler, which is not a command")),
        None => return Err("a handler with no `type`".to_string()),
    }
    match object.get("command").and_then(serde_json::Value::as_str) {
        Some(c) if !c.trim().is_empty() => Ok(c.to_string()),
        _ => Err("a command handler with no command".to_string()),
    }
}

/// A name for a hook whose original format had none.
///
/// From the moment and the program it runs, because those are what somebody
/// scanning `.omh/hooks/` needs to recognise it by — `after-tool-prettier`
/// rather than a number. `detect::program` answers `None` for a command it
/// cannot read, and a numbered fallback is better than a name built from shell.
fn name_for(taken: &BTreeMap<String, hook::Hook>, on: hook::Event, command: &str) -> String {
    let base = match crate::detect::program(command) {
        Some(p) => format!("{on}-{}", p.trim_start_matches("./").replace('/', "-")),
        None => format!("{on}-imported"),
    };
    if !taken.contains_key(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|n| !taken.contains_key(n))
        .expect("an unbounded range contains a free name")
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

    // ── reading a harness's hooks back ──────────────────────────────────────

    /// An adapter that spells every moment differently from Claude Code, so a
    /// parser with `"Stop"` written into it cannot pass.
    ///
    /// The whole point of the round trip is that the vocabulary travels through
    /// data. A fixture using Claude's own words would be satisfied by a reader
    /// that had them hardcoded, which is the bug this pair exists to prevent.
    fn foreign() -> (Binding, BTreeMap<hook::Tool, String>) {
        let binding: Binding = toml::from_str(
            "path = \"x\"\nrender = \"claude-settings\"\n\n\
             [events]\nturn-end = \"AfterTurn\"\nafter-tool = \"ToolFinished\"\n\
             session-start = \"Boot\"\n",
        )
        .unwrap();
        let tools = BTreeMap::from([
            (hook::Tool::Edit, "Modify".to_string()),
            (hook::Tool::Read, "Fetch".to_string()),
        ]);
        (binding, tools)
    }

    /// **Every format that renders must also parse, and the pair must round
    /// trip.** The rule `render::parse` already follows for MCP, applied to the
    /// capability that actually needs importing.
    ///
    /// Through the harness document, not through a fixture of one: what is
    /// asserted is that a hook rendered into somebody else's file comes back as
    /// the same hook, which is the only thing that makes `omh import` a
    /// translation rather than a guess.
    #[test]
    fn hooks_survive_a_round_trip_through_the_harness_document() {
        let (binding, tools) = foreign();
        let mine = hooks_named(&[
            ("tests", r#"{"on":"turn-end","run":"cargo test"}"#),
            (
                "fmt",
                r#"{"on":"after-tool","tools":["edit"],"run":"cargo fmt"}"#,
            ),
        ]);

        let (rendered, dropped) = translate(&mine, &binding, &tools).unwrap();
        assert!(dropped.is_empty(), "the fixture must render: {dropped:?}");
        let document = claude_settings(&rendered).unwrap();

        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let (back, residue) = parse_hooks(&document, &vocab).unwrap();
        assert!(residue.is_empty(), "nothing should be residue: {residue:?}");

        let recovered: BTreeSet<(hook::Event, Vec<hook::Tool>, String)> = back
            .values()
            .map(|h| (h.on, h.tools.clone(), h.does().to_string()))
            .collect();
        assert_eq!(
            recovered,
            mine.values()
                .map(|h| (h.on, h.tools.clone(), h.does().to_string()))
                .collect(),
            "a hook rendered into a harness's file must come back as itself"
        );
    }

    /// **Nothing is imported half-way.** A handler carrying anything omh cannot
    /// express is residue, reported by name — never a `command` brought across
    /// with the rest of its entry quietly dropped.
    ///
    /// `if` is the one that matters and is why this is an allowlist rather than
    /// a list of keys to ignore: it is a permission gate, so importing the
    /// command without it turns a hook that fires on one narrow case into one
    /// that fires on every call. The others are the same shape of loss —
    /// `args` changes what runs, a `type` omh does not know is not a command at
    /// all — and a key omh has never heard of is the case a denylist cannot
    /// cover.
    #[test]
    fn a_handler_omh_cannot_express_whole_is_not_imported_in_part() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();

        for (why, handler) in [
            (
                "a permission gate omh would drop",
                r#"{"type":"command","command":"fmt","if":"tool.name == 'Bash'"}"#,
            ),
            (
                "arguments that change what runs",
                r#"{"type":"command","command":"fmt","args":["--check"]}"#,
            ),
            (
                "a type that is not a command",
                r#"{"type":"builtin","command":"fmt"}"#,
            ),
            (
                "a key omh has never heard of",
                r#"{"type":"command","command":"fmt","onFailure":"block"}"#,
            ),
        ] {
            let doc =
                format!(r#"{{"hooks":{{"AfterTurn":[{{"matcher":"","hooks":[{handler}]}}]}}}}"#);
            let (imported, residue) = parse_hooks(&doc, &vocab).unwrap();
            assert!(imported.is_empty(), "{why}: imported anyway: {imported:?}");
            assert_eq!(residue.len(), 1, "{why}: not reported: {residue:?}");
        }
    }

    /// A matcher omh cannot read as tools is residue, and the two ways that
    /// happens are different mistakes.
    ///
    /// Claude's matcher grammar is **wider than `|`-alternation**: it is an
    /// unanchored regex, so `Edit.*` matches tools omh has no name for and
    /// importing it as `edit` would narrow somebody's hook without saying so.
    /// And at `SessionStart` a matcher is not a tool at all — it is
    /// `startup|resume|clear`, a completely different axis — so reading one as
    /// a tool name would be a category error that happens to typecheck.
    #[test]
    fn a_matcher_that_is_not_a_list_of_tools_is_residue() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();

        for (why, event, matcher) in [
            (
                "a regex is wider than the tools it mentions",
                "ToolFinished",
                "Modify.*",
            ),
            ("a tool this harness never declared", "ToolFinished", "Bash"),
        ] {
            let doc = format!(
                r#"{{"hooks":{{"{event}":[{{"matcher":"{matcher}","hooks":[{{"type":"command","command":"x"}}]}}]}}}}"#
            );
            let (imported, residue) = parse_hooks(&doc, &vocab).unwrap();
            assert!(imported.is_empty(), "{why}: {imported:?}");
            assert_eq!(residue.len(), 1, "{why}: not reported: {residue:?}");
        }

        // An empty matcher is not a narrowing — it is every tool this moment
        // has, which is what omh's own empty `tools` means.
        let doc = r#"{"hooks":{"AfterTurn":[{"matcher":"","hooks":[{"type":"command","command":"x"}]}]}}"#;
        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert_eq!(imported.len(), 1, "got: {residue:?}");
        assert!(imported.values().next().unwrap().tools.is_empty());
    }

    /// A harness's word for one omh tool may **itself be an alternation** —
    /// Claude spells `edit` as `Edit|Write|MultiEdit` — so a matcher is read by
    /// matching whole spellings against it, never by splitting on `|`.
    ///
    /// Both halves are failures. Splitting first turns one tool into three
    /// words that name none, so every hook narrowed to a tool becomes residue
    /// and nothing imports. And a *partial* alternation is not that tool:
    /// somebody matching `Edit|Write` deliberately excluded `MultiEdit`, and
    /// importing it as `edit` would widen their hook to fire where they had
    /// stopped it — the mirror of narrowing, and just as silent.
    ///
    /// Found end to end rather than here: the fixture below spells tools as
    /// single words, so the round trip passed against a parser that could not
    /// read the adapter omh actually ships.
    #[test]
    fn a_tool_spelled_as_an_alternation_is_one_tool() {
        let adapter = crate::adapter::Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let binding = adapter.supports(Capability::Hooks).unwrap();
        let vocab = hook::Vocabulary::of(binding, &adapter.tools).unwrap();

        let doc = |matcher: &str| {
            format!(
                r#"{{"hooks":{{"PostToolUse":[{{"matcher":"{matcher}","hooks":[{{"type":"command","command":"fmt"}}]}}]}}}}"#
            )
        };

        let (imported, residue) = parse_hooks(&doc("Edit|Write|MultiEdit"), &vocab).unwrap();
        assert_eq!(
            imported.len(),
            1,
            "one tool, however it is spelled: {residue:?}"
        );
        assert_eq!(
            imported.values().next().unwrap().tools,
            vec![hook::Tool::Edit]
        );

        // Two of them, joined — still whole spellings, not six words.
        let (imported, residue) = parse_hooks(&doc("Edit|Write|MultiEdit|Read"), &vocab).unwrap();
        assert_eq!(imported.len(), 1, "got: {residue:?}");
        assert_eq!(
            imported.values().next().unwrap().tools,
            vec![hook::Tool::Edit, hook::Tool::Read]
        );

        // And a partial alternation is **not** that tool.
        let (imported, residue) = parse_hooks(&doc("Edit|Write"), &vocab).unwrap();
        assert!(
            imported.is_empty(),
            "importing this as `edit` would widen a hook that deliberately \
             excluded MultiEdit: {imported:?}"
        );
        assert_eq!(residue.len(), 1, "and it is reported: {residue:?}");
    }

    /// At session start a matcher is **not a tool** — Claude's are
    /// `startup|resume|clear`, a different axis entirely — and the guard for
    /// that only bites where the word also happens to name a tool.
    ///
    /// So the fixture makes it: a harness that spells `edit` as `startup`.
    /// Contrived, and it is exactly the collision the guard exists for. Without
    /// it, a hook that fired when a session resumed would be imported as a hook
    /// that fires on every edit — a category error that typechecks, produces a
    /// valid hook, and is wrong in a way nothing downstream can notice.
    ///
    /// Written after deleting the guard changed no test: the first version of
    /// this used a word no tool was spelled as, so the tool lookup failed
    /// anyway and the guard was decoration.
    #[test]
    fn a_session_start_matcher_is_never_read_as_a_tool() {
        let (binding, _) = foreign();
        let colliding = BTreeMap::from([(hook::Tool::Edit, "startup".to_string())]);
        let vocab = hook::Vocabulary::of(&binding, &colliding).unwrap();

        let doc = r#"{"hooks":{"Boot":[{"matcher":"startup","hooks":[{"type":"command","command":"x"}]}]}}"#;
        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert!(
            imported.is_empty(),
            "a session-start matcher became a tool narrowing: {imported:?}"
        );
        assert_eq!(residue.len(), 1, "and is reported: {residue:?}");
    }

    /// A moment this harness has and omh does not is residue rather than an
    /// error: the rest of somebody's hooks still come across, and the one that
    /// did not is named.
    #[test]
    fn a_moment_omh_has_no_word_for_is_reported_not_fatal() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = r#"{"hooks":{
            "AfterTurn":[{"matcher":"","hooks":[{"type":"command","command":"keep me"}]}],
            "PreCompact":[{"matcher":"","hooks":[{"type":"command","command":"drop me"}]}]}}"#;

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert_eq!(imported.len(), 1, "the rest still come across");
        assert_eq!(residue.len(), 1);
        assert!(
            residue[0].wanted.contains("PreCompact"),
            "and the residue names what it was: {residue:?}"
        );
    }

    // ── suppression by measurement ──────────────────────────────────────────

    fn hooks_named(pairs: &[(&str, &str)]) -> BTreeMap<String, hook::Hook> {
        pairs
            .iter()
            .map(|(name, body)| {
                (
                    name.to_string(),
                    hook::Hook::parse(body, name).expect("fixture must be a valid hook"),
                )
            })
            .collect()
    }

    /// A repo states what its hooks are; a **measurement** states what the
    /// sandbox can run. The hook file is committed and travels, so a toolchain
    /// missing in this image must never remove it from the repo — it stops the
    /// hook firing against this image, and is reported by name so the absence
    /// is never silent.
    #[test]
    fn a_hook_needing_a_program_the_image_lacks_is_dropped_by_name() {
        let mut hooks = hooks_named(&[
            ("rust-test", r#"{"on":"turn-end","run":"cargo test"}"#),
            ("greet", r#"{"on":"turn-end","run":"echo hi"}"#),
        ]);
        let measured = BTreeMap::from([("cargo".to_string(), false)]);
        let dropped = suppressed_by_probe(&mut hooks, &measured);

        assert_eq!(dropped.len(), 1, "one hook is held back: {dropped:?}");
        assert_eq!(dropped[0].name, "rust-test");
        assert!(
            dropped[0].wanted.contains("cargo"),
            "and it names the program: {:?}",
            dropped[0]
        );
        assert!(
            !hooks.contains_key("rust-test"),
            "a suppressed hook must not reach the harness"
        );
        assert!(
            hooks.contains_key("greet"),
            "and nothing else is disturbed: {:?}",
            hooks.keys().collect::<Vec<_>>()
        );
    }

    /// Only a **measured absence** suppresses. A program measured present, and
    /// a program nobody measured at all, both leave the hook alone.
    ///
    /// The second half carries the weight: `facts::Facts::resolves` answers
    /// `None` for a program nobody probed, and that must reach here as *not in
    /// the map* rather than as `false`. A first run, a hook added since the
    /// last probe, a cache somebody deleted — all of them look identical to an
    /// empty sandbox if silence is read as absence, and omh would ship a
    /// session with every hook switched off and no reason given.
    #[test]
    fn only_a_measured_absence_suppresses_and_an_unmeasured_program_never_does() {
        let present = BTreeMap::from([("cargo".to_string(), true)]);
        let mut hooks = hooks_named(&[("rust-test", r#"{"on":"turn-end","run":"cargo test"}"#)]);
        assert!(
            suppressed_by_probe(&mut hooks, &present).is_empty(),
            "the probe found it — that is the answer that keeps a hook working"
        );
        assert!(hooks.contains_key("rust-test"));

        // And a program nobody has measured is simply run.
        let mut hooks = hooks_named(&[("rust-test", r#"{"on":"turn-end","run":"cargo test"}"#)]);
        assert!(
            suppressed_by_probe(&mut hooks, &BTreeMap::new()).is_empty(),
            "silence is cannot-tell, and cannot-tell is never a licence to act"
        );
        assert!(hooks.contains_key("rust-test"));
    }

    /// Suppression follows the *command*, not the stack that produced it, so a
    /// hook somebody wrote by hand is covered by the same answer. That is what
    /// makes this general rather than a rule about the four stacks omh detects.
    #[test]
    fn a_hand_written_hook_obeys_the_same_answer() {
        let mut hooks = hooks_named(&[
            (
                "mine",
                r#"{"on":"after-tool","tools":["edit"],"run":"cargo clippy"}"#,
            ),
            // An injected capture shells out too, so it is judged the same way.
            (
                "ask",
                r#"{"on":"turn-end","capture":"cargo metadata","inject":"$OMH_CAPTURE"}"#,
            ),
            // A refusal runs nothing at all and can never need a toolchain.
            (
                "refuse",
                r#"{"on":"before-tool","tools":["edit"],"refuse":"no"}"#,
            ),
        ]);
        let measured = BTreeMap::from([("cargo".to_string(), false)]);
        let dropped = suppressed_by_probe(&mut hooks, &measured);

        let names: BTreeSet<&str> = dropped.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            BTreeSet::from(["mine", "ask"]),
            "both shell out to cargo: {dropped:?}"
        );
        assert!(
            hooks.contains_key("refuse"),
            "a refusal runs nothing and cannot be blocked by a missing program"
        );
    }

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
        // A `Table`, not a `Value`: what this renders is a TOML *document*, and
        // since toml 1.0 parsing into `Value` reads a single value expression
        // instead — which takes `[mcp_servers.a]` for an array and then refuses
        // the rest of the file. Both spellings compiled; only one asks the
        // question this test is for.
        let reparsed: toml::Table = codex.parse().expect("codex output must be valid TOML");
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
        out.parse::<toml::Table>()
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
            &Default::default(),
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

    /// The other half of the `mcp` binding, which lands in this document
    /// because this is the only file the harness reads it from.
    ///
    /// A project-scoped MCP document that has not been approved is *listed* and
    /// not *loaded* — servers sit at "pending approval" waiting for a dialog no
    /// unattended session can answer, and every check that reads the document
    /// rather than asking the harness stays green throughout. Mounting the
    /// document is therefore only half of handing it over.
    #[test]
    fn the_settings_document_approves_the_mcp_document_omh_mounts() {
        let dir = tempfile::tempdir().unwrap();
        file(
            dir.path(),
            "h/a.json",
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
            &Default::default(),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();

        assert_eq!(
            v["enableAllProjectMcpServers"], true,
            "the mounted document would be listed and never loaded: {}",
            out.body
        );
        assert!(
            v["hooks"]["Stop"].is_array(),
            "and the hooks this file exists for still ship: {}",
            out.body
        );
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
            &Default::default(),
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
    /// What `init` reports held back is exactly what a launch holds back.
    ///
    /// `init` has no document to render, so it asks `held_back` directly while
    /// the launcher goes through `document`. Two paths, one answer — and the
    /// report is the thing people read to find out what they got, so a report
    /// naming a different set from the session's is worse than no report at
    /// all: it would send somebody away believing a hook was running.
    ///
    /// Asserted as agreement between the two rather than against a fixed list,
    /// because the failure is *divergence*, and a list would go on passing
    /// while both drifted together.
    #[test]
    fn what_init_reports_held_back_is_what_a_launch_holds_back() {
        let dir = tempfile::tempdir().unwrap();
        file(
            dir.path(),
            "h/rust-test.json",
            r#"{"on":"turn-end","run":"cargo test"}"#,
        );
        file(
            dir.path(),
            "h/lint.json",
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        );
        file(
            dir.path(),
            "h/greet.json",
            r#"{"on":"turn-end","run":"echo hi"}"#,
        );
        let dirs = [dir.path().join("h")];
        let measured = BTreeMap::from([
            ("cargo".to_string(), false),
            ("shellcheck".to_string(), false),
            ("echo".to_string(), true),
        ]);

        let reported: BTreeSet<String> =
            held_back(&dirs, &Default::default(), &Default::default(), &measured)
                .unwrap()
                .into_iter()
                .map(|d| d.name)
                .collect();

        let adapter = claude_hooks();
        let shipped: BTreeSet<String> = document(
            Capability::Hooks,
            hooks_binding(&adapter),
            &dirs,
            &Default::default(),
            &Default::default(),
            &adapter.tools,
            &measured,
        )
        .unwrap()
        .dropped
        .into_iter()
        .map(|d| d.name)
        .collect();

        assert_eq!(reported, shipped, "the report and the session disagree");
        assert_eq!(
            reported,
            BTreeSet::from(["rust-test".to_string(), "lint".to_string()]),
            "and both must be the measured absences, not everything or nothing"
        );
    }

    /// Suppression is a rule about **this repo**, so it has to hold on every
    /// render path — asserted through `document`, which is what the launcher
    /// calls, rather than through `suppressed_by_probe`, which is what the
    /// three tests above call.
    ///
    /// The distinction is not academic. The unit tests would all pass with the
    /// call deleted from one arm of `document`'s match, and the result would be
    /// a hook that ships on opencode and not on claude — so which automation a
    /// repo gets would depend on which harness you happened to launch. That is
    /// not a weaker version of the rule; it is a different rule per harness,
    /// which is the thing this module exists to prevent.
    #[test]
    fn a_measured_absence_holds_on_every_render_path() {
        let dir = tempfile::tempdir().unwrap();
        file(
            dir.path(),
            "h/rust-test.json",
            r#"{"on":"turn-end","run":"cargo test"}"#,
        );
        file(
            dir.path(),
            "h/greet.json",
            r#"{"on":"turn-end","run":"echo hi"}"#,
        );
        let measured = BTreeMap::from([("cargo".to_string(), false)]);

        let claude = claude_hooks();
        // Every render path, which is what "on every render path" means — a
        // third arm was added to `document`'s match without this list moving,
        // and deleting `suppressed_by_probe` from that arm alone left the suite
        // green. This test's own doc predicts exactly that.
        let paths: [(&Binding, &BTreeMap<hook::Tool, String>); 3] = [
            (hooks_binding(&claude), &claude.tools),
            (opencode_hooks(), &opencode().tools),
            (omp_hooks(), &omp().tools),
        ];
        for (binding, tools) in paths {
            let out = document(
                Capability::Hooks,
                binding,
                &[dir.path().join("h")],
                &Default::default(),
                &Default::default(),
                tools,
                &measured,
            )
            .unwrap();

            assert!(
                !out.body.contains("cargo test"),
                "{:?}: a hook the sandbox cannot run reached the harness:\n{}",
                binding.render,
                out.body
            );
            assert!(
                out.dropped.iter().any(|d| d.name == "rust-test"),
                "{:?}: and it must be named, never silently absent: {:?}",
                binding.render,
                out.dropped
            );
            assert!(
                out.body.contains("echo hi"),
                "{:?}: nothing else is disturbed:\n{}",
                binding.render,
                out.body
            );
        }
    }

    fn opencode() -> &'static Adapter {
        static CELL: std::sync::OnceLock<Adapter> = std::sync::OnceLock::new();
        CELL.get_or_init(|| Adapter::find(Path::new(ADAPTERS), "opencode").unwrap())
    }

    fn opencode_hooks() -> &'static Binding {
        opencode()
            .supports(Capability::Hooks)
            .expect("opencode has hooks")
    }

    fn omp() -> &'static Adapter {
        static CELL: std::sync::OnceLock<Adapter> = std::sync::OnceLock::new();
        CELL.get_or_init(|| Adapter::find(Path::new(ADAPTERS), "omp").unwrap())
    }

    fn omp_hooks() -> &'static Binding {
        omp().supports(Capability::Hooks).expect("omp has hooks")
    }

    fn omp_module(hooks: &[(&str, &str)]) -> Document {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in hooks {
            file(dir.path(), &format!("h/{name}.json"), body);
        }
        document(
            Capability::Hooks,
            omp_hooks(),
            &[dir.path().join("h")],
            &Default::default(),
            &Default::default(),
            &omp().tools,
            &Default::default(),
        )
        .unwrap()
    }

    fn dropped_for(doc: &Document, name: &str) -> String {
        doc.dropped
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} was not dropped: {:?}", doc.dropped))
            .wanted
            .clone()
    }

    /// Each moment omh knows about registers itself under omp's own name for
    /// it — and under the *right* one.
    ///
    /// This asserted a set: that each of omp's four event names appeared
    /// somewhere in the module. Swapping `session-start` and `turn-end` in the
    /// adapter leaves that set unchanged, so the one table whose entire content
    /// is a mapping was checked per-set and the swap passed the whole suite.
    /// Each hook is now paired with the event its own moment must produce.
    #[test]
    fn every_moment_omp_has_registers_itself_under_the_right_name() {
        for (name, moment, event) in [
            ("fmt", "after-tool", "tool_result"),
            ("test", "turn-end", "turn_end"),
            ("orient", "session-start", "session_start"),
            ("guard", "before-tool", "tool_call"),
        ] {
            let body = format!(r#"{{"on":"{moment}","run":"echo hi"}}"#);
            let doc = omp_module(&[(name, body.as_str())]);
            assert!(doc.dropped.is_empty(), "dropped: {:?}", doc.dropped);
            assert!(
                doc.body.contains(&format!("pi.on({event:?}")),
                "omh's `{moment}` must reach omp as `{event}`: {}",
                doc.body
            );
        }
    }

    /// The two protocols land in the two moments that have them, in omp's
    /// words rather than omh's.
    #[test]
    fn omp_blocks_before_the_call_and_rewrites_after_it() {
        let doc = omp_module(&[
            (
                "guard",
                r#"{"on":"before-tool","tools":["shell"],"refuse":"denied"}"#,
            ),
            (
                "nudge",
                r#"{"on":"after-tool","tools":["read"],"inject":"noted"}"#,
            ),
        ]);
        assert!(
            doc.body.contains("return { block: true, reason:"),
            "no block protocol: {}",
            doc.body
        );
        assert!(
            doc.body
                .contains("return { content: [...(event.content ?? []),"),
            "no content-append protocol: {}",
            doc.body
        );
        // The tool guard reads omp's own word for the tool, off the property
        // omp puts it on.
        assert!(
            doc.body
                .contains(r#"if (!["bash"].includes(event.toolName)) return"#),
            "tool guard is not omp's: {}",
            doc.body
        );
    }

    /// A nudge is never promoted to a wall.
    ///
    /// `tool_call` can block or say nothing, so advisory text has no channel
    /// there — the one translation omh refuses to make silently. Dropped **by
    /// name**, saying what it asked for.
    ///
    /// The mirror case, a `refuse` at `after-tool`, is not tested here because
    /// it cannot reach a renderer: omh refuses that pairing when the hook is
    /// parsed. Writing this test is what established that — the renderer had a
    /// branch for it, and the branch was unreachable.
    #[test]
    fn omp_never_swaps_a_nudge_for_a_wall() {
        let doc = omp_module(&[(
            "early",
            r#"{"on":"before-tool","tools":["read"],"inject":"advice"}"#,
        )]);
        assert_eq!(
            dropped_for(&doc, "early"),
            "way to inject text before a tool runs"
        );
        assert!(
            !doc.body.contains("event.content"),
            "a dropped hook still reached the module: {}",
            doc.body
        );
    }

    /// A moment with no call in scope cannot narrow to a tool or read a field
    /// off one, and says so rather than binding an empty string.
    #[test]
    fn a_moment_without_a_call_admits_it_has_no_payload() {
        let doc = omp_module(&[
            (
                "narrow",
                r#"{"on":"turn-end","tools":["edit"],"run":"true"}"#,
            ),
            (
                "field",
                r#"{"on":"session-start","run":"echo $OMH_TOOL_FILE"}"#,
            ),
        ]);
        assert_eq!(
            dropped_for(&doc, "narrow"),
            "way to narrow to a tool at `turn-end`"
        );
        assert_eq!(
            dropped_for(&doc, "field"),
            "payload field at `session-start`"
        );
    }

    /// Two hooks that both speak at the same moment both get to speak.
    ///
    /// omp runs one handler per registration, in registration order, and
    /// `tool_result` handlers **chain** — each sees the previous one's
    /// modifications. Collapsing several omh hooks into one registration threw
    /// that away: the first hook to return ended the handler, so a second
    /// injecting hook at the same moment was silently never run. Nothing said
    /// so — it was not dropped, not warned about, just absent.
    ///
    /// Asserted as one registration per surviving hook, which is the invariant
    /// that keeps omp's ordering rules meaning what omp says they mean.
    #[test]
    fn hooks_sharing_a_moment_each_get_their_own_handler() {
        let doc = omp_module(&[
            (
                "first",
                r#"{"on":"after-tool","tools":["read"],"inject":"one"}"#,
            ),
            (
                "second",
                r#"{"on":"after-tool","tools":["read"],"inject":"two"}"#,
            ),
        ]);
        assert!(doc.dropped.is_empty(), "dropped: {:?}", doc.dropped);
        assert_eq!(
            doc.body.matches(r#"pi.on("tool_result""#).count(),
            2,
            "both hooks must reach the model, not just the first: {}",
            doc.body
        );
        for text in ["\"one\"", "\"two\""] {
            assert!(
                doc.body.contains(text),
                "{text} never reaches the module: {}",
                doc.body
            );
        }
    }

    /// The staged file has to be something a JavaScript runtime will load.
    ///
    /// The omp generator is a hand-built string generator like opencode's, and
    /// a module that does not parse takes **every** hook down at once rather
    /// than the one that broke it. It is fed the same awkward body opencode's
    /// is — quotes, backslashes, backticks, a literal `}`, a `%`, and an `awk`
    /// program whose braces would defeat any brace-counting check.
    ///
    /// `#[ignore]` for the reason opencode's twin is: it needs `node`. Run with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn an_omp_module_is_something_a_runtime_can_load() {
        let body = omp_module(&[
            ("fmt", r#"{"on":"turn-end","run":"cargo fmt"}"#),
            (
                "awkward",
                r#"{"on":"before-tool","tools":["shell"],"when":"awk '{print $1}' </dev/null; case \"$OMH_TOOL_COMMAND\" in *\"}\"*) ;; *) false ;; esac","refuse":"no \" quote, back\\slash, `tick`, 100%"}"#,
            ),
            (
                "noisy",
                r#"{"on":"after-tool","tools":["read"],"inject":"back\\slash `tick` \" quote 100% $OMH_TOOL_FILE"}"#,
            ),
        ])
        .body;
        assert!(
            body.contains("export default function"),
            "omp imports the default export: {body}"
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

    /// An event name omp does not have is an adapter error, not a harness
    /// limitation, and it says so.
    ///
    /// `Moment::of` classified anything unrecognised as "no call in scope", so
    /// a typo in `[capabilities.hooks.events]` came back as *"no way to refuse
    /// a call at `before-tool`"* — a statement that oh-my-pi cannot block a
    /// tool call, which is untrue and sends the reader to the harness instead
    /// of to the one-word mismatch. The drop rule is "say what it asked for";
    /// blaming the harness for the adapter fails it.
    #[test]
    fn an_event_omp_does_not_have_is_named_as_an_adapter_error() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("typo.toml"),
            r#"
name    = "typo"
bin     = "typo"
install = "true"

[tools]
shell = "bash"

[capabilities.hooks]
path   = "$HOME/.typo/omh.ts"
render = "omp-plugin"

[capabilities.hooks.events]
before-tool = "toolcall"

[capabilities.hooks.refuse]
template = 'return { block: true, reason: {{text}} }'
"#,
        )
        .unwrap();
        let adapter = Adapter::find(d.path(), "typo").unwrap();
        let binding = adapter.supports(Capability::Hooks).unwrap();
        let src = tempfile::tempdir().unwrap();
        file(
            src.path(),
            "h/guard.json",
            r#"{"on":"before-tool","tools":["shell"],"refuse":"no"}"#,
        );
        let err = document(
            Capability::Hooks,
            binding,
            &[src.path().join("h")],
            &Default::default(),
            &Default::default(),
            &adapter.tools,
            &Default::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("toolcall"),
            "the refusal must name the event nobody has: {err}"
        );
    }

    /// A guard that could not be evaluated blocks the call it guards.
    ///
    /// `when` failing to run is not the same as `when` declining. Both left
    /// `p.code` at 1 and returned, and returning from a `tool_call` handler
    /// means *allow* — so `git-unavailable`, under fork pressure or in an image
    /// where `/bin/sh` is not where the runtime looks, let every git call
    /// through while `doctor` still reported the module present and correct.
    ///
    /// A `refuse` whose predicate is unavailable must fail closed. An `inject`
    /// or a `run` in the same state still degrades to a no-op, because there
    /// the no-op *is* the safe answer — the asymmetry is the whole point.
    #[test]
    fn a_refusal_whose_guard_cannot_be_evaluated_blocks() {
        let doc = omp_module(&[(
            "guard",
            r#"{"on":"before-tool","tools":["shell"],"when":"test -x /nope","refuse":"use omh commit"}"#,
        )]);
        assert!(
            doc.body.contains("if (!p.ran) return { block: true"),
            "an unevaluable guard on a refusal must block, not fall through: {}",
            doc.body
        );
        let advisory = omp_module(&[(
            "nudge",
            r#"{"on":"after-tool","tools":["read"],"when":"test -x /nope","inject":"noted"}"#,
        )]);
        assert!(
            !advisory.body.contains("block: true"),
            "an advisory hook must still degrade to silence: {}",
            advisory.body
        );
    }

    /// `warn` says which of the three things happened.
    ///
    /// It hard-coded "did not run" and fired for any non-zero exit, so a
    /// `turn-end` hook running `cargo test` against a failing suite reported
    /// `hook test: its \`run\` did not run` — a false statement, with `r.out`
    /// discarded, leaving a user debugging it no evidence at all.
    #[test]
    fn a_warning_distinguishes_not_running_from_running_and_failing() {
        let body = omp_module(&[("test", r#"{"on":"turn-end","run":"cargo test"}"#)]).body;
        assert!(
            body.contains("could not run") && body.contains("exited"),
            "the bridge must be able to say both things: {body}"
        );
    }

    /// A file path on omp's `edit` is a thing this harness cannot say, so the
    /// hook wanting it is dropped by name rather than handed an empty string.
    ///
    /// omp's edit tool takes one `input` string with the path embedded in
    /// `[PATH#TAG]` sections, so `event.input.path` is never there. The adapter
    /// wrote that down and the renderer emitted the binding anyway: the hook
    /// shipped, bound `""`, and never fired — present in the module, present in
    /// `omh doctor`'s name list, absent from `dropped`, and indistinguishable
    /// from a hook with nothing to say. Naming it is the whole rule.
    #[test]
    fn a_file_path_on_omps_edit_tool_is_dropped_by_name() {
        let doc = omp_module(&[(
            "fmt-one",
            r#"{"on":"after-tool","tools":["edit"],"run":"prettier $OMH_TOOL_FILE"}"#,
        )]);
        let wanted = dropped_for(&doc, "fmt-one");
        assert!(
            wanted.contains("tool-file") && wanted.contains("edit"),
            "the drop must name the field and the tool it cannot come from: {wanted}"
        );
        assert!(
            !doc.body.contains(r#"env["OMH_TOOL_FILE"]"#),
            "a dropped hook left its binding behind: {}",
            doc.body
        );
    }

    /// The same field on `read` is fine, and stays.
    ///
    /// The pair matters: dropping every `tool-file` hook would cost omp a
    /// capability it has, which is the other half of the capability floor.
    #[test]
    fn a_file_path_on_omps_read_tool_still_works() {
        let doc = omp_module(&[(
            "cite",
            r#"{"on":"after-tool","tools":["read"],"run":"echo $OMH_TOOL_FILE"}"#,
        )]);
        assert!(doc.dropped.is_empty(), "dropped: {:?}", doc.dropped);
        assert!(
            doc.body
                .contains(r#"env["OMH_TOOL_FILE"] = String(event.input?.path ?? "")"#),
            "read's path is real and must still be bound: {}",
            doc.body
        );
    }

    /// The inject protocol survives a tool result that carries no content.
    ///
    /// `[...undefined]` throws, and a tool returning nothing is ordinary. The
    /// generated module has no `try`/`catch` anywhere, and this file's own
    /// bridge says a hook degrades to a no-op rather than to an error — while
    /// on omp a throw out of a handler becomes model-visible error text, which
    /// turns a nudge into exactly the wall the renderer exists to prevent.
    #[test]
    fn injecting_into_an_empty_tool_result_does_not_throw() {
        let doc = omp_module(&[(
            "cite",
            r#"{"on":"after-tool","tools":["read"],"inject":"noted"}"#,
        )]);
        assert!(
            doc.body.contains("event.content ?? []"),
            "the spread must tolerate a result with no content: {}",
            doc.body
        );
    }

    /// A `when` predicate and a `capture` both reach the module.
    ///
    /// Neither had a guard on this path, and deleting both emission blocks left
    /// the suite green. That is not academic: the shipped `git-unavailable`
    /// hook is `before-tool` + `refuse` narrowed to the shell tool, and its
    /// `when` predicate is the **only** thing restricting it to git commands —
    /// losing it turns "block git" into "block every bash call", silently.
    #[test]
    fn a_when_predicate_and_a_capture_reach_the_omp_module() {
        let doc = omp_module(&[
            (
                "guard",
                r#"{"on":"before-tool","tools":["shell"],"when":"case \"$OMH_TOOL_COMMAND\" in git*) ;; *) false ;; esac","refuse":"use omh commit"}"#,
            ),
            (
                "orient",
                r#"{"on":"after-tool","tools":["read"],"capture":"omh graph query","inject":"context: $OMH_CAPTURE"}"#,
            ),
        ]);
        assert!(doc.dropped.is_empty(), "dropped: {:?}", doc.dropped);
        assert!(
            doc.body.contains("in git*)") && doc.body.contains("if (p.code !== 0) return"),
            "the `when` predicate and its gate must both be emitted: {}",
            doc.body
        );
        assert!(
            doc.body.contains("omh graph query") && doc.body.contains("OMH_CAPTURE"),
            "the capture and the variable it binds must both be emitted: {}",
            doc.body
        );
    }

    /// omp keeps a call's arguments on `event.input`, and that is asserted
    /// rather than described.
    ///
    /// Reading the wrong parameter does not fail — it binds the empty string,
    /// so the hook simply never fires and nothing says why. opencode has the
    /// same guard for the same reason; omp's claim to differ from it was the
    /// untested half.
    #[test]
    fn the_payload_field_is_read_where_omp_keeps_it() {
        let doc = omp_module(&[(
            "guard",
            r#"{"on":"before-tool","tools":["shell"],"when":"test -n \"$OMH_TOOL_COMMAND\"","refuse":"no"}"#,
        )]);
        assert!(
            doc.body
                .contains(r#"env["OMH_TOOL_COMMAND"] = String(event.input?.command ?? "")"#),
            "the field must be read off `event.input`, under omp's name for it: {}",
            doc.body
        );
    }

    /// Text that names a payload field is expanded, not emitted raw.
    ///
    /// Every other omp test uses literal inject text, so the `t()` wrapper —
    /// which runs the text through the same shell that bound `$OMH_*` — could
    /// be removed without a red test. Without it a hook's references reach the
    /// model as the characters `$OMH_TOOL_FILE`.
    #[test]
    fn text_naming_a_field_is_expanded_on_omp() {
        let doc = omp_module(&[(
            "cite",
            r#"{"on":"after-tool","tools":["read"],"inject":"see $OMH_TOOL_FILE"}"#,
        )]);
        assert!(
            doc.body.contains(r#"t("cite", "see $OMH_TOOL_FILE""#),
            "the text must go through the expander with its raw form beside it: {}",
            doc.body
        );
    }

    /// A hook's name never lands where JavaScript would read it as code.
    ///
    /// omh's own hook names carry `-` (`graph-read`, `git-unavailable`), and
    /// `-` is a minus sign in an identifier position: `const r_graph-read` does
    /// not parse, and a module that does not parse takes every *other* hook
    /// down with it.
    ///
    /// This began as a test that the name was *slugged* into an identifier,
    /// which is how the first generator held its per-hook result. Giving each
    /// hook its own `pi.on` deleted the identifier, and with it the entire bug
    /// class — so the guard now asserts the property that made slugging
    /// necessary, rather than the mechanism that used to satisfy it. A test
    /// pinned to the mechanism would have gone green on an implementation that
    /// no longer has one.
    #[test]
    fn a_hyphenated_hook_name_never_reaches_code_position() {
        let doc = omp_module(&[(
            "graph-read",
            r#"{"on":"after-tool","tools":["read"],"inject":"context"}"#,
        )]);
        let loose: Vec<&str> = doc
            .body
            .lines()
            .filter(|l| l.contains("graph-read"))
            .filter(|l| !l.trim_start().starts_with("//") && !l.contains("\"graph-read\""))
            .collect();
        assert!(
            loose.is_empty(),
            "the name appears outside a comment or a string literal: {loose:?}"
        );
        assert!(
            doc.body.contains("// graph-read"),
            "the hook is not labelled in the module at all: {}",
            doc.body
        );
    }

    #[test]
    #[ignore]
    fn show_the_omp_module() {
        println!("{}", omp_module(&[
            ("fmt", r#"{"on":"after-tool","tools":["edit"],"run":"cargo fmt"}"#),
            ("graph-read", r#"{"on":"after-tool","tools":["read"],"inject":"see the graph"}"#),
            ("git-unavailable", r#"{"on":"before-tool","tools":["shell"],"when":"case \"$OMH_TOOL_COMMAND\" in git*) ;; *) false ;; esac","refuse":"use omh commit"}"#),
            ("test", r#"{"on":"turn-end","run":"cargo test"}"#),
        ]).body);
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
            &Default::default(),
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
            &Default::default(),
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
            &Default::default(),
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

    /// Every program the hooks that would *ship* name, asked about as one set.
    ///
    /// The gap this closes is easy to miss and cost the whole first design: a
    /// stack's `needs` lists what a **stack** declared, and suppression has to
    /// cover every program a **hook** names. A hand-written `shellcheck` hook
    /// appears in no `needs` list, so a probe built from `needs` alone never
    /// asks about it, `shellcheck` is missing, and the hook ships anyway — the
    /// `cargo: not found` failure in a different spelling.
    ///
    /// Read through `merge_hooks`, so it is the same set the renderer will
    /// ship: a hook `[use]` did not select, or one whose file is not there, is
    /// not a program worth a question. Two ways of deciding which hooks count
    /// is how the probe starts answering about a hook nobody runs.
    #[test]
    fn every_program_a_shipped_hook_would_run_is_asked_about() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("h");
        file(
            &hooks,
            "lint.json",
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        );
        // A refusal shells out to nothing, so it can never need a program.
        file(
            &hooks,
            "no.json",
            r#"{"on":"before-tool","tools":["edit"],"refuse":"no"}"#,
        );
        // And a command omh cannot read is *cannot tell* — `detect::program`
        // answers `None` rather than guessing `$(which`, and a question about
        // a program nobody named would report a gap omh invented.
        file(
            &hooks,
            "clever.json",
            r#"{"on":"turn-end","run":"$(which cargo) test"}"#,
        );

        let own = crate::base::Own {
            hooks: vec![crate::base::Hook {
                name: "graph-refresh",
                hook: hook::Hook::parse(r#"{"on":"turn-end","run":"omh-graph index"}"#, "own")
                    .unwrap(),
            }],
            ..Default::default()
        };

        let asked = hook_programs(std::slice::from_ref(&hooks), &own, &Default::default()).unwrap();

        assert!(
            asked.contains("shellcheck"),
            "a hand-written hook's program is in no stack's `needs`: {asked:?}"
        );
        assert!(
            asked.contains("omh-graph"),
            "omh's own hooks run programs too: {asked:?}"
        );
        assert_eq!(
            asked,
            BTreeSet::from(["shellcheck".to_string(), "omh-graph".to_string()]),
            "a refusal runs nothing, and a command omh cannot read is not a gap \
             it may invent: {asked:?}"
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
            &Default::default(),
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

    // ── candidate guards (mutation testing) ─────────────────────────────────

    /// **Two handlers running the same program at the same moment are two
    /// hooks.** The name is derived, so it collides — and a collision that
    /// overwrote would drop one of somebody's hooks on import, silently, with
    /// nothing in the residue to say it had gone.
    #[test]
    fn two_hooks_that_would_share_a_name_are_both_imported() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = "{\"hooks\":{\"AfterTurn\":[\
            {\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"prettier a\"}]},\
            {\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"prettier b\"}]}]}}";

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert_eq!(
            imported.len(),
            2,
            "one was overwritten by the other: {imported:?} {residue:?}"
        );
        let commands: BTreeSet<&str> = imported.values().map(|h| h.does()).collect();
        assert_eq!(commands.len(), 2, "and they are the two that were written");
    }

    /// **A hook name is a filename**, so a command run from a path must not put
    /// a separator into it. `./bin/fmt` naming a hook `after-tool-./bin/fmt`
    /// would be written into a directory nobody asked for, or not at all.
    #[test]
    fn a_hook_named_after_a_path_is_still_one_filename() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = "{\"hooks\":{\"AfterTurn\":[{\"matcher\":\"\",\"hooks\":[\
            {\"type\":\"command\",\"command\":\"./bin/fmt --check\"}]}]}}";

        let (imported, _) = parse_hooks(doc, &vocab).unwrap();
        let name = imported.keys().next().expect("must import").clone();
        assert!(
            !name.contains('/') && !name.contains('.'),
            "a hook name is a filename, not a path: {name}"
        );
    }

    /// **A blank command is not a command.** A handler whose `command` is
    /// whitespace imports as a hook that runs a shell doing nothing, at the end
    /// of every turn, in a file omh put there and attributed to omh.
    #[test]
    fn a_handler_with_a_blank_command_is_residue() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = "{\"hooks\":{\"AfterTurn\":[{\"matcher\":\"\",\"hooks\":[\
            {\"type\":\"command\",\"command\":\"   \"}]}]}}";

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert!(imported.is_empty(), "imported anyway: {imported:?}");
        assert_eq!(residue.len(), 1, "and it is reported: {residue:?}");
    }

    /// A session-start hook is the ordinary shape — **no matcher at all** — and
    /// it has to import. The guard against reading a session-start *matcher* as
    /// a tool must not also refuse the case where there is none, and no test
    /// imported a session-start hook at all.
    #[test]
    fn a_session_start_hook_with_no_matcher_is_imported() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = "{\"hooks\":{\"Boot\":[{\"matcher\":\"\",\"hooks\":[\
            {\"type\":\"command\",\"command\":\"echo hi\"}]}]}}";

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert_eq!(
            imported.len(),
            1,
            "an unnarrowed session start: {residue:?}"
        );
        assert_eq!(
            imported.values().next().unwrap().on,
            hook::Event::SessionStart
        );
    }

    /// **A spelling that is a prefix of another must not claim the shorter
    /// reading.** The adapter omh ships happens to have no such pair, so the
    /// longest-match rule is asserted against a vocabulary that does — which is
    /// the only way to tell it apart from taking whichever came first.
    #[test]
    fn the_longest_spelling_wins_so_a_prefix_cannot_strand_the_rest() {
        let (binding, _) = foreign();
        let overlapping = BTreeMap::from([
            (hook::Tool::Edit, "Edit".to_string()),
            (hook::Tool::Read, "Edit|Write".to_string()),
        ]);
        let vocab = hook::Vocabulary::of(&binding, &overlapping).unwrap();
        let doc = "{\"hooks\":{\"ToolFinished\":[{\"matcher\":\"Edit|Write\",\"hooks\":[\
            {\"type\":\"command\",\"command\":\"x\"}]}]}}";

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert_eq!(imported.len(), 1, "got: {residue:?}");
        assert_eq!(
            imported.values().next().unwrap().tools,
            vec![hook::Tool::Read],
            "`Edit|Write` is one spelling, not `Edit` followed by something"
        );
    }

    /// **A spelling matches a whole word, not a prefix.** `ModifyFetch` is one
    /// word naming no tool; read as a prefix it becomes `Modify` then `Fetch`,
    /// which is two tools nobody wrote and a narrowing omh invented.
    #[test]
    fn a_spelling_matches_a_whole_word_not_a_prefix() {
        let (binding, tools) = foreign();
        let vocab = hook::Vocabulary::of(&binding, &tools).unwrap();
        let doc = "{\"hooks\":{\"ToolFinished\":[{\"matcher\":\"ModifyFetch\",\"hooks\":[\
            {\"type\":\"command\",\"command\":\"x\"}]}]}}";

        let (imported, residue) = parse_hooks(doc, &vocab).unwrap();
        assert!(imported.is_empty(), "read as two tools: {imported:?}");
        assert_eq!(residue.len(), 1, "and reported: {residue:?}");
    }

    // ── what an offered hook belongs to ─────────────────────────────────────

    fn hook_dirs(layers: &[&[(&str, &str)]]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let d = tempfile::tempdir().unwrap();
        let mut dirs = Vec::new();
        for (i, files) in layers.iter().enumerate() {
            let dir = d.path().join(format!("layer{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            for (name, body) in *files {
                std::fs::write(dir.join(name), body).unwrap();
            }
            dirs.push(dir);
        }
        (d, dirs)
    }

    /// **Later directories win, matching `merge_hooks`.** A repo hook shadowing
    /// a catalogue name decides what that name belongs to — otherwise the
    /// launcher runs the repo's hook while the offered list describes the
    /// catalogue's, and the two disagree about the same word.
    #[test]
    fn a_repo_hook_shadowing_a_catalogue_name_decides_its_ecosystem() {
        let (_d, dirs) = hook_dirs(&[
            &[(
                "test.json",
                "{\"on\":\"turn-end\",\"stack\":\"rust\",\"run\":\"cargo test\"}",
            )],
            &[(
                "test.json",
                "{\"on\":\"turn-end\",\"stack\":\"node\",\"run\":\"npm run test\"}",
            )],
        ]);
        assert_eq!(
            declared_stacks(&dirs).unwrap().get("test"),
            Some(&Some("node".to_string())),
            "the layer that decides what runs decides what it belongs to"
        );
    }

    /// **A file that will not parse stays offered.** It contributes `None`,
    /// which means *belongs everywhere* — dropping the name because its file
    /// has a typo would hide the hook and the typo together, and `merge_hooks`
    /// is where a bad hook file is an error, at the point it would run.
    #[test]
    fn a_hook_file_that_will_not_parse_is_still_offered() {
        let (_d, dirs) = hook_dirs(&[&[
            ("broken.json", "{ this is not json"),
            ("mistyped.json", "{\"on\":\"whenever\",\"run\":\"x\"}"),
        ]]);
        let got = declared_stacks(&dirs).unwrap();
        assert_eq!(
            got.get("broken"),
            Some(&None),
            "a file omh cannot read belongs everywhere: {got:?}"
        );
        assert_eq!(got.get("mistyped"), Some(&None), "got: {got:?}");
    }

    /// **A `.yours` backup is not a hook.** `install_bundled` keeps somebody's
    /// replaced edit beside the file that replaced it, and reading one as a
    /// hook puts `rust-test.json` into the offered list as a name nothing
    /// answers to. `stack::load_dir` refuses the same file for the same reason.
    #[test]
    fn a_replaced_files_backup_is_not_a_hook_name() {
        let (_d, dirs) = hook_dirs(&[&[
            (
                "rust-test.json",
                "{\"on\":\"turn-end\",\"stack\":\"rust\",\"run\":\"cargo test\"}",
            ),
            (
                "rust-test.json.yours",
                "{\"on\":\"turn-end\",\"run\":\"cargo t\"}",
            ),
        ]]);
        let got = declared_stacks(&dirs).unwrap();
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            vec!["rust-test"],
            "only a file omh reads back is a name: {got:?}"
        );
    }
}
