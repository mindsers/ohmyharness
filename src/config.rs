//! Settings with provenance.
//!
//! Three layers means the useful question is never "what is this set to" but
//! "which layer is it coming from, and what is it shadowing". So the resolver
//! reports origin alongside every value, the way `git config --show-origin`
//! does — otherwise a three-layer merge is undebuggable.

use crate::adapter::Render;
use crate::profile::Paths;
use crate::render::Server;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// `~/.omh/profile` — yours, every project.
    Personal,
    /// `<repo>/.omh/profile` — committed, shared with the team.
    Shared,
    /// `<repo>/.omh/local` — gitignored, yours alone.
    Local,
}

/// A resolved setting: the winning value, where it came from, and what it beat.
#[derive(Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub layer: Layer,
    /// Layers that also declared this key but lost. Always lower precedence.
    pub shadows: Vec<Layer>,
}

/// Where a write landed, and whether that location is under version control.
#[derive(Debug, PartialEq)]
pub struct Written {
    pub path: PathBuf,
    pub layer: Layer,
    pub committed: bool,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Self::Personal, Self::Shared, Self::Local];

    /// `omh set` with no `--layer` writes here: a mistyped secret must not be
    /// committable by accident.
    pub const DEFAULT_WRITE: Layer = Self::Local;

    pub fn dir(&self, paths: &Paths) -> PathBuf {
        match self {
            Self::Personal => paths.root.join("profile"),
            Self::Shared => paths.repo.join(".omh/profile"),
            Self::Local => paths.repo.join(".omh/local"),
        }
    }

    /// Only the shared layer is under version control.
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Shared)
    }
}

impl std::str::FromStr for Layer {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Layer::ALL
            .into_iter()
            .find(|l| l.to_string() == s)
            .with_context(|| format!("unknown layer `{s}` — expected personal, shared, or local"))
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Personal => "personal",
            Self::Shared => "shared",
            Self::Local => "local",
        })
    }
}

/// Every `policy.toml` key across all layers, resolved with provenance.
pub fn policy(paths: &Paths) -> Result<Vec<Setting>> {
    let mut found = Vec::new();
    for layer in Layer::ALL {
        let path = layer.dir(paths).join("policy.toml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let table: toml::Table =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        for (key, value) in table {
            found.push((key, repr(&value), layer));
        }
    }
    Ok(resolve(found))
}

/// Every hook across all layers, resolved with provenance.
///
/// Same shape as [`servers`] deliberately: `omh why` treats an MCP server and a
/// hook as the same kind of thing — something installed, from some layer, that
/// omh either chose or you did. `render::merge_hooks` merges for *rendering*
/// and drops the layer, which is the one fact this needs.
pub fn hooks(paths: &Paths) -> Result<Vec<Setting>> {
    let mut found = Vec::new();
    for layer in Layer::ALL {
        let dir = layer.dir(paths).join("hooks");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let doc: serde_json::Value = serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?;
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            // Not `unwrap_or("")`. An empty command compares unequal to every
            // baseline, so a hook file that is truncated, half-written, or
            // hand-edited with the wrong key was reported to the user as
            // "modified by you" — a false accusation, with a blank value as the
            // only tell, from the one command whose job is telling authorship
            // straight. `render.rs` treats the same file as a hard error, so
            // two subsystems told two stories about it.
            let command = doc
                .get("command")
                .and_then(|c| c.as_str())
                .with_context(|| {
                    format!(
                        "{} has no `command` string — it is not a usable hook",
                        path.display()
                    )
                })?;
            found.push((name, command.to_string(), layer));
        }
    }
    Ok(resolve(found))
}

/// Every MCP server across all layers, resolved with provenance.
pub fn servers(paths: &Paths) -> Result<Vec<Setting>> {
    let mut found = Vec::new();
    for layer in Layer::ALL {
        let path = layer.dir(paths).join("mcp.json");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let doc: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        let Some(servers) = doc.get("mcpServers").and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, spec) in servers {
            let command = spec.get("command").and_then(|c| c.as_str()).unwrap_or("");
            found.push((name.clone(), command.to_string(), layer));
        }
    }
    Ok(resolve(found))
}

