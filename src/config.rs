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
/// Reported the same way as [`servers`] — `omh why` treats a server and a hook
/// as the same kind of thing, something installed from somewhere that omh
/// either chose or you did — but resolved differently: hooks have two tiers and
/// can genuinely shadow, servers have one catalogue and never do.
/// `render::merge_hooks` merges for *rendering* and drops the tier, which is
/// the one fact this needs.
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
                    format!(
                        "{}: server `{name}` is not one omh can run. omh launches \
                         stdio servers — {{\"command\": …, \"args\": […], \"env\": {{…}}}} \
                         — and has no support for remote/HTTP ones yet.",
                        path.display()
                    )
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
    edit_layer(paths, layer, |doc| {
        refuse_a_table(doc, key)?;
        doc[key] = toml_edit::Item::Value(parse_value(raw));
        Ok(())
    })
}

pub fn unset(paths: &Paths, key: &str, layer: Layer) -> Result<bool> {
    let path = layer.file(paths);
    let mut doc = read_doc(&path)?;
    refuse_a_table(&doc, key).with_context(|| format!("editing {}", path.display()))?;
    if doc.remove(key).is_none() {
        return Ok(false);
    }
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// A table is configuration *for* something, never a setting with a value.
///
/// `write_table` refuses the opposite direction and says why; the guard was
/// one-way, so `omh repo set omh false` replaced the whole `[omh]` table with a
/// scalar. That is worse than losing the switches it held: `settings::File`
/// deserialises `omh` as a map, so every command afterwards failed to parse the
/// file — while the write printed a path and exited 0.
///
/// `config::policy` already skips tables when *reading*, for the same reason.
/// This is that rule on the way out.
fn refuse_a_table(doc: &toml_edit::DocumentMut, key: &str) -> Result<()> {
    // `as_table_like`, so an inline `omh = { codegraph = false }` counts — it is
    // valid TOML and `settings.rs` reads it, so a guard that could not see it
    // would be the same bug one spelling over.
    if doc
        .as_table()
        .get(key)
        .is_some_and(|item| item.as_table_like().is_some())
    {
        anyhow::bail!(
            "`{key}` is a table, not a setting — omh will not replace it with a value. \
             `[omh]` is `omh repo enable`/`disable`, `[use]` is `omh use`/`unuse`, and \
             `[mcp.<name>.env]` is edited by hand."
        );
    }
    Ok(())
}

/// The table `[use]` lives in, and the one `[omh]` lives in. Spelled once here
/// rather than at each writer, so the reader in `settings.rs` and the writers
/// below cannot drift about what a table is called.
pub const USE: &str = "use";
pub const OMH: &str = "omh";

/// Write one capability's `[use]` list, or several.
///
/// A whole list per capability rather than an append, because that is what the
/// table means: `[use]` is an allowlist and the value *is* the selection. The
/// caller computes the new list — `omh use` from the effective one plus a name,
/// `omh use --all` from the catalogue — so the one place that knows "absent
/// means everything" stays [`crate::selection`] rather than being re-derived
/// inside a writer.
pub fn write_selection(
    paths: &Paths,
    layer: Layer,
    lists: &BTreeMap<crate::adapter::Capability, Vec<String>>,
) -> Result<Written> {
    write_table(paths, layer, USE, |table| {
        for (cap, names) in lists {
            let mut array = toml_edit::Array::new();
            for name in names {
                array.push(name.as_str());
            }
            table[&cap.to_string()] = toml_edit::value(array);
        }
    })
}

/// Switch one of omh's features on or off here.
pub fn write_feature(paths: &Paths, layer: Layer, feature: &str, on: bool) -> Result<Written> {
    write_table(paths, layer, OMH, |table| {
        table[feature] = toml_edit::value(on);
    })
}

/// The repo layers a write to `table.key` has to reach.
///
/// Always the committed file: what a project uses, and which of omh's features
/// it runs with, are facts about the project, and a teammate cloning it should
/// get them. **And** the gitignored one when it already declares the same key —
/// because `settings::resolve` applies the layers in order with that one last,
/// so writing only the committed file made `omh unuse` report success while the
/// entry it removed was still being staged. A command that removes something
/// has to remove it.
///
/// Never a layer that does not already declare it. A `[use]` table appearing in
/// a gitignored file because a committed one was edited is how a teammate stops
/// getting what the repo says it uses — and `Personal` is absent for a
/// different reason: it is *lower* precedence than `Shared`, so it can never
/// shadow the write, and rewriting your default for every project because you
/// curated one repo would be the worst of the three.
pub fn declaring(paths: &Paths, table: &str, key: &str) -> Result<Vec<Layer>> {
    let mut out = vec![Layer::Shared];
    if declares_key(paths, Layer::Local, table, key)? {
        out.push(Layer::Local);
    }
    Ok(out)
}

/// Does this layer's file have a `[table]` at all?
///
/// `init` asks, to decide whether a repo already says what it uses — and the
/// answer decides whether it overwrites a list somebody pruned, so absent and
/// unreadable must not be the same answer. `read_doc` is what keeps them apart.
pub fn declares(paths: &Paths, layer: Layer, table: &str) -> Result<bool> {
    Ok(read_doc(&layer.file(paths))?.contains_key(table))
}

fn declares_key(paths: &Paths, layer: Layer, table: &str, key: &str) -> Result<bool> {
    Ok(read_doc(&layer.file(paths))?
        .get(table)
        // `as_table_like`, so an inline `use = { skills = [...] }` counts. It is
        // valid TOML and `settings.rs` reads it, so a writer that could not see
        // it would be the same bug one spelling over.
        .and_then(|item| item.as_table_like())
        .is_some_and(|t| t.contains_key(key)))
}

/// Read-modify-write one named table inside a layer's file.
fn write_table(
    paths: &Paths,
    layer: Layer,
    name: &str,
    edit: impl FnOnce(&mut toml_edit::Table),
) -> Result<Written> {
    edit_layer(paths, layer, |doc| {
        // The entry API, not `doc[name]`: indexing a document with a key it
        // does not have *panics*, and the first `omh use` in a repo is exactly
        // that case.
        let item = doc
            .as_table_mut()
            .entry(name)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        // A non-table under this name would be silently replaced, taking
        // whatever somebody wrote with it. Refused instead, naming the file.
        let Some(table) = item.as_table_mut() else {
            anyhow::bail!("`{name}` is not a table — omh will not overwrite it");
        };
        edit(table);
        Ok(())
    })
}

/// Read a layer's file as a **document**, apply an edit, write it back.
///
/// `DocumentMut` rather than `toml::Table` and `to_string_pretty`, and the
/// difference is not cosmetic: a settings file is one somebody maintains by
/// hand, `omh init` writes it full of explanatory comments, and P4 turned
/// writing it from something `omh config set` did occasionally into something
/// `omh use`, `omh unuse` and `omh repo enable` all do. A serializer round trip
/// deletes every comment in the file, which is deleting what the user wrote.
fn edit_layer(
    paths: &Paths,
    layer: Layer,
    edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<Written> {
    let path = layer.file(paths);
    let mut doc = read_doc(&path)?;
    edit(&mut doc).with_context(|| format!("editing {}", path.display()))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(Written {
        path,
        layer,
        committed: layer.is_committed(),
    })
}

/// Absent is not unreadable, and this is where conflating them **destroys**:
/// every caller is a read-modify-write, so an error read as "empty" turns the
/// write into a replacement. One byte that is not UTF-8 in `settings.toml` used
/// to take every `[omh]` switch and every `[mcp.<name>.env]` token with it, and
/// print success.
fn read_doc(path: &Path) -> Result<toml_edit::DocumentMut> {
    let Some(raw) = read_layer(path)? else {
        return Ok(toml_edit::DocumentMut::new());
    };
    raw.parse()
        .with_context(|| format!("parsing {}", path.display()))
}

/// Accept TOML literals (`["a", "b"]`, `true`, `30`) and fall back to a bare
/// string, so `omh set idle_timeout 30m` does not need quoting.
fn parse_value(raw: &str) -> toml_edit::Value {
    raw.parse::<toml_edit::Value>()
        .unwrap_or_else(|_| toml_edit::Value::from(raw))
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
/// The same rule as `read_table`, and the same stakes. `mcp_add` starting from
/// an empty map on an unreadable catalogue writes a file holding only the
/// server just added — every other server, for every repo, gone with a success
/// message. `mcp_import` is worse still: its "never clobber what you wrote by
/// hand" guard is built on `all.get(&name)`, so an empty map classifies every
/// incoming server as new and overwrites without `--force`.
fn read_servers(path: &Path) -> Result<BTreeMap<String, Server>> {
    let Some(raw) = read_layer(path)? else {
        return Ok(BTreeMap::new());
    };
    crate::render::parse(Render::McpJson, &raw)
        .with_context(|| format!("reading {}", path.display()))
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

    /// A file omh cannot read is never a file omh may overwrite.
    ///
    /// `set` and `mcp_add` are read-modify-write. Treating every read error as
    /// "empty" meant one unreadable byte — a token pasted with a stray
    /// character, a file an editor saved as UTF-16 — turned the write into a
    /// **replacement**: `omh config set idle_timeout 30m` would report success
    /// having deleted every `[omh]` switch and every `[mcp.<name>.env]` token
    /// beside it.
    ///
    /// This got sharper in this PR, twice over: settings layers used to be a
    /// `policy.toml` holding scalars, and now hold the feature switches and the
    /// MCP environment; and `mcp.json` used to be one of three mergeable layers
    /// and is now the single catalogue for every repo.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_never_overwritten_with_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths) = fixture();

        // Not a permissions trick: bytes that are not UTF-8, which is what a
        // mispasted token actually looks like.
        let settings = Layer::Local.file(&paths);
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, [b'k', b'=', b'"', 0xff, b'"']).unwrap();
        assert!(
            set(&paths, "idle_timeout", "30m", Layer::Local).is_err(),
            "an unreadable settings file must stop the write, not replace it"
        );

        let catalogue = mcp_path(&paths);
        std::fs::create_dir_all(catalogue.parent().unwrap()).unwrap();
        std::fs::write(&catalogue, [0xff, 0xfe, b'{']).unwrap();
        let before = std::fs::read(&catalogue).unwrap();
        assert!(
            mcp_add(&paths, "new", server("c")).is_err(),
            "an unreadable catalogue must stop the write, not replace it"
        );
        assert_eq!(
            std::fs::read(&catalogue).unwrap(),
            before,
            "and must leave every server you had on disk"
        );
        let _ = std::fs::set_permissions(&catalogue, std::fs::Permissions::from_mode(0o644));
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

    #[test]
    fn set_creates_the_layer_file_when_absent() {
        let (_d, paths) = fixture();
        let w = set(&paths, "idle_timeout", "30m", Layer::Local).unwrap();
        assert_eq!(w.path, Layer::Local.file(&paths));
        assert!(!w.committed);
        assert_eq!(get(&policy(&paths).unwrap(), "idle_timeout").value, "30m");
    }

    /// A scalar must not land where a table lives.
    ///
    /// `write_table` refuses the opposite direction and says why — "a non-table
    /// under this name would be silently replaced, taking whatever somebody
    /// wrote with it" — and the guard was one-way, so `omh repo set omh false`
    /// replaced the whole `[omh]` table with `omh = false`. That is worse than
    /// losing the switches: `settings::File` deserialises `omh` as a map, so
    /// **every subsequent command** — launch, `omh repo`, `omh use` — failed to
    /// parse the file, while the write itself printed a path and exited 0.
    ///
    /// `[use]` and `[mcp]` are the same shape of accident one key over.
    #[test]
    fn a_scalar_never_replaces_a_table() {
        let (_d, paths) = fixture();
        // Both tables have to exist, or this passes for the wrong reason: a key
        // no table answers to is an ordinary setting and `set` is right to take
        // it. The first draft of this test looped over three names and only one
        // of them was a table.
        write_feature(&paths, Layer::Shared, "codegraph", false).unwrap();
        write_selection(
            &paths,
            Layer::Shared,
            &BTreeMap::from([(crate::adapter::Capability::Skills, vec!["mine".to_string()])]),
        )
        .unwrap();

        for key in [OMH, USE] {
            let before = std::fs::read_to_string(Layer::Shared.file(&paths)).unwrap();
            let err = set(&paths, key, "false", Layer::Shared)
                .expect_err("`{key}` names a table, and a table is not a setting");
            assert!(format!("{err:#}").contains(key), "name it: {err:#}");
            assert_eq!(
                std::fs::read_to_string(Layer::Shared.file(&paths)).unwrap(),
                before,
                "and the refusal has to leave the file alone"
            );
        }
        // A key that is not a table is still an ordinary setting.
        assert!(set(&paths, "idle_timeout", "30m", Layer::Shared).is_ok());
    }

    /// `unset` is the same read-modify-write and would drop the whole table.
    #[test]
    fn unset_refuses_to_take_a_table_away() {
        let (_d, paths) = fixture();
        write_feature(&paths, Layer::Shared, "codegraph", false).unwrap();
        let err = unset(&paths, OMH, Layer::Shared)
            .expect_err("removing `[omh]` is not removing a setting");
        assert!(format!("{err:#}").contains(OMH), "name it: {err:#}");
        assert!(
            std::fs::read_to_string(Layer::Shared.file(&paths))
                .unwrap()
                .contains("codegraph"),
            "and the table survives"
        );
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
