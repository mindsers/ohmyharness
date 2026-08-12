//! `[omh]` — which of omh's own features are on here.
//!
//! One table for now. `[omh]` names **features**, never entries: `codegraph`
//! is the server, its four hooks and its section of the rules, and switching
//! half of it off produces a graph that quietly stops tracking the code. That
//! state is unrepresentable rather than warned about, which is what removed the
//! guard, the manifest field and the launch warning an earlier design needed.
//!
//! Disabling is not removal. A feature off here leaves your `mcp.json` exactly
//! as you have it; the server is dropped from the document this session is
//! given, and the next repo gets it back.
//!
//! Everything else in these files — `carry_in`, `idle_timeout`, and `[use]`
//! when it lands — is read by [`crate::config::policy`], which resolves the
//! same three paths with provenance. Two readers of one file rather than two
//! files: a setting and a feature switch are both something a repo decided, and
//! `policy.toml` was a fourth name for that idea living inside a directory whose
//! purpose was content.

use crate::base::Manifest;
use crate::profile::Paths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Deliberately not `deny_unknown_fields`: this file holds settings too, and
/// `config::policy` is what reads them. Denying here would make a `carry_in`
/// beside `[omh]` an error in one reader and a value in the other.
///
/// That argument covers *scalars* and stops there. `[omh]` and `[mcp]` are the
/// complete set of tables either reader understands, so an unrecognised one is
/// read by nobody and reported by nothing — which is why `rest` is collected
/// and checked rather than ignored.
#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    omh: BTreeMap<String, bool>,
    #[serde(default)]
    mcp: BTreeMap<String, ServerOverride>,
    #[serde(flatten)]
    rest: toml::Table,
}

/// What a repo may say about a catalogue server. Environment and nothing else:
/// a repo names entries from your catalogue, it does not define one, so there
/// is deliberately no `command` here to redeclare it with.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerOverride {
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// The gitignored layer's filename, so `init` ignores exactly the file this
/// module reads.
///
/// It is not covered by the `local/` line `init` already writes — that ignores
/// the *directory* `.omh/local`, and this is a file beside it. Documenting a
/// tracked file as gitignored is how a machine-local override gets committed
/// to somebody's team repo.
pub const LOCAL: &str = "settings.local.toml";

/// Personal, then this repo's, then this repo's gitignored — later winning.
///
/// Read from `config::Layer` rather than spelled again, so the file a feature
/// switch is read from and the file a setting is read from cannot drift apart.
fn layers(paths: &Paths) -> [PathBuf; 3] {
    crate::config::Layer::ALL.map(|l| l.file(paths))
}

/// Which of omh's features this repo has switched off, and what it says about
/// the environment of the servers it uses.
///
/// One pass for both, because they come from the same three files and a second
/// pass would be a second chance for the two to disagree about which layer won.
pub fn resolve(paths: &Paths, manifest: &Manifest) -> Result<Resolved> {
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    let mut mcp_env: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for path in layers(paths) {
        let Some(raw) = read(&path)? else {
            continue;
        };
        let file: File =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        // A table — or an array *of* tables, which `is_table()` answers `false`
        // for, so `[[omhh]]` slipped through and was read by nobody. A scalar
        // or a plain array falls through on purpose: `carry_in = [".env"]` is
        // a setting, and `config::policy` resolves those.
        let is_table_like = |v: &toml::Value| {
            v.is_table()
                || v.as_array()
                    .is_some_and(|a| a.iter().any(toml::Value::is_table))
        };
        for (key, value) in &file.rest {
            if is_table_like(value) {
                anyhow::bail!(
                    "{}: `[{key}]` is read by nobody. This file holds settings at the \
                     top level, `[omh]` for omh's own features, and `[mcp.<name>.env]` \
                     for a server's environment in this repo.",
                    path.display()
                );
            }
        }
        for (key, on) in file.omh {
            validate(&key, manifest, &path)?;
            state.insert(key, on);
        }
        // Variable by variable, so a later layer adding a token does not drop
        // the one an earlier layer set.
        for (name, over) in file.mcp {
            mcp_env.entry(name).or_default().extend(over.env);
        }
    }
    Ok(Resolved {
        off: state
            .into_iter()
            .filter(|(_, on)| !on)
            .map(|(name, _)| name)
            .collect(),
        mcp_env,
    })
}

/// What the settings files say that the launcher acts on.
#[derive(Debug, Default)]
pub struct Resolved {
    pub off: BTreeSet<String>,
    pub mcp_env: BTreeMap<String, BTreeMap<String, String>>,
}