pub fn set(paths: &Paths, key: &str, raw: &str, layer: Layer) -> Result<Written> {
    let path = layer.dir(paths).join("policy.toml");
    let mut table = read_table(&path)?;
    table.insert(key.to_string(), parse_value(raw));
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, toml::to_string_pretty(&table)?)?;
    Ok(Written {
        path,
        layer,
        committed: layer.is_committed(),
    })
}

pub fn unset(paths: &Paths, key: &str, layer: Layer) -> Result<bool> {
    let path = layer.dir(paths).join("policy.toml");
    let mut table = read_table(&path)?;
    if table.remove(key).is_none() {
        return Ok(false);
    }
    std::fs::write(&path, toml::to_string_pretty(&table)?)?;
    Ok(true)
}

/// Last layer wins; every earlier declaration of the same key is recorded as
/// shadowed. Without that trail a three-layer merge cannot be debugged.
fn resolve(found: Vec<(String, String, Layer)>) -> Vec<Setting> {
    let mut by_key: BTreeMap<String, Vec<(String, Layer)>> = BTreeMap::new();
    for (key, value, layer) in found {
        by_key.entry(key).or_default().push((value, layer));
    }
    by_key
        .into_iter()
        .map(|(key, mut hits)| {
            let (value, layer) = hits.pop().expect("entry exists because it was inserted");
            Setting {
                key,
                value,
                layer,
                shadows: hits.into_iter().map(|(_, l)| l).collect(),
            }
        })
        .collect()
}

fn read_table(path: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).with_context(|| format!("parsing {}", path.display())),
        Err(_) => Ok(toml::Table::new()),
    }
}

/// Accept TOML literals (`["a", "b"]`, `true`, `30`) and fall back to a bare
/// string, so `omh set idle_timeout 30m` does not need quoting.
fn parse_value(raw: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .and_then(|t| t.get("v").cloned())
        .unwrap_or_else(|| toml::Value::String(raw.to_string()))
}

fn repr(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ── MCP servers ─────────────────────────────────────────────────────────────

/// What an import did, or would do. Reported rather than assumed, because
/// import pulls config you did not type into a file you will commit.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub added: Vec<String>,
    /// Present already with different settings. Skipped unless forced.
    pub conflicts: Vec<String>,
    /// Present already, byte-identical. Import is idempotent.
    pub unchanged: Vec<String>,
}

pub fn mcp_add(paths: &Paths, layer: Layer, name: &str, server: Server) -> Result<Written> {
    let path = mcp_path(paths, layer);
    let mut all = read_servers(&path)?;
    all.insert(name.to_string(), server);
    write_servers(&path, &all)?;
    Ok(Written {
        path,
        layer,
        committed: layer.is_committed(),
    })
}

pub fn mcp_remove(paths: &Paths, layer: Layer, name: &str) -> Result<bool> {
    let path = mcp_path(paths, layer);
    let mut all = read_servers(&path)?;
    if all.remove(name).is_none() {
        return Ok(false);
    }
    write_servers(&path, &all)?;
    Ok(true)
}

/// `dry_run` reports the same plan without writing.
pub fn mcp_import(
    paths: &Paths,
    layer: Layer,
    incoming: BTreeMap<String, Server>,
    force: bool,
    dry_run: bool,
) -> Result<Imported> {
    let path = mcp_path(paths, layer);
    let mut all = read_servers(&path)?;
    let mut report = Imported::default();

    for (name, server) in incoming {
        match all.get(&name) {
            // Identical: import stays idempotent and re-runnable.
            Some(existing) if *existing == server => report.unchanged.push(name),
            // Different: never clobber config the user wrote by hand.
            Some(_) if !force => report.conflicts.push(name),
            _ => {
                all.insert(name.clone(), server);
                report.added.push(name);
            }
        }
    }

    if !dry_run && !report.added.is_empty() {
        write_servers(&path, &all)?;
    }
    Ok(report)
}

fn mcp_path(paths: &Paths, layer: Layer) -> PathBuf {
    layer.dir(paths).join("mcp.json")
}

