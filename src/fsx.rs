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

/// Keep `bytes`, what `path` held, beside it, and answer the name they are
/// under: `<name>.yours`, or `<name>.yours.1` and on when that is taken.
///
/// **Never over an earlier one.** A `.yours` is the only copy of somebody's
/// edit. It was written whatever was there, so the next release to change the
/// file took omh's own older copy for an edit and wrote it over the real one.
/// `create_new` is the check: it fails on anything at that name, a link
/// included, and between looking and writing nothing can arrive.
///
/// The same bytes are the same copy, so a refresh that finds the same edit
/// again answers the name it is already under. A copy omh cannot read is not
/// known to be the same, and takes the next name.
///
/// With `path`'s permissions, set before a byte is written: `mcp.json` holds
/// tokens, and one made private does not come back readable to all.
pub(crate) fn keep_yours(path: &Path, bytes: &[u8]) -> anyhow::Result<String> {
    use anyhow::Context as _;
    use std::io::Write as _;
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name to keep", path.display()))?
        .to_string_lossy();
    let like = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .permissions();
    let mut taken = 0_u32;
    loop {
        let saved = match taken {
            0 => format!("{name}.yours"),
            n => format!("{name}.yours.{n}"),
        };
        let at = path.with_file_name(&saved);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&at)
        {
            Ok(mut file) => {
                file.set_permissions(like)
                    .and_then(|()| file.write_all(bytes))
                    .with_context(|| format!("saving {name} as {}", at.display()))?;
                return Ok(saved);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(&at).is_ok_and(|held| held == bytes) {
                    return Ok(saved);
                }
                taken += 1;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("saving {name} as {}", at.display()));
            }
        }
    }
}

/// `dir`'s entries, or `None` when there is no directory at all. Only
/// `NotFound` is absence: a directory omh could not read is an error naming
/// it, never one that holds nothing.
pub(crate) fn read_dir_or_absent(dir: &Path) -> anyhow::Result<Option<std::fs::ReadDir>> {
    use anyhow::Context as _;
    match std::fs::read_dir(dir) {
        Ok(entries) => Ok(Some(entries)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same three answers for a directory: none there, entries, and a
    /// file where the directory belongs, which is an error naming it.
    #[test]
    fn a_directory_omh_cannot_list_is_an_error_naming_it_never_empty() {
        let d = tempfile::tempdir().unwrap();
        let at = d.path().join("stacks");
        assert!(read_dir_or_absent(&at).unwrap().is_none(), "nothing there");
        std::fs::create_dir(&at).unwrap();
        std::fs::write(at.join("rust.toml"), "").unwrap();
        assert_eq!(read_dir_or_absent(&at).unwrap().unwrap().count(), 1);
        std::fs::remove_dir_all(&at).unwrap();
        std::fs::write(&at, "").unwrap();
        let err = format!(
            "{:#}",
            read_dir_or_absent(&at).expect_err("a file listed as a directory")
        );
        assert!(
            err.contains("stacks"),
            "the error names the directory: {err}"
        );
    }

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
