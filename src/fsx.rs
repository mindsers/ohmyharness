//! Filesystem questions answered without folding "could not tell" into "no",
//! or "could not read" into "nothing there".

use std::path::Path;

/// Whether something is still on disk — with "could not tell" kept apart from
/// "not there".
///
/// Not `Path::exists`, which is `metadata().is_ok()` and so folds every stat
/// failure into `false`. An unreadable parent, a directory whose search bit is
/// gone, a mount that went away mid-command: all of them answered "absent",
/// and a caller checking whether it had finished deleting something read that
/// as done. Measured on macOS: with the parent at `0o600` the child still
/// exists and `exists()` returns `false`.
pub(crate) fn still_there(at: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(at) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // Not followed, so a dangling symlink is its own presence rather than
        // the absence of what it points at.
        Err(e) => Err(format!("{e}")),
    }
}

/// Whether a thing this command is named for actually went.
///
/// **Not a bool.** `No` carries why, because a removal that did not happen and
/// a removal nobody could explain are the same news to a caller that only has
/// a flag — and this is the value `rm` prints to somebody deciding what to
/// clean up by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gone {
    Yes,
    No(String),
}

/// `path`'s text, or `None` when there is nothing at it. Only `NotFound` is
/// absence: a file omh could not read — a `chmod 000` one, a directory where a
/// file belongs — is an error naming it, never a file that declares nothing.
/// Reading it as nothing is how a closed loop once advised a user into the
/// state they were already in, and exited 0.
pub(crate) fn read_or_absent(path: &Path) -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absent, present, and there but unreadable are three answers. A
    /// directory where the file belongs is the uid-independent way to make
    /// the third, since CI runs as root and root reads through any mode.
    #[test]
    fn a_file_omh_cannot_read_is_an_error_naming_it_never_nothing_there() {
        let d = tempfile::tempdir().unwrap();
        let at = d.path().join("settings.toml");
        assert_eq!(read_or_absent(&at).unwrap(), None, "nothing there");
        std::fs::write(&at, "x = 1\n").unwrap();
        assert_eq!(read_or_absent(&at).unwrap().as_deref(), Some("x = 1\n"));
        std::fs::remove_file(&at).unwrap();
        std::fs::create_dir(&at).unwrap();
        let err = format!(
            "{:#}",
            read_or_absent(&at).expect_err("a directory read as absent")
        );
        assert!(
            err.contains("settings.toml"),
            "the error names the file: {err}"
        );
    }

    /// Absent, present, and unanswerable are three answers, and `Path::exists`
    /// collapses the third into the first.
    #[test]
    #[cfg(unix)]
    fn a_directory_that_cannot_be_stated_is_neither_there_nor_gone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let parent = d.path().join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();

        assert_eq!(still_there(&child), Ok(true), "it is there");
        assert_eq!(
            still_there(&parent.join("never")),
            Ok(false),
            "and that one is not"
        );

        let was = std::fs::metadata(&parent).unwrap().permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o600)).unwrap();
        let blind = still_there(&child);
        std::fs::set_permissions(&parent, was).unwrap();

        assert!(
            blind.is_err(),
            "an unreadable parent is not an answer of `false`: {blind:?}"
        );
    }
}
