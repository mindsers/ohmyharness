//! A directory the sandbox can write, read and written from the host without
//! following anything in it.
//!
//! Several directories on the host are mounted writable into a sandbox — the
//! note store and the transcripts among them — and omh reads and writes in
//! them while the agent is running. A path into one of them is
//! a question whose answer can change between two askings: check that
//! `store/ns` is a directory, and by the time `store/ns/note.md` is written
//! `ns` can be a link to anywhere the user can write. Every fix that *looked
//! first* left that window, and it was reported against each of them.
//!
//! So nothing here is named twice. A directory is opened once, without
//! following, and everything after is done **through the descriptor**:
//! `openat` and `renameat` resolve against the directory that was opened,
//! wherever the agent has since moved it, and each further step is one name,
//! opened the same way. There is no check to race because there is no second
//! look.
//!
//! Unix only, like the mounts it exists for.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

/// A directory, held open.
pub(crate) struct Dir {
    fd: File,
}

/// One step, never a route: a name that is empty, `.` or `..`, or that has a
/// separator or a NUL in it, would walk — and names come from keys and
/// directory listings, which is to say from the agent.
fn step(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a single name"),
        ));
    }
    CString::new(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// What has a name in a held directory, as [`Dir::entry`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Absent,
    File,
    Dir,
    /// A link, and where it points — read, not followed.
    Link(std::path::PathBuf),
    /// A pipe, a socket, a device.
    Other,
}

