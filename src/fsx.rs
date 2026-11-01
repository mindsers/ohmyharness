//! Filesystem questions answered without folding "could not tell" into "no".

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

#[cfg(test)]
mod tests {
    use super::*;

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
