//! Settings with provenance.
//!
//! Three layers means the useful question is never "what is this set to" but
//! "which layer is it coming from, and what is it shadowing". So the resolver
//! reports origin alongside every value, the way `git config --show-origin`
//! does — otherwise a three-layer merge is undebuggable.
//!
//! Content stopped having layers when the catalogue arrived; settings keep
//! theirs, because a setting genuinely has one value and the question is which
//! file decided it. What changed is that a layer is now a **file** rather than a
//! directory, and `policy.toml` — a fourth name for the same idea, living inside
//! a directory whose purpose was content — folded into it.

use crate::adapter::Render;
use crate::profile::Paths;
use crate::render::Server;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// `~/.omh/settings.toml` — yours, every project.
    Personal,
    /// `<repo>/.omh/settings.toml` — committed, shared with the team.
    Shared,
    /// `<repo>/.omh/settings.local.toml` — gitignored, yours alone.
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

    /// The file this layer's settings live in.
    pub fn file(&self, paths: &Paths) -> PathBuf {
        match self {
            Self::Personal => paths.root.join("settings.toml"),
            Self::Shared => paths.repo.join(".omh").join("settings.toml"),
            Self::Local => paths.repo.join(".omh").join(crate::settings::LOCAL),
        }
    }

    /// Where this layer's *content* lives, for the one capability that has more
    /// than one tier.
    ///
    /// `None` for `Local`: a repo's gitignored tier is settings only. Hooks are
    /// executable and arrive by `git clone`, which is exactly why they are
    /// committed and disclosed rather than hidden — a gitignored hook would be
    /// executable content with no reviewer at all.
    pub fn content_dir(&self, paths: &Paths) -> Option<PathBuf> {
        match self {
            Self::Personal => Some(paths.root.clone()),
            Self::Shared => Some(paths.repo.join(".omh")),
            Self::Local => None,
        }
    }

    /// Whose this is, for reporting *content* rather than a setting.
    ///
    /// The layer names are exactly right for a setting — they are what `--layer`
    /// takes, and personal/shared/local says which of three files decided a
    /// value. They are wrong for content, where there is one catalogue and one
    /// repo tier: `omh why rust-test` reporting a hook this project committed
    /// as "installed shared" names a layer that no longer describes anything.
    pub fn whose(&self) -> &'static str {
        match self {
            Self::Personal => "your catalogue",
            Self::Shared => "this repo",
            Self::Local => "local",
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

/// Every settings key across all layers, resolved with provenance.
pub fn policy(paths: &Paths) -> Result<Vec<Setting>> {
    let mut found = Vec::new();
    for layer in Layer::ALL {
        let path = layer.file(paths);
        let Some(raw) = read_layer(&path)? else {
            continue;
        };
        let table: toml::Table =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        for (key, value) in table {
            // A table is configuration *for* something, not a setting with a
            // value. `[omh]` shares this file, and `[mcp]` will — stringifying
            // either would report a feature switch as a setting whose value is
            // an inline table, and `omh config` would print it as one.
            if value.is_table() {
                continue;
            }
            found.push((key, repr(&value), layer));
        }
    }
    Ok(resolve(found))
}

/// Read a layer's file, distinguishing "this layer declares nothing" from
/// "this layer could not be read".
///
/// `let Ok(..) else { continue }` conflated them, and the second case is
/// unrecoverable from the outside: `chmod 000` on `mcp.json` made `omh why`
/// answer "not installed here" about an installed server and advise `omh init`
/// — which does nothing, because `write_if_absent` sees the file exists. A
/// closed loop, exit 0, no error anywhere.
fn read_layer(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        // Absent is a normal, expected state: most layers declare most things
        // not at all.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
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
        let Some(dir) = layer.content_dir(paths).map(|d| d.join("hooks")) else {
            continue;
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            // Not `.flatten()`, for the reason `render::merge_hooks` gives: a
            // `readdir` failing part-way through would drop a hook from this
            // listing while the launcher still ships it, and the two telling
            // different stories about one directory is what this module's
            // other guards exist to stop.
            let path = entry
                .with_context(|| format!("reading {}", dir.display()))?
                .path();
            if !path.extension().is_some_and(|e| e == "json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            // Parsed, never probed for a key. A hook file that is truncated,
            // half-written, or hand-edited with the wrong key used to yield a
            // blank command here, which compares unequal to every baseline and
            // so was reported to the user as "modified by you" — a false
            // accusation, from the one command whose job is telling authorship
            // straight. `render.rs` treats the same file as a hard error, so
            // two subsystems told two stories about it; going through the same
            // parser is what keeps them agreeing.
            let hook = crate::hook::Hook::parse(&raw, &path.display().to_string())?;
            found.push((name, hook.does().to_string(), layer));
        }
    }
    Ok(resolve(found))
}

/// Every MCP server, resolved with provenance.
///
/// The catalogue and nowhere else. A server is a thing you hold: a repo names
/// entries from your catalogue and overrides their environment, but cannot
/// define one — so there is one path to read, and a file anywhere else is a
/// mistake to report rather than a tier to merge.
pub fn servers(paths: &Paths) -> Result<Vec<Setting>> {
    refuse_a_repo_server(paths)?;

    let mut found = Vec::new();
    let path = mcp_path(paths);
    if let Some(raw) = read_layer(&path)? {
        let doc: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        if let Some(servers) = doc.get("mcpServers").and_then(|v| v.as_object()) {
            for (name, spec) in servers {
                // Through the same parser the renderer uses, for the reason
                // `hooks` gives: an entry omh cannot understand used to become
                // a server with a blank command here, which compares unequal to
                // every baseline — so `omh why` accused you of modifying it
                // while a launch failed hard on the same file. Two subsystems,
                // two stories, from the one command whose job is telling
                // authorship straight.
                let server: Server = serde_json::from_value(spec.clone()).with_context(|| {
                    format!("{}: server `{name}` is not one omh can run", path.display())
                })?;
                found.push((name.clone(), server.command, Layer::Personal));
            }
        }
    }
    Ok(resolve(found))
}

/// `<repo>/.omh/` holds `settings.toml`, `memory.toml` and `hooks/`, and it
/// used to hold `mcp.json` too — so writing one there is the natural mistake
/// rather than the exotic one, and it is silent in the worst way: reported as
/// installed by `omh config mcp ls`, never mounted, and counted as `installed`
/// when deciding whether a feature's server is still there.
fn refuse_a_repo_server(paths: &Paths) -> Result<()> {
    let stray = paths.repo.join(".omh").join("mcp.json");
    if stray.exists() {
        anyhow::bail!(
            "{}: a repo names servers from your catalogue, it cannot declare one — \
             nothing reads this file. Add it with `omh config mcp add`, and put a \
             token for this repo alone under `[mcp.<name>.env]` in .omh/{}.",
            stray.display(),
            crate::settings::LOCAL
        );
    }
    Ok(())
}

pub fn set(paths: &Paths, key: &str, raw: &str, layer: Layer) -> Result<Written> {
    let path = layer.file(paths);
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
    let path = layer.file(paths);
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

pub fn mcp_add(paths: &Paths, name: &str, server: Server) -> Result<Written> {
    let path = mcp_path(paths);
    let mut all = read_servers(&path)?;
    all.insert(name.to_string(), server);
    write_servers(&path, &all)?;
    Ok(Written {
        path,
        layer: Layer::Personal,
        committed: false,
    })
}

pub fn mcp_remove(paths: &Paths, name: &str) -> Result<bool> {
    let path = mcp_path(paths);
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
    incoming: BTreeMap<String, Server>,
    force: bool,
    dry_run: bool,
) -> Result<Imported> {
    let path = mcp_path(paths);
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

/// The catalogue's, and there is only one.
///
/// A server used to be declarable in any of three layers, which is what made
/// `--layer` meaningful on these commands. With one catalogue there is one
/// destination, and a per-repo override is a *setting* — `[mcp.<name>.env]` in
/// `settings.local.toml` — rather than a second declaration of the server.
pub fn mcp_path(paths: &Paths) -> PathBuf {
    paths.root.join("mcp.json")
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

    /// Content, which now has one tier and a half: the catalogue, and the
    /// repo's hooks. `Local` has none, so a test that seeds into it is asking
    /// for something that no longer exists.
    fn seed(paths: &Paths, layer: Layer, name: &str, body: &str) {
        let p = layer
            .content_dir(paths)
            .unwrap_or_else(|| panic!("{layer} holds no content"))
            .join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// A settings key, in the file that layer reads.
    fn settings(paths: &Paths, layer: Layer, body: &str) {
        let p = layer.file(paths);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// A setting and a feature switch live in one file, because they are one
    /// kind of thing: something this repo decided.
    ///
    /// `policy.toml` was a fourth file with a fourth name for the same idea,
    /// and it sat inside a profile layer whose whole purpose was content — so
    /// removing the content directories would have left one file behind for no
    /// reason but where it happened to be written.
    #[test]
    fn a_setting_and_a_feature_switch_share_one_file() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n\n[omh]\ncodegraph = false\n",
        )
        .unwrap();

        let found = policy(&paths).unwrap();
        assert_eq!(get(&found, "carry_in").value, "[\".env\"]");
        assert_eq!(get(&found, "carry_in").layer, Layer::Shared);
    }

    /// A table is configuration *for* something, not a setting with a value.
    ///
    /// `policy` stringifies whatever it finds at the top level, so with `[omh]`
    /// now sharing the file it would report a feature switch as a setting whose
    /// value is an inline table — and `omh config` would print it as one.
    #[test]
    fn a_table_is_not_reported_as_a_setting() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n",
        )
        .unwrap();
        assert!(
            policy(&paths).unwrap().is_empty(),
            "a table is not a setting with a value"
        );
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
            Layer::Personal,
            "hooks/graph-read.json",
            r#"{"on":"turn-end","run":"a"}"#,
        );
        seed(
            &paths,
            Layer::Shared,
            "hooks/mine.json",
            r#"{"on":"turn-end","run":"b"}"#,
        );

        let found = hooks(&paths).unwrap();
        let by = |k: &str| {
            found
                .iter()
                .find(|s| s.key == k)
                .unwrap_or_else(|| panic!("{k}"))
        };
        assert_eq!(by("graph-read").value, "a");
        assert_eq!(by("graph-read").layer, Layer::Personal, "your catalogue");
        assert_eq!(by("mine").layer, Layer::Shared, "this repo's");
    }

    #[test]
    fn a_project_hook_shadows_a_catalogue_hook_of_the_same_name() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "hooks/format.json",
            r#"{"on":"turn-end","run":"yours"}"#,
        );
        seed(
            &paths,
            Layer::Shared,
            "hooks/format.json",
            r#"{"on":"turn-end","run":"this repo's"}"#,
        );

        let found = hooks(&paths).unwrap();
        let x = found.iter().find(|s| s.key == "format").unwrap();
        assert_eq!(x.value, "this repo's", "project beats catalogue");
        assert_eq!(x.shadows, vec![Layer::Personal]);
    }

    /// A hook file that says when but never what used to become an empty
    /// command string here, which compares unequal to every baseline — so a
    /// truncated or hand-edited file was reported to the user as their own
    /// edit, with a blank value as the only tell. `render.rs` treats the same
    /// file as a hard error, so the two subsystems told two different stories
    /// about it; both go through `hook::Hook::parse` now, which is what keeps
    /// them agreeing.
    #[test]
    fn a_hook_that_does_nothing_is_an_error_not_an_empty_string() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "hooks/broken.json",
            r#"{"on":"turn-end"}"#,
        );

        let err = format!("{:#}", hooks(&paths).unwrap_err());
        assert!(err.contains("broken.json"), "must name the file: {err}");
        assert!(
            err.contains("run") && err.contains("inject"),
            "must say what is missing: {err}"
        );
    }

    /// An entry omh cannot run is an error, not a server with a blank command.
    ///
    /// This is `hooks`' lesson applied to its stated twin. A `"command"` that
    /// is an array — the natural confusion with the opencode form — used to
    /// yield `""` here, which compares unequal to every baseline, so `omh why`
    /// told you *you* had modified a server you had not touched. Meanwhile a
    /// launch failed hard on the same file, because the renderer requires a
    /// string. Two subsystems, two stories, from the command whose whole job
    /// is telling authorship straight.
    #[test]
    fn a_server_omh_cannot_run_is_an_error_not_a_blank_command() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "mcp.json",
            r#"{"mcpServers":{"linear":{"command":["npx","-y","mcp-remote"]}}}"#,
        );

        let err = format!("{:#}", servers(&paths).unwrap_err());
        assert!(err.contains("linear"), "must name the server: {err}");
        assert!(err.contains("mcp.json"), "and the file: {err}");
    }

    /// A repo cannot declare an MCP server, and writing one is an error rather
    /// than a file nobody reads.
    ///
    /// `<repo>/.omh/` holds `settings.toml`, `memory.toml` and `hooks/`, so
    /// dropping an `mcp.json` beside them is the natural mistake — and it used
    /// to be the *documented* place for one. Reported as installed by
    /// `omh config mcp ls` and never mounted, it is a server you would swear
    /// you configured; worse, it fed `installed`, so naming `codegraph` there
    /// kept the graph hooks generated against a server no session receives —
    /// the exact state `base::own`'s `gone` set exists to prevent.
    #[test]
    fn a_repo_declaring_an_mcp_server_is_an_error_naming_the_catalogue() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Shared,
            "mcp.json",
            r#"{"mcpServers":{"linear":{"command":"npx"}}}"#,
        );

        let err = format!("{:#}", servers(&paths).unwrap_err());
        assert!(err.contains("mcp.json"), "must name the file: {err}");
        assert!(
            err.contains("omh config mcp add"),
            "and where a server goes instead: {err}"
        );
    }

    /// A layer that cannot be read is not a layer that declares nothing.
    ///
    /// Conflating them produced a closed loop: `chmod 000` on `mcp.json` made
    /// `omh why` report an installed server as "not installed here" and advise
    /// `omh init`, which does nothing because `write_if_absent` sees the file.
    /// Exit 0, no error, and no way out from the outside.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_layer_is_an_error_not_an_absent_one() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "mcp.json",
            r#"{"mcpServers":{"codegraph":{"command":"c"}}}"#,
        );
        let path = mcp_path(&paths);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = servers(&paths);
        // Restore before asserting, so a failure does not leave a locked file.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = result
            .expect_err("an unreadable layer must not read as empty")
            .to_string();
        assert!(err.contains("mcp.json"), "must name the file: {err}");
    }

    /// The other half: absent really is normal, and must stay silent. Most
    /// layers declare most things not at all.
    #[test]
    fn an_absent_layer_is_not_an_error() {
        let (_d, paths) = fixture();
        assert!(servers(&paths).unwrap().is_empty());
        assert!(policy(&paths).unwrap().is_empty());
        assert!(hooks(&paths).unwrap().is_empty());
    }

    #[test]
    fn a_non_json_file_in_the_hooks_directory_is_ignored() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Personal, "hooks/notes.md", "not a hook");
        seed(
            &paths,
            Layer::Personal,
            "hooks/real.json",
            r#"{"on":"turn-end","run":"c"}"#,
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
    fn layers_map_to_their_files() {
        let (_d, paths) = fixture();
        assert_eq!(
            Layer::Personal.file(&paths),
            paths.root.join("settings.toml")
        );
        assert_eq!(
            Layer::Shared.file(&paths),
            paths.repo.join(".omh/settings.toml")
        );
        assert_eq!(
            Layer::Local.file(&paths),
            paths.repo.join(".omh/settings.local.toml")
        );
    }

    /// The gitignored tier holds settings and no content. A hook is executable
    /// and arrives by `git clone`, which is why it is committed and disclosed —
    /// a gitignored one would be executable content with no reviewer at all.
    #[test]
    fn the_gitignored_layer_holds_no_content() {
        let (_d, paths) = fixture();
        assert!(Layer::Local.content_dir(&paths).is_none());
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
        settings(&paths, Layer::Personal, "idle_timeout = \"30m\"");
        let settings = policy(&paths).unwrap();
        assert_eq!(get(&settings, "idle_timeout").layer, Layer::Personal);
    }

    #[test]
    fn later_layers_win_and_the_loser_is_named() {
        let (_d, paths) = fixture();
        settings(&paths, Layer::Personal, "idle_timeout = \"30m\"");
        settings(&paths, Layer::Shared, "idle_timeout = \"5m\"");
        settings(&paths, Layer::Local, "idle_timeout = \"2h\"");

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
        settings(&paths, Layer::Shared, "carry_in = [\".env\"]");
        assert!(get(&policy(&paths).unwrap(), "carry_in").shadows.is_empty());
    }

    #[test]
    fn missing_layers_are_not_an_error() {
        let (_d, paths) = fixture();
        assert!(policy(&paths).unwrap().is_empty());
        assert!(servers(&paths).unwrap().is_empty());
    }

    #[test]
    fn mcp_servers_resolve_from_the_catalogue() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Personal,
            "mcp.json",
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"},
                              "omh-memory":{"command":"omh-mcp"}}}"#,
        );

        let s = servers(&paths).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(get(&s, "codegraph").layer, Layer::Personal);
        assert_eq!(get(&s, "codegraph").value, "codebase-memory-mcp");
        assert!(
            get(&s, "codegraph").shadows.is_empty(),
            "one catalogue, so nothing to shadow"
        );
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
        assert_eq!(w.path, Layer::Local.file(&paths));
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
        settings(
            &paths,
            Layer::Local,
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
        settings(&paths, Layer::Personal, "idle_timeout = \"30m\"");
        settings(&paths, Layer::Local, "idle_timeout = \"2h\"");

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
        let w = mcp_add(&paths, "g", server("c")).unwrap();
        assert_eq!(w.path, mcp_path(&paths));
        assert_eq!(names(&paths), ["g"]);
    }

    /// A server is a thing you hold, so it lands in your catalogue — and the
    /// catalogue is not committed, so nothing here reaches a teammate by
    /// `git clone`.
    ///
    /// This is what replaced `--layer`. A server used to be declarable in any
    /// of three places, with the gitignored one the default because MCP env
    /// carries tokens; one catalogue leaves one destination, and a token scoped
    /// to a single repo is a *setting* — `[mcp.<name>.env]` — rather than a
    /// second declaration of the server.
    #[test]
    fn mcp_add_writes_to_the_catalogue_and_commits_nothing() {
        let (_d, paths) = fixture();
        let w = mcp_add(&paths, "g", server("c")).unwrap();
        assert!(!w.committed);
        assert!(w.path.starts_with(&paths.root), "got: {}", w.path.display());
    }

    #[test]
    fn mcp_add_preserves_existing_servers() {
        let (_d, paths) = fixture();
        mcp_add(&paths, "first", server("a")).unwrap();
        mcp_add(&paths, "second", server("b")).unwrap();
        assert_eq!(names(&paths), ["first", "second"]);
    }

    #[test]
    fn mcp_rm_takes_the_server_out_of_the_catalogue() {
        let (_d, paths) = fixture();
        mcp_add(&paths, "keep", server("a")).unwrap();
        mcp_add(&paths, "drop", server("b")).unwrap();

        assert!(mcp_remove(&paths, "drop").unwrap());
        assert_eq!(names(&paths), ["keep"]);
    }

    #[test]
    fn mcp_rm_reports_when_nothing_was_removed() {
        let (_d, paths) = fixture();
        assert!(!mcp_remove(&paths, "absent").unwrap());
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
        let r = mcp_import(&paths, incoming(), false, false).unwrap();
        assert_eq!(r.added, ["a", "b"]);
        assert!(r.conflicts.is_empty() && r.unchanged.is_empty());
        assert_eq!(names(&paths), ["a", "b"]);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (_d, paths) = fixture();
        let r = mcp_import(&paths, incoming(), false, true).unwrap();
        assert_eq!(r.added, ["a", "b"], "the plan is still reported");
        assert!(names(&paths).is_empty(), "but nothing was written");
    }

    /// Re-importing the same config must be a no-op, not a duplicate or a churn
    /// of the file — otherwise `omh mcp import` is unsafe to re-run.
    #[test]
    fn importing_twice_changes_nothing() {
        let (_d, paths) = fixture();
        mcp_import(&paths, incoming(), false, false).unwrap();
        let second = mcp_import(&paths, incoming(), false, false).unwrap();
        assert_eq!(second.unchanged, ["a", "b"]);
        assert!(second.added.is_empty());
    }

    #[test]
    fn a_changed_server_is_a_conflict_not_a_silent_overwrite() {
        let (_d, paths) = fixture();
        mcp_add(&paths, "a", server("mine")).unwrap();

        let r = mcp_import(&paths, incoming(), false, false).unwrap();
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
        mcp_add(&paths, "a", server("mine")).unwrap();

        let r = mcp_import(&paths, incoming(), true, false).unwrap();
        assert_eq!(r.added, ["a", "b"]);
        assert!(r.conflicts.is_empty());
        assert_eq!(get(&servers(&paths).unwrap(), "a").value, "a-cmd");
    }

    #[test]
    fn importing_nothing_is_not_an_error() {
        let (_d, paths) = fixture();
        assert_eq!(
            mcp_import(&paths, BTreeMap::new(), false, false).unwrap(),
            Imported::default()
        );
    }

    #[test]
    fn a_list_setting_comes_back_as_a_list() {
        let (_d, paths) = fixture();
        settings(&paths, Layer::Shared, "carry_in = [\".env\", \"certs/\"]");
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
        settings(&paths, Layer::Personal, "carry_in = [\".env\"]");
        settings(&paths, Layer::Local, "carry_in = [\".env.local\"]");
        assert_eq!(policy_list(&paths, "carry_in"), vec![".env.local"]);
    }
}