/// The temporary name something is laid at before it is renamed over `name`.
fn part_of(name: &CString) -> io::Result<CString> {
    let mut part = name.as_bytes().to_vec();
    part.splice(0..0, *b".");
    part.extend_from_slice(format!(".{}.omh-part", std::process::id()).as_bytes());
    CString::new(part).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// `openat`, with the result owned or the reason kept.
fn open_under(dir: &File, name: &CString, flags: libc::c_int) -> io::Result<File> {
    open_under_mode(dir, name, flags, 0o666)
}

/// [`open_under`], creating with `mode` rather than what `fs::write` asks for.
fn open_under_mode(
    dir: &File,
    name: &CString,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> io::Result<File> {
    // SAFETY: `dir` is an open descriptor that outlives the call, and `name`
    // is a NUL-terminated string that does too. The mode is read only when
    // `O_CREAT` is among the flags, and is what `fs::write` would ask for.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by `openat` and nothing else has it.
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Dir {
    /// Hold `path`, which has to be a directory and not a link to one.
    ///
    /// The one place a path is used. What it names is omh's own — the source
    /// of a mount, which the sandbox cannot rename — so this is where holding
    /// starts and not something to do again further down.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let fd = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        Ok(Self { fd })
    }

    /// A directory inside this one. Never a link to one.
    pub(crate) fn sub(&self, name: &OsStr) -> io::Result<Dir> {
        let fd = open_under(&self.fd, &step(name)?, libc::O_RDONLY | libc::O_DIRECTORY)?;
        Ok(Dir { fd })
    }

    /// The same, made first if it is not there.
    ///
    /// *Is not there* is `mkdirat`'s to decide, in the one call: it makes the
    /// directory or says something already has the name. What has it is then
    /// opened like any other, so a link that got there first is refused
    /// rather than created around.
    pub(crate) fn sub_or_create(&self, name: &OsStr) -> io::Result<Dir> {
        let step = step(name)?;
        // SAFETY: as `open_under` — an open descriptor and a C string, both
        // outliving the call.
        let made = unsafe { libc::mkdirat(self.fd.as_raw_fd(), step.as_ptr(), 0o777) };
        if made != 0 {
            let why = io::Error::last_os_error();
            if why.kind() != io::ErrorKind::AlreadyExists {
                return Err(why);
            }
        }
        self.sub(name)
    }

    /// A regular file inside this one, open for reading.
    ///
    /// Without following and without waiting — a pipe opened for reading
    /// blocks until something writes to it, and the only thing that could is
    /// the agent — and then the thing that was opened is asked what it is.
    pub(crate) fn file(&self, name: &OsStr) -> io::Result<File> {
        let file = open_under(&self.fd, &step(name)?, libc::O_RDONLY | libc::O_NONBLOCK)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name:?} is not a regular file"),
            ));
        }
        Ok(file)
    }

    /// Make `name` with `body`, only if nothing has that name — not a file,
    /// not a link, dangling or not.
    pub(crate) fn create_new(&self, name: &OsStr, body: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        let mut file = open_under(
            &self.fd,
            &step(name)?,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        file.write_all(body)
    }

    /// Put `body` at `name`, replacing whatever has that name.
    ///
    /// Written beside and renamed over. A rename replaces a link instead of
    /// following it, and never leaves a prefix of the new content behind. The
    /// file is created the way `fs::write` creates one, so the umask decides
    /// who can read it — these are read inside the sandbox, by a different
    /// uid on some hosts, and a temporary file's `0600` is a file the agent
    /// cannot open.
    pub(crate) fn replace(&self, name: &OsStr, body: &[u8]) -> io::Result<()> {
        self.replace_mode(name, body, 0o666)
    }

    /// [`Dir::replace`], for a file only the user may read — one holding a
    /// token. Created `0600`, so no moment exists where the umask lets anyone
    /// else open it, before or after the rename.
    pub(crate) fn replace_private(&self, name: &OsStr, body: &[u8]) -> io::Result<()> {
        self.replace_mode(name, body, 0o600)
    }

    /// What has `name` here, looked at without following it — a link is
    /// itself, and where it points is read, never visited.
    pub(crate) fn entry(&self, name: &OsStr) -> io::Result<Entry> {
        let name = step(name)?;
        // SAFETY: `stat` is plain data, written whole by a successful call.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: as `open_under` — an open descriptor and a C string, both
        // outliving the call — and `st` is ours to write.
        let r = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if r != 0 {
            let why = io::Error::last_os_error();
            return match why.kind() {
                io::ErrorKind::NotFound => Ok(Entry::Absent),
                _ => Err(why),
            };
        }
        Ok(match st.st_mode & libc::S_IFMT {
            libc::S_IFREG => Entry::File,
            libc::S_IFDIR => Entry::Dir,
            libc::S_IFLNK => {
                let mut buf = vec![0u8; libc::PATH_MAX as usize];
                // SAFETY: as above, with `buf` ours and its length the bound.
                let n = unsafe {
                    libc::readlinkat(
                        self.fd.as_raw_fd(),
                        name.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n < 0 {
                    return Err(io::Error::last_os_error());
                }
                buf.truncate(n as usize);
                Entry::Link(std::ffi::OsString::from_vec(buf).into())
            }
            _ => Entry::Other,
        })
    }

    /// Make `name` a link to `target`, replacing the file or link that has
    /// the name: laid beside and renamed over, so the name is never missing
    /// and never anything else. A directory there is refused — a rename does
    /// not replace one with a link — and left as it is.
    pub(crate) fn link_over(&self, name: &OsStr, target: &Path) -> io::Result<()> {
        let name = step(name)?;
        let part = part_of(&name)?;
        let target = CString::new(target.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let dir = self.fd.as_raw_fd();
        // SAFETY, for the calls below: `dir` is this struct's own open
        // descriptor, and the three names are C strings that outlive them.
        unsafe { libc::unlinkat(dir, part.as_ptr(), 0) };
        if unsafe { libc::symlinkat(target.as_ptr(), dir, part.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        match unsafe { libc::renameat(dir, part.as_ptr(), dir, name.as_ptr()) } {
            0 => Ok(()),
            _ => {
                let why = io::Error::last_os_error();
                unsafe { libc::unlinkat(dir, part.as_ptr(), 0) };
                Err(why)
            }
        }
    }

    fn replace_mode(&self, name: &OsStr, body: &[u8], mode: libc::c_uint) -> io::Result<()> {
        use std::io::Write as _;
        let name = step(name)?;
        let part = part_of(&name)?;
        let dir = self.fd.as_raw_fd();

        // SAFETY, for the three calls below: `dir` is this struct's own open
        // descriptor, and both names are C strings that outlive the calls.
        //
        // Whatever a run that died left at the temporary name goes first;
        // `unlinkat` removes a link rather than what it points at.
        unsafe { libc::unlinkat(dir, part.as_ptr(), 0) };
        let mut file = open_under_mode(
            &self.fd,
            &part,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            mode,
        )?;
        let written = file.write_all(body);
        drop(file);
        let placed = written.and_then(|()| {
            match unsafe { libc::renameat(dir, part.as_ptr(), dir, name.as_ptr()) } {
                0 => Ok(()),
                _ => Err(io::Error::last_os_error()),
            }
        });
        if placed.is_err() {
            unsafe { libc::unlinkat(dir, part.as_ptr(), 0) };
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::os::unix::fs::symlink;

    fn name(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    /// What has a name is said without following it: a link is a link, to
    /// wherever, dangling or not.
    #[test]
    fn an_entry_is_what_has_the_name_not_what_it_points_at() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("file"), "x").unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        symlink(d.path().join("file"), d.path().join("to-file")).unwrap();
        symlink("/omh/prefs/config.toml", d.path().join("dangling")).unwrap();
        let held = Dir::open(d.path()).unwrap();
        assert_eq!(held.entry(name("absent")).unwrap(), Entry::Absent);
        assert_eq!(held.entry(name("file")).unwrap(), Entry::File);
        assert_eq!(held.entry(name("dir")).unwrap(), Entry::Dir);
        assert_eq!(
            held.entry(name("to-file")).unwrap(),
            Entry::Link(d.path().join("file"))
        );
        assert_eq!(
            held.entry(name("dangling")).unwrap(),
            Entry::Link("/omh/prefs/config.toml".into())
        );
        assert!(held.entry(name("../x")).is_err(), "a route is not a name");
    }

    /// A link laid over a file or over another link replaces it whole, in
    /// one rename, and points exactly where it was asked to; a directory with
    /// the name is refused rather than laid beside or into.
    #[test]
    fn a_link_replaces_what_has_its_name_and_never_a_directory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("config.toml"), "planted").unwrap();
        symlink(d.path().join("elsewhere"), d.path().join("wrong")).unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        let held = Dir::open(d.path()).unwrap();
        let target = Path::new("/omh/prefs/config.toml");
        for at in ["config.toml", "wrong", "new"] {
            held.link_over(name(at), target).unwrap();
            assert_eq!(
                std::fs::read_link(d.path().join(at)).unwrap(),
                target,
                "{at}"
            );
        }
        assert!(held.link_over(name("dir"), target).is_err());
        assert!(d.path().join("dir").is_dir());
        let left: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("omh-part"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// A private file is the user's alone from its first byte, and stays so
    /// when it replaces one that was not.
    #[test]
    fn a_private_replacement_is_the_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("token"), "old").unwrap();
        std::fs::set_permissions(
            d.path().join("token"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let held = Dir::open(d.path()).unwrap();
        held.replace_private(name("token"), b"secret").unwrap();
        let mode = std::fs::metadata(d.path().join("token"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read(d.path().join("token")).unwrap(), b"secret");
    }

    /// The whole point, stated once: what was opened is what is used.
    ///
    /// A path is a question that can change its answer between two askings,
    /// and everything under one of these directories is the agent's to
    /// rename while the host is working. So the agent's move is played in
    /// the window — after the directory is held, before it is used — for
    /// each thing a holder can do.
    #[test]
    fn a_directory_swapped_for_a_link_after_it_was_held_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(store.join("ns")).unwrap();
        std::fs::create_dir_all(elsewhere.join("deeper")).unwrap();
        std::fs::write(elsewhere.join("theirs.jsonl"), "the user's own\n").unwrap();

        let ns = Dir::open(&store).unwrap().sub(name("ns")).unwrap();
        // The agent, in the window: the real directory aside, a link in its
        // place.
        std::fs::rename(store.join("ns"), store.join("ns.aside")).unwrap();
        symlink(&elsewhere, store.join("ns")).unwrap();

        ns.replace(name("note.md"), b"a note\n").unwrap();
        assert!(
            !elsewhere.join("note.md").exists(),
            "a write followed a link put there after the directory was held"
        );
        assert_eq!(
            std::fs::read_to_string(store.join("ns.aside/note.md")).unwrap(),
            "a note\n",
            "it landed in the directory that was held, wherever that now is"
        );
        assert!(
            ns.file(name("theirs.jsonl")).is_err(),
            "a read followed it too"
        );
        assert!(ns.sub(name("deeper")).is_err(), "and so did a descent");
    }

    /// And a link is never held in the first place, at any step.
    #[test]
    fn nothing_is_reached_through_a_link() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("a-host-file"), "the user's own\n").unwrap();
        symlink(&elsewhere, store.join("linked-dir")).unwrap();
        symlink(elsewhere.join("a-host-file"), store.join("linked.jsonl")).unwrap();
        symlink(elsewhere.join("not-there-yet"), store.join("dangling.md")).unwrap();
        symlink(&store, d.path().join("store-by-another-name")).unwrap();

        assert!(
            Dir::open(&d.path().join("store-by-another-name")).is_err(),
            "the root itself"
        );
        let held = Dir::open(&store).unwrap();
        assert!(held.sub(name("linked-dir")).is_err(), "a directory");
        assert!(
            held.sub_or_create(name("linked-dir")).is_err(),
            "one that already exists as a link is not *created* around"
        );
        assert!(held.file(name("linked.jsonl")).is_err(), "a file");
        // A pipe is not a link, and is the other thing a name can be that a
        // read must not open: it would wait for a writer who need not come.
        let piped = std::process::Command::new("mkfifo")
            .arg(store.join("pipe.jsonl"))
            .status()
            .unwrap();
        assert!(piped.success(), "the fixture needs a pipe");
        assert!(
            held.file(name("pipe.jsonl")).is_err(),
            "a pipe, and at once"
        );

        // A write over a link replaces the link. The file it named is not
        // touched, and one it named that was not there is not made.
        held.replace(name("linked.jsonl"), b"omh's\n").unwrap();
        held.replace(name("dangling.md"), b"omh's\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("a-host-file")).unwrap(),
            "the user's own\n"
        );
        assert!(!elsewhere.join("not-there-yet").exists());
        assert_eq!(
            std::fs::read_to_string(store.join("dangling.md")).unwrap(),
            "omh's\n"
        );
    }

    /// A name is one step, never a route.
    ///
    /// Everything above is undone by a name that walks: `..`, or one with a
    /// separator in it. They come from keys and directory listings, which is
    /// to say from the agent.
    #[test]
    fn a_name_that_is_a_path_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        std::fs::create_dir_all(store.join("ns")).unwrap();
        std::fs::write(d.path().join("outside.md"), "the user's own\n").unwrap();
        let held = Dir::open(&store).unwrap();

        for walking in ["..", ".", "", "ns/../..", "../outside.md", "/etc"] {
            assert!(held.sub(name(walking)).is_err(), "sub({walking:?})");
            assert!(
                held.sub_or_create(name(walking)).is_err(),
                "sub_or_create({walking:?})"
            );
            assert!(held.file(name(walking)).is_err(), "file({walking:?})");
            assert!(
                held.replace(name(walking), b"x").is_err(),
                "replace({walking:?})"
            );
        }
        assert_eq!(
            std::fs::read_to_string(d.path().join("outside.md")).unwrap(),
            "the user's own\n"
        );
    }

    /// What it is for, working: a file is read, a file is written at the mode
    /// an ordinary write gives one, a directory is made once and found again.
    #[test]
    fn the_ordinary_case_is_ordinary() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let held = Dir::open(&store).unwrap();

        let ns = held.sub_or_create(name("ns")).unwrap();
        held.sub_or_create(name("ns"))
            .expect("a second time finds it");
        ns.replace(name("note.md"), b"one\n").unwrap();
        ns.replace(name("note.md"), b"two\n").unwrap();
        let mut text = String::new();
        ns.file(name("note.md"))
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "two\n");

        std::fs::write(store.join("plain"), "x").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode(&store.join("ns/note.md")),
            mode(&store.join("plain")),
            "readable by whoever an ordinary file is readable by"
        );
        assert!(
            std::fs::read_dir(store.join("ns")).unwrap().count() == 1,
            "and no temporary file is left beside it"
        );
        assert!(held.file(name("ns")).is_err(), "a directory is not a file");
        assert!(ns.sub(name("note.md")).is_err(), "nor a file a directory");
    }
}