/// A layer's canonical file is itself `mcp-json`, so reading it is the same
/// parser import uses — one code path, one set of round-trip tests.
fn read_servers(path: &Path) -> Result<BTreeMap<String, Server>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => crate::render::parse(Render::McpJson, &raw)
            .with_context(|| format!("reading {}", path.display())),
        Err(_) => Ok(BTreeMap::new()),
    }
}

fn write_servers(path: &Path, servers: &BTreeMap<String, Server>) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let doc = serde_json::json!({ "mcpServers": servers });
    std::fs::write(path, serde_json::to_string_pretty(&doc)? + "\n")?;
    Ok(())
}

/// A policy value that is a list. `policy()` renders values for display, so an
/// array arrives as its TOML text and has to be parsed back.
pub fn policy_list(paths: &Paths, key: &str) -> Vec<String> {
    let Some(repr) = policy(paths)
        .ok()
        .and_then(|s| s.into_iter().find(|s| s.key == key))
    else {
        return Vec::new();
    };
    toml::from_str::<toml::Table>(&format!("v = {}", repr.value))
        .ok()
        .and_then(|t| t.get("v").and_then(|v| v.as_array()).cloned())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        (dir, paths)
    }

    fn seed(paths: &Paths, layer: Layer, name: &str, body: &str) {
        let p = layer.dir(paths).join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    // ── hooks ───────────────────────────────────────────────────────────────
    //
    // `hooks()` shipped with no tests at all while being the sole input to
    // `omh why`'s hook answers. Its body could be replaced with
    // `Ok(Vec::new())` and the whole suite stayed green — at which point every
    // installed hook reports as "not installed here".

    #[test]
    fn hooks_resolve_with_the_layer_they_came_from() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "hooks/graph-read.json",
            r#"{"command":"a"}"#,
        );
        seed(
            &paths,
            Layer::Local,
            "hooks/mine.json",
            r#"{"command":"b"}"#,
        );

        let found = hooks(&paths).unwrap();
        let by = |k: &str| {
            found
                .iter()
                .find(|s| s.key == k)
                .unwrap_or_else(|| panic!("{k}"))
        };
        assert_eq!(by("graph-read").value, "a");
        assert_eq!(by("graph-read").layer, Layer::Shared);
        assert_eq!(by("mine").layer, Layer::Local);
    }

    #[test]
    fn a_later_layer_wins_and_names_what_it_shadowed() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "hooks/x.json",
            r#"{"command":"shared"}"#,
        );
        seed(
            &paths,
            Layer::Local,
            "hooks/x.json",
            r#"{"command":"local"}"#,
        );

        let found = hooks(&paths).unwrap();
        let x = found.iter().find(|s| s.key == "x").unwrap();
        assert_eq!(x.value, "local");
        assert_eq!(x.shadows, vec![Layer::Shared]);
    }

    /// A hook file with no `command` used to become an empty string, which
    /// compares unequal to every baseline — so a truncated or hand-edited file
    /// was reported to the user as their own edit, with a blank value as the
    /// only tell. `render.rs` treats the same file as a hard error, so the two
    /// subsystems told two different stories about it.
    #[test]
    fn a_hook_without_a_command_is_an_error_not_an_empty_string() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "hooks/broken.json",
            r#"{"event":"Stop"}"#,
        );

        let err = hooks(&paths).unwrap_err().to_string();
        assert!(err.contains("broken.json"), "must name the file: {err}");
        assert!(err.contains("command"), "must say what is missing: {err}");
    }

    #[test]
    fn a_non_json_file_in_the_hooks_directory_is_ignored() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Shared, "hooks/notes.md", "not a hook");
        seed(
            &paths,
            Layer::Shared,
            "hooks/real.json",
            r#"{"command":"c"}"#,
        );

        let found = hooks(&paths).unwrap();
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].key, "real");
    }

    fn get<'a>(settings: &'a [Setting], key: &str) -> &'a Setting {
        settings
            .iter()
            .find(|s| s.key == key)
            .unwrap_or_else(|| panic!("no key {key}"))
    }

    // ── layers ──────────────────────────────────────────────────────────────

    #[test]
    fn only_the_shared_layer_is_committed() {
        assert!(Layer::Shared.is_committed());
        assert!(!Layer::Personal.is_committed());
        assert!(!Layer::Local.is_committed(), "local must be gitignored");
    }

    #[test]
    fn layers_map_to_their_directories() {
        let (_d, paths) = fixture();
        assert_eq!(Layer::Personal.dir(&paths), paths.root.join("profile"));
        assert_eq!(Layer::Shared.dir(&paths), paths.repo.join(".omh/profile"));
        assert_eq!(Layer::Local.dir(&paths), paths.repo.join(".omh/local"));
    }

    #[test]
    fn layers_round_trip_through_their_names() {
        for layer in Layer::ALL {
            assert_eq!(Layer::from_str(&layer.to_string()).unwrap(), layer);
        }
        assert!(Layer::from_str("nonsense").is_err());
    }

    // ── provenance ──────────────────────────────────────────────────────────

    #[test]
    fn policy_reports_the_winning_layer() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "policy.toml",
            "idle_timeout = \"30m\"",
        );
        let settings = policy(&paths).unwrap();
        assert_eq!(get(&settings, "idle_timeout").layer, Layer::Personal);
    }

    #[test]
    fn later_layers_win_and_the_loser_is_named() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "policy.toml",
            "idle_timeout = \"30m\"",
        );
        seed(
            &paths,
            Layer::Shared,
            "policy.toml",
            "idle_timeout = \"5m\"",
        );
        seed(&paths, Layer::Local, "policy.toml", "idle_timeout = \"2h\"");

        let s = policy(&paths).unwrap();
        let t = get(&s, "idle_timeout");
        assert_eq!(t.value, "2h", "local wins");
        assert_eq!(t.layer, Layer::Local);
        assert_eq!(
            t.shadows,
            vec![Layer::Personal, Layer::Shared],
            "a value that beat others must say so, or the merge is undebuggable"
        );
    }

    #[test]
    fn unshadowed_settings_report_no_losers() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "policy.toml",
            "carry_in = [\".env\"]",
        );
        assert!(get(&policy(&paths).unwrap(), "carry_in").shadows.is_empty());
    }

    #[test]
    fn missing_layers_are_not_an_error() {
        let (_d, paths) = fixture();
        assert!(policy(&paths).unwrap().is_empty());
        assert!(servers(&paths).unwrap().is_empty());
    }

    #[test]
    fn mcp_servers_resolve_with_provenance() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "mcp.json",
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"}}}"#,
        );
        seed(
            &paths,
            Layer::Local,
            "mcp.json",
            r#"{"mcpServers":{"omh-memory":{"command":"omh-mcp"}}}"#,
        );

        let s = servers(&paths).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(get(&s, "codegraph").layer, Layer::Shared);
        assert_eq!(get(&s, "omh-memory").layer, Layer::Local);
        assert_eq!(get(&s, "codegraph").value, "codebase-memory-mcp");
    }

    #[test]
    fn a_local_server_can_shadow_a_shared_one() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "mcp.json",
            r#"{"mcpServers":{"codegraph":{"command":"old"}}}"#,
        );
        seed(
            &paths,
            Layer::Local,
            "mcp.json",
            r#"{"mcpServers":{"codegraph":{"command":"new"}}}"#,
        );

        let s = servers(&paths).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(get(&s, "codegraph").value, "new");
        assert_eq!(get(&s, "codegraph").shadows, vec![Layer::Shared]);
    }

    // ── writes ──────────────────────────────────────────────────────────────

    /// The safety property: an unqualified write can never reach version control.
    #[test]
    fn default_write_target_is_gitignored() {
        assert_eq!(Layer::DEFAULT_WRITE, Layer::Local);
        assert!(!Layer::DEFAULT_WRITE.is_committed());
    }

    #[test]
    fn set_creates_the_layer_file_when_absent() {
        let (_d, paths) = fixture();
        let w = set(&paths, "idle_timeout", "30m", Layer::DEFAULT_WRITE).unwrap();
        assert_eq!(w.path, Layer::Local.dir(&paths).join("policy.toml"));
        assert!(!w.committed);
        assert_eq!(get(&policy(&paths).unwrap(), "idle_timeout").value, "30m");
    }

    /// Writing to the committed layer must be flagged, because that is the one
    /// mistake git makes unrecoverable.
    #[test]
    fn writing_to_the_shared_layer_is_flagged_as_committed() {
        let (_d, paths) = fixture();
        let w = set(&paths, "carry_in", "[\".env\"]", Layer::Shared).unwrap();
        assert!(w.committed);
        assert_eq!(w.layer, Layer::Shared);
    }

    #[test]
    fn set_preserves_unrelated_keys() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Local,
            "policy.toml",
            "carry_in = [\".env\"]\nidle_timeout = \"5m\"",
        );
        set(&paths, "idle_timeout", "1h", Layer::Local).unwrap();

        let s = policy(&paths).unwrap();
        assert_eq!(get(&s, "idle_timeout").value, "1h");
        assert_eq!(
            get(&s, "carry_in").value,
            "[\".env\"]",
            "must not clobber siblings"
        );
    }

    #[test]
    fn set_accepts_arrays_as_well_as_scalars() {
        let (_d, paths) = fixture();
        set(&paths, "carry_in", "[\".env\", \"certs/\"]", Layer::Local).unwrap();
        assert_eq!(
            get(&policy(&paths).unwrap(), "carry_in").value,
            "[\".env\", \"certs/\"]"
        );
    }

    #[test]
    fn unset_touches_only_the_named_layer() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "policy.toml",
            "idle_timeout = \"30m\"",
        );
        seed(&paths, Layer::Local, "policy.toml", "idle_timeout = \"2h\"");

        assert!(unset(&paths, "idle_timeout", Layer::Local).unwrap());

        let resolved = policy(&paths).unwrap();
        let t = get(&resolved, "idle_timeout");
        assert_eq!(t.value, "30m", "the personal layer must resurface");
        assert_eq!(t.layer, Layer::Personal);
    }

    #[test]
    fn unset_reports_when_nothing_was_removed() {
        let (_d, paths) = fixture();
        assert!(!unset(&paths, "absent", Layer::Local).unwrap());
    }

    // ── mcp add / rm ────────────────────────────────────────────────────────

    fn server(command: &str) -> Server {
        Server {
            command: command.into(),
            args: vec![],
            env: BTreeMap::new(),
        }
    }

    fn names(paths: &Paths) -> Vec<String> {
        servers(paths).unwrap().into_iter().map(|s| s.key).collect()
    }

    #[test]
    fn mcp_add_creates_the_file_when_absent() {
        let (_d, paths) = fixture();
        let w = mcp_add(&paths, Layer::DEFAULT_WRITE, "g", server("c")).unwrap();
        assert_eq!(w.path, Layer::Local.dir(&paths).join("mcp.json"));
        assert_eq!(names(&paths), ["g"]);
    }

    /// MCP entries carry env, and env carries tokens. An unqualified add must
    /// land somewhere git will never see.
    #[test]
    fn mcp_add_defaults_to_the_gitignored_layer() {
        let (_d, paths) = fixture();
        let w = mcp_add(&paths, Layer::DEFAULT_WRITE, "g", server("c")).unwrap();
        assert!(!w.committed);
        assert_eq!(w.layer, Layer::Local);
    }

    #[test]
    fn mcp_add_to_the_shared_layer_is_flagged_as_committed() {
        let (_d, paths) = fixture();
        assert!(
            mcp_add(&paths, Layer::Shared, "g", server("c"))
                .unwrap()
                .committed
        );
    }

    #[test]
    fn mcp_add_preserves_existing_servers() {
        let (_d, paths) = fixture();
        mcp_add(&paths, Layer::Local, "first", server("a")).unwrap();
        mcp_add(&paths, Layer::Local, "second", server("b")).unwrap();
        assert_eq!(names(&paths), ["first", "second"]);
    }

    #[test]
    fn mcp_rm_touches_only_the_named_layer() {
        let (_d, paths) = fixture();
        mcp_add(&paths, Layer::Shared, "g", server("shared-cmd")).unwrap();
        mcp_add(&paths, Layer::Local, "g", server("local-cmd")).unwrap();

        assert!(mcp_remove(&paths, Layer::Local, "g").unwrap());

        let resolved = servers(&paths).unwrap();
        let g = get(&resolved, "g");
        assert_eq!(g.value, "shared-cmd", "the shared layer must resurface");
        assert_eq!(g.layer, Layer::Shared);
    }

    #[test]
    fn mcp_rm_reports_when_nothing_was_removed() {
        let (_d, paths) = fixture();
        assert!(!mcp_remove(&paths, Layer::Local, "absent").unwrap());
    }

    // ── import ──────────────────────────────────────────────────────────────

    fn incoming() -> BTreeMap<String, Server> {
        BTreeMap::from([
            ("a".to_string(), server("a-cmd")),
            ("b".to_string(), server("b-cmd")),
        ])
    }

    #[test]
    fn import_reports_what_it_would_add() {
        let (_d, paths) = fixture();
        let r = mcp_import(&paths, Layer::Local, incoming(), false, false).unwrap();
        assert_eq!(r.added, ["a", "b"]);
        assert!(r.conflicts.is_empty() && r.unchanged.is_empty());
        assert_eq!(names(&paths), ["a", "b"]);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (_d, paths) = fixture();
        let r = mcp_import(&paths, Layer::Local, incoming(), false, true).unwrap();
        assert_eq!(r.added, ["a", "b"], "the plan is still reported");
        assert!(names(&paths).is_empty(), "but nothing was written");
    }

    /// Re-importing the same config must be a no-op, not a duplicate or a churn
    /// of the file — otherwise `omh mcp import` is unsafe to re-run.
    #[test]
    fn importing_twice_changes_nothing() {
        let (_d, paths) = fixture();
        mcp_import(&paths, Layer::Local, incoming(), false, false).unwrap();
        let second = mcp_import(&paths, Layer::Local, incoming(), false, false).unwrap();
        assert_eq!(second.unchanged, ["a", "b"]);
        assert!(second.added.is_empty());
    }

    #[test]
    fn a_changed_server_is_a_conflict_not_a_silent_overwrite() {
        let (_d, paths) = fixture();
        mcp_add(&paths, Layer::Local, "a", server("mine")).unwrap();

        let r = mcp_import(&paths, Layer::Local, incoming(), false, false).unwrap();
        assert_eq!(r.conflicts, ["a"]);
        assert_eq!(r.added, ["b"], "unconflicted servers still import");
        assert_eq!(
            get(&servers(&paths).unwrap(), "a").value,
            "mine",
            "kept, not clobbered"
        );
    }

    #[test]
    fn force_resolves_a_conflict_by_overwriting() {
        let (_d, paths) = fixture();
        mcp_add(&paths, Layer::Local, "a", server("mine")).unwrap();

        let r = mcp_import(&paths, Layer::Local, incoming(), true, false).unwrap();
        assert_eq!(r.added, ["a", "b"]);
        assert!(r.conflicts.is_empty());
        assert_eq!(get(&servers(&paths).unwrap(), "a").value, "a-cmd");
    }

    #[test]
    fn importing_nothing_is_not_an_error() {
        let (_d, paths) = fixture();
        assert_eq!(
            mcp_import(&paths, Layer::Local, BTreeMap::new(), false, false).unwrap(),
            Imported::default()
        );
    }

    #[test]
    fn a_list_setting_comes_back_as_a_list() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "policy.toml",
            "carry_in = [\".env\", \"certs/\"]",
        );
        assert_eq!(policy_list(&paths, "carry_in"), vec![".env", "certs/"]);
    }

    #[test]
    fn an_absent_list_is_empty_not_an_error() {
        let (_d, paths) = fixture();
        assert!(policy_list(&paths, "carry_in").is_empty());
    }

    /// Layers merge as usual, so a project can narrow or widen what a personal
    /// default carries.
    #[test]
    fn a_later_layer_replaces_the_list() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "policy.toml",
            "carry_in = [\".env\"]",
        );
        seed(
            &paths,
            Layer::Local,
            "policy.toml",
            "carry_in = [\".env.local\"]",
        );
        assert_eq!(policy_list(&paths, "carry_in"), vec![".env.local"]);
    }
}
