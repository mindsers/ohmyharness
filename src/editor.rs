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
    pub name: String,
    /// Executable on the **host** — an editor is not installed in the sandbox.
    pub bin: String,
    /// Arguments, with `$ALIAS` and `$URL` substituted.
    pub args: Vec<String>,
}

impl Editor {
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.push(
                    toml::from_str(&raw)
                        .with_context(|| format!("parsing {}", path.display()))?,
                );
            }
        }
        out.sort_by(|a: &Self, b: &Self| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn find(dir: &Path, name: &str) -> Option<Self> {
        let path = dir.join(format!("{name}.toml"));
        std::fs::read_to_string(path).ok().and_then(|raw| toml::from_str(&raw).ok())
    }

    /// The command to run on the host.
    pub fn command(&self, alias: &str) -> Vec<String> {
        let url = format!("ssh://{alias}/work");
        std::iter::once(self.bin.clone())
            .chain(self.args.iter().map(|a| {
                a.replace("$ALIAS", alias).replace("$URL", &url)
            }))
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
        let names: Vec<_> =
            Editor::load_dir(Path::new(BUNDLED)).unwrap().into_iter().map(|e| e.name).collect();
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
        assert!(!cmd.iter().any(|a| a.contains('$')), "unsubstituted placeholder: {cmd:?}");
    }

    #[test]
    fn vscode_uses_its_remote_syntax() {
        let cmd = Editor::find(Path::new(BUNDLED), "code").unwrap().command("omh-x-s01");
        assert!(cmd.contains(&"ssh-remote+omh-x-s01".to_string()), "got: {cmd:?}");
        assert!(cmd.contains(&"/work".to_string()));
    }

    #[test]
    fn cursor_is_its_own_entry_not_a_special_case() {
        let cmd = Editor::find(Path::new(BUNDLED), "cursor").unwrap().command("omh-x-s01");
        assert_eq!(cmd[0], "cursor");
    }

    #[test]
    fn an_unknown_editor_is_simply_absent() {
        assert!(Editor::find(Path::new(BUNDLED), "mystery-ide").is_none());
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
