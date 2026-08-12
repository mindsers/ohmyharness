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
/// `own` is what omh itself contributes and what this repo has switched off.
/// It is not a layer: omh's hooks belong to no directory, and a server whose
/// feature is disabled here is still in your `mcp.json` — the file is yours and
/// is left exactly as you have it.
pub fn document(
    cap: Capability,
    binding: &Binding,
    sources: &[PathBuf],
    own: &crate::base::Own,
) -> Result<Document> {
    match binding.render {
        Render::McpJson | Render::CodexToml | Render::OpencodeJson => {
            let mut servers = merge_servers(sources)?;
            servers.retain(|name, _| !own.disabled_servers.contains(name));
            // Variable by variable, not entry by entry: a repo overriding one
            // token must not silently inherit the rest of a catalogue entry it
            // never saw. Named where it is applied rather than merged into the
            // sources, so `omh config` still shows the catalogue as written.
            for (name, env) in &own.mcp_env {
                let server = servers.get_mut(name).with_context(|| {
                    format!(
                        "[mcp.{name}.env] overrides a server that is not in your \
                         catalogue — nothing would read it. `omh config mcp ls` \
                         lists what is there."
                    )
                })?;
                server.env.extend(env.clone());
            }
            Ok(mcp(binding.render, &servers)?.into())
        }
        Render::ClaudeSettings => {
            let (rendered, dropped) = translate(&merge_hooks(sources, own)?, binding)?;
            Ok(Document {
                body: claude_settings(&rendered)?,
                dropped,
            })
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
) -> Result<(BTreeMap<String, hook::Rendered>, Vec<hook::Dropped>)> {
    let mut rendered = BTreeMap::new();
    let mut dropped = Vec::new();
    for (name, h) in hooks {
        match hook::render(name, h, binding)? {
            Outcome::Rendered(r) => {
                rendered.insert(name.clone(), r);
            }
            Outcome::Dropped(d) => dropped.push(d),
        }
    }
    Ok((rendered, dropped))
}

/// Union by filename across layers; later layers shadow earlier ones.
///
/// A file answering to a manifest name is **never read** — see `Own::reserved`.
/// Read-and-then-override is not enough: with the feature off there is nothing
/// to override it with, and a repo initialised before generation still has the
/// five seeded files, so switching a feature off would leave it running.
///
/// omh's own are inserted after the layers, but that ordering is not what makes
/// them win. They are generated from the manifest and belong to no layer, which
/// is the point: a hook you can edit is a hook omh can never ship a fix to, and
/// `git-unavailable` has already needed one. A planned migration deletes the
/// leftovers (`docs/design/profile.md`, P3); until then they are inert, and
/// `omh why` says so rather than reporting one as yours.
fn merge_hooks(dirs: &[PathBuf], own: &crate::base::Own) -> Result<BTreeMap<String, hook::Hook>> {
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
        for entry in entries.flatten() {
            let path = entry.path();
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
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.insert(name, hook::Hook::parse(&raw, &path.display().to_string())?);
            }
        }
    }
    for own_hook in &own.hooks {
        out.insert(own_hook.name.to_string(), own_hook.hook.clone());
    }
    Ok(out)
}

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
    /// Today a hook file has to say `"event": "Stop"` — Claude Code's
    /// vocabulary, in a file omh presents as its own — and `matcher` and the
    /// `hookSpecificOutput` payload are the same leak one level down. Nothing
    /// has had to translate one only because opencode declares no hooks
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
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "cargo test");
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

        let own = crate::base::Own {
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
            &own,
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
        let own = crate::base::Own {
            mcp_env: BTreeMap::from([("linear".to_string(), BTreeMap::new())]),
            ..Default::default()
        };
        let adapter = claude_hooks();
        let err = document(
            Capability::Mcp,
            adapter.supports(Capability::Mcp).unwrap(),
            &[mcp],
            &own,
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
        let err = merge_hooks(&[dir.path().join("h")], &own)
            .expect_err("a manifest name is not something a file may claim");
        let msg = format!("{err:#}");
        assert!(msg.contains("graph-refresh.json"), "name the file: {msg}");
        assert!(
            msg.contains("codegraph") || msg.contains("omh"),
            "and whose name it is: {msg}"
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

        let err = merge_hooks(std::slice::from_ref(&hooks), &Default::default())
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
        let err = document(Capability::Skills, skills, &[], &Default::default()).unwrap_err();
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