/// A key has to be a feature. Checked where the value is minted, the rule
/// `memory::expand_key` states, so every reader inherits the one guard.
///
/// An entry name is the interesting error: it is how somebody discovers the
/// grouping without reading the manifest, and it is the request the design
/// deliberately refuses — `graph-first = false` would mean keeping the graph
/// and dropping one of the things that make it used, which is a bundle taken
/// apart rather than a setting.
fn validate(key: &str, manifest: &Manifest, path: &Path) -> Result<()> {
    let features: BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    if features.contains(key) {
        return Ok(());
    }
    if let Some(entry) = manifest.entry(key) {
        anyhow::bail!(
            "{}: `{key}` is part of the `{}` feature, not a feature itself. \
             Write `{} = false` to switch off all of it — there is no way to \
             keep the rest and drop this one.",
            path.display(),
            entry.feature,
            entry.feature
        );
    }
    anyhow::bail!(
        "{}: `{key}` is not one of omh's features ({})",
        path.display(),
        features.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// Absent is not unreadable. `config::read_layer` records what conflating them
/// cost: a `chmod 000` file reported as "not declared", advice that could not
/// help, and a closed loop exiting 0.
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    fn fixture() -> (tempfile::TempDir, Paths, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        for d in [&paths.root, &paths.repo.join(".omh")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let manifest = Manifest::load_dir(Path::new(BASE)).unwrap();
        (dir, paths, manifest)
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// `init` ignores a filename; this module reads a path. They have to be
    /// the same file, and `.omh/.gitignore`'s existing `local/` line does not
    /// cover it — that is a directory, this is a file beside it. A tracked
    /// `settings.local.toml` is a machine-local override committed to a team
    /// repo.
    #[test]
    fn the_gitignored_layer_is_the_file_init_ignores() {
        let (_d, paths, _m) = fixture();
        let last = layers(&paths).last().unwrap().clone();
        assert_eq!(last.file_name().unwrap().to_string_lossy(), LOCAL);
        assert!(last.starts_with(paths.repo.join(".omh")));
    }

    #[test]
    fn a_repo_with_no_settings_has_everything_on() {
        let (_d, paths, m) = fixture();
        assert!(resolve(&paths, &m).unwrap().off.is_empty());
    }

    #[test]
    fn a_feature_named_false_is_off() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n",
        );
        assert_eq!(
            resolve(&paths, &m).unwrap().off,
            BTreeSet::from(["codegraph".to_string()])
        );
    }

    /// A machine-wide preference and a one-repo exception both have to be
    /// expressible, or the layering is decoration.
    #[test]
    fn a_later_layer_wins() {
        let (_d, paths, m) = fixture();
        write(paths.root.join("settings.toml"), "[omh]\nmemory = false\n");
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[omh]\nmemory = true\n",
        );
        assert!(
            resolve(&paths, &m).unwrap().off.is_empty(),
            "this repo turned it back on"
        );
    }

    /// The state "graph on, refresher off" has to be unrepresentable rather
    /// than warned about — a graph that quietly stops tracking the code is the
    /// one combination that manufactures confident wrong answers.
    ///
    /// The error names the feature, which is also how somebody discovers the
    /// grouping without reading the manifest.
    #[test]
    fn a_hook_name_where_a_feature_belongs_names_the_feature() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ngraph-first = false\n",
        );
        // Not "mentions codegraph": the unknown-key error lists every feature
        // and would satisfy that while saying nothing about the grouping. The
        // guard is that this key is *part of* something, and which.
        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(
            err.contains("`graph-first` is part of the `codegraph` feature"),
            "must say what it belongs to: {err}"
        );
        assert!(
            err.contains("codegraph = false"),
            "and what to write instead: {err}"
        );
    }

    #[test]
    fn an_unknown_feature_lists_the_features() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\nteleport = false\n",
        );
        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("teleport"), "got: {err}");
        assert!(err.contains("codegraph") && err.contains("memory"), "{err}");
    }

    /// A repo says what a catalogue server's environment should be here, and
    /// nothing more — there is no `command`, so a repo cannot define a server
    /// by pretending to configure one.
    #[test]
    fn a_repo_overrides_a_servers_env_and_only_that() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[mcp.linear.env]\nLINEAR_API_KEY = \"secret\"\n",
        );
        let r = resolve(&paths, &m).unwrap();
        assert_eq!(r.mcp_env["linear"]["LINEAR_API_KEY"], "secret");

        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[mcp.linear]\ncommand = \"mine\"\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());
        assert!(err.contains("command"), "must name the key: {err}");
    }

    /// Variable by variable, so a machine-wide token and a per-repo region are
    /// both expressible — merging entry by entry would make the later layer
    /// silently drop the earlier one's variables.
    #[test]
    fn env_overrides_merge_variable_by_variable() {
        let (_d, paths, m) = fixture();
        write(
            paths.root.join("settings.toml"),
            "[mcp.linear.env]\nTOKEN = \"t\"\n",
        );
        write(
            paths.repo.join(".omh/settings.toml"),
            "[mcp.linear.env]\nREGION = \"eu\"\n",
        );
        let env = &resolve(&paths, &m).unwrap().mcp_env["linear"];
        assert_eq!(env["TOKEN"], "t");
        assert_eq!(env["REGION"], "eu");
    }

    /// A table nobody reads is refused by name.
    ///
    /// `deny_unknown_fields` came off `File` so a `carry_in` beside `[omh]`
    /// would not be an error in one reader and a value in the other — right
    /// for scalars, which `config::policy` does read. It does not extend to
    /// tables: `[omh]` and `[mcp]` are the complete set either reader
    /// understands, so `[omhh]` or `[mcpp]` is read by nobody and reported by
    /// nothing. A token that reaches nothing, silently, is the shape both
    /// modules exist to refuse.
    #[test]
    fn a_table_nobody_reads_is_refused_by_name() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omhh]\ncodegraph = false\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());
        assert!(err.contains("omhh"), "must name the table: {err}");
        assert!(err.contains("settings.toml"), "and the file: {err}");

        // And a scalar still is not: `config::policy` reads those.
        write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        );
        assert!(
            resolve(&paths, &m).is_ok(),
            "a setting is not an unknown key"
        );
    }

    /// Absent is not unreadable — the `config::read_layer` lesson, which cost a
    /// closed loop that exited 0.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_settings_file_is_an_error_not_an_absent_one() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths, m) = fixture();
        let path = paths.repo.join(".omh/settings.toml");
        write(path.clone(), "[omh]\ncodegraph = false\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("settings.toml"), "must name the file: {err}");
    }
}
