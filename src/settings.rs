//! `settings.toml` — what this repo says about omh's own features.
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
//! Everything else — `carry_in`, `idle_timeout`, `[use]` — still lives in
//! `policy.toml` and moves here when the catalogue does. A key that arrives
//! early is refused by name rather than read and ignored.

use crate::base::Manifest;
use crate::profile::Paths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    omh: BTreeMap<String, bool>,
}

/// Personal, then this repo's, then this repo's gitignored — later winning.
///
/// The same order every other layered thing here uses, so a machine-wide
/// preference and a one-repo exception are both expressible.
fn layers(paths: &Paths) -> [PathBuf; 3] {
    [
        paths.root.join("settings.toml"),
        paths.repo.join(".omh/settings.toml"),
        paths.repo.join(".omh/settings.local.toml"),
    ]
}

/// Which of omh's features this repo has switched off.
pub fn features_off(paths: &Paths, manifest: &Manifest) -> Result<BTreeSet<String>> {
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    for path in layers(paths) {
        let Some(raw) = read(&path)? else {
            continue;
        };
        let file: File =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        for (key, on) in file.omh {
            validate(&key, manifest, &path)?;
            state.insert(key, on);
        }
    }
    Ok(state
        .into_iter()
        .filter(|(_, on)| !on)
        .map(|(name, _)| name)
        .collect())
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

    #[test]
    fn a_repo_with_no_settings_has_everything_on() {
        let (_d, paths, m) = fixture();
        assert!(features_off(&paths, &m).unwrap().is_empty());
    }

    #[test]
    fn a_feature_named_false_is_off() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n",
        );
        assert_eq!(
            features_off(&paths, &m).unwrap(),
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
            features_off(&paths, &m).unwrap().is_empty(),
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
        let err = features_off(&paths, &m).unwrap_err().to_string();
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
        let err = features_off(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("teleport"), "got: {err}");
        assert!(err.contains("codegraph") && err.contains("memory"), "{err}");
    }

    /// `settings.toml` holds one table today. A `carry_in` written here would
    /// otherwise be read by nobody and reported by nothing, which is the exact
    /// shape of a setting somebody swears they configured.
    #[test]
    fn a_key_that_does_not_live_here_yet_is_refused() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        );
        // `{:#}` — the key is named by serde's own message, one level down
        // the context chain from the file.
        let err = format!("{:#}", features_off(&paths, &m).unwrap_err());
        assert!(err.contains("carry_in"), "must name the key: {err}");
        assert!(err.contains("settings.toml"), "and the file: {err}");
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

        let err = features_off(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("settings.toml"), "must name the file: {err}");
    }
}
