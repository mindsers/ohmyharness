//! Editors are data, exactly like adapters.
//!
//! `omh <name>` means "attach this tool to the session". A harness runs inside
//! it; an editor attaches from outside over SSH. Same gesture, so the same
//! dispatch — and adding an editor stays a TOML file rather than a match arm,
//! which is the whole reason adapters work.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Editor {
    /// Checked as it is deserialized, exactly as an adapter's is — see
    /// [`crate::adapter::Name`]. It keys no path today; a rule that held for
    /// one catalogue and not its twin is one nobody could rely on.
    #[serde(deserialize_with = "crate::adapter::de_name")]
    pub name: crate::adapter::Name,
    /// Executable on the **host** — an editor is not installed in the sandbox.
    pub bin: String,
    /// Arguments, with `$ALIAS` and `$URL` substituted.
    pub args: Vec<String>,
}

impl Editor {
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
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.push(
                    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?,
                );
            }
        }
        out.sort_by(|a: &Self, b: &Self| a.name.cmp(&b.name));
        Ok(out)
    }

    /// An editor by name, or `None` — including when the word is not a name at
    /// all. The same join, and so the same hazard, as `Adapter::find`; see
    /// `adapter::Name`.
    pub fn find(dir: &Path, name: &str) -> Option<Self> {
        let name = crate::adapter::Name::parse(name).ok()?;
        let path = dir.join(format!("{name}.toml"));
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
    }

    /// The command to run on the host.
    pub fn command(&self, alias: &str) -> Vec<String> {
        let url = format!("ssh://{alias}/work");
        std::iter::once(self.bin.clone())
            .chain(
                self.args
                    .iter()
                    .map(|a| a.replace("$ALIAS", alias).replace("$URL", &url)),
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/editors");

    fn zed() -> Editor {
        Editor::find(Path::new(BUNDLED), "zed").expect("bundled zed editor")
    }

    #[test]
    fn the_bundled_editors_parse() {
        let names: Vec<_> = Editor::load_dir(Path::new(BUNDLED))
            .unwrap()
            .into_iter()
            .map(|e| e.name.to_string())
            .collect();
        assert!(names.contains(&"zed".to_string()), "got: {names:?}");
        assert!(names.contains(&"code".to_string()), "got: {names:?}");
    }

    #[test]
    fn placeholders_are_substituted() {
        let cmd = zed().command("omh-repo-s01");
        assert_eq!(cmd[0], "zed");
        assert!(
            cmd.iter().any(|a| a.contains("omh-repo-s01")),
            "alias never reached the command: {cmd:?}"
        );
        assert!(
            !cmd.iter().any(|a| a.contains('$')),
            "unsubstituted placeholder: {cmd:?}"
        );
    }

    #[test]
    fn vscode_uses_its_remote_syntax() {
        let cmd = Editor::find(Path::new(BUNDLED), "code")
            .unwrap()
            .command("omh-x-s01");
        assert!(
            cmd.contains(&"ssh-remote+omh-x-s01".to_string()),
            "got: {cmd:?}"
        );
        assert!(cmd.contains(&"/work".to_string()));
    }

    #[test]
    fn cursor_is_its_own_entry_not_a_special_case() {
        let cmd = Editor::find(Path::new(BUNDLED), "cursor")
            .unwrap()
            .command("omh-x-s01");
        assert_eq!(cmd[0], "cursor");
    }

    #[test]
    fn an_unknown_editor_is_simply_absent() {
        assert!(Editor::find(Path::new(BUNDLED), "mystery-ide").is_none());
    }

    /// A word that is not a name does not reach the filesystem.
    ///
    /// `find` joins its argument into `<dir>/<word>.toml`, so `../planted`
    /// names a file outside the catalogue. The name check that stops it had no
    /// guard: deleting the line left the whole suite green.
    ///
    /// **The target is planted first**, which is the whole construction: `find`
    /// answers `None` both for "not a name" and for "no such file", so a test
    /// against a word that resolves to nothing passes either way and proves
    /// nothing. With the file there, `None` can only mean the word was refused.
    #[test]
    fn an_editor_name_is_not_a_path() {
        let d = tempfile::tempdir().unwrap();
        let editors = d.path().join("editors");
        std::fs::create_dir_all(&editors).unwrap();
        std::fs::write(
            d.path().join("planted.toml"),
            "name = \"planted\"\nbin = \"planted\"\nargs = []\n",
        )
        .unwrap();

        // The planted file is real: loaded from its own directory it parses.
        assert!(
            Editor::find(d.path(), "planted").is_some(),
            "the fixture has to be a readable editor, or the refusal below is \
             indistinguishable from a broken file"
        );
        assert!(
            Editor::find(&editors, "../planted").is_none(),
            "a word with `..` in it reached a file outside the catalogue"
        );
    }

    /// A stray key means a typo'd editor file that silently does the wrong
    /// thing — same reasoning as adapters.
    #[test]
    fn unknown_keys_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("bad.toml"),
            "name = \"bad\"\nbin = \"bad\"\nargs = []\nflavour = \"oops\"\n",
        )
        .unwrap();
        assert!(Editor::load_dir(d.path()).is_err());
    }
}
