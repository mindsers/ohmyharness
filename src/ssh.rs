//! IDE access.
//!
//! The worktree is a plain host directory, so this was never about file access.
//! It is about where the language server runs: dependencies are installed
//! Linux-side while the host is macOS/arm64, so a host LSP means a second
//! dependency tree that silently diverges.
//!
//! The integration point is a **managed SSH config include**, not an IDE plugin.
//! Write one `Host` block and VS Code, Zed, JetBrains Gateway, and plain `ssh`
//! all work without omh knowing they exist — which is the only way a
//! harness-agnostic tool avoids being IDE-locked.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const INCLUDE_LINE: &str = "Include ~/.ssh/config.d/omh";

/// SSH host alias for a session: the session's container name.
///
/// Respelled by [`crate::catalogue::runtime_id`], because a space in a `Host`
/// line is two patterns and in `HostKeyAlias` a parse error — and the block is
/// included from `~/.ssh/config`, so the error stops every ssh on the machine.
pub fn host_alias(repo: &str, session: &str) -> String {
    format!("omh-{}-{session}", crate::catalogue::runtime_id(repo))
}

/// Loopback port for a session's sshd.
///
/// Derived rather than assigned so the alias keeps working across restarts —
/// a port that moved would silently break every IDE bookmark pointing at it.
const PORT_LOW: u16 = 49152;

pub fn port(repo: &str, session: &str) -> u16 {
    // FNV of `repo\0session` into the ephemeral range (below 1024 needs root).
    // Stable across toolchains — it was `DefaultHasher`, which is explicitly
    // not, so a compiler bump moved the port and the saved IDE bookmark
    // stopped resolving.
    let mut bytes = repo.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(session.as_bytes());
    let span = u32::from(u16::MAX - PORT_LOW);
    (u32::from(PORT_LOW) + (crate::hash::fnv1a_64(&bytes) % u64::from(span)) as u32) as u16
}

/// The port a session actually uses, chosen once and remembered.
///
/// `port` gives a starting point; if something already holds it, walk up until
/// free. The choice is written under the run directory so it never moves again
/// — an IDE bookmark points at the alias, which resolves through the port, and
/// a port that wandered between launches would break every saved window.
pub fn assigned_port(
    runs: &Path,
    repo: &str,
    session: &str,
    taken: &dyn Fn(u16) -> bool,
) -> Result<u16> {
    if let Some(recorded) = recorded_port(runs, session) {
        return Ok(recorded);
    }
    let mut candidate = port(repo, session);
    for _ in 0..1024 {
        if !taken(candidate) {
            break;
        }
        candidate = if candidate == u16::MAX {
            PORT_LOW
        } else {
            candidate + 1
        };
    }
    let file = runs.join(session).join("port");
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, candidate.to_string())?;
    Ok(candidate)
}

/// The recorded port for a session, if one was written. Readers that must not
/// probe — `attach` builds a block for a session whose port is in use by the
/// session itself — read this, and fall back to the pure `port` for a session
/// an older omh launched without recording one.
pub fn recorded_port(runs: &Path, session: &str) -> Option<u16> {
    std::fs::read_to_string(runs.join(session).join("port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// A `taken` probe for real: the port is in use if nothing else can bind it.
pub fn port_in_use(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// Where the session finds the host key omh generated: a directory, mounted
/// read-only, holding `ssh_host_ed25519_key` and its `.pub`. The entrypoint
/// installs both into `/etc/ssh` as root, because sshd refuses a key file it
/// does not own and a bind mount lands with the host's ownership.
pub const GUEST_HOST_KEYS: &str = "/omh/hostkeys";

/// The per-repo directory the host key lives in, under `Paths::keys`.
pub fn host_key_dir(keys: &Path) -> PathBuf {
    keys.join("host")
}

/// One `Host` block: the alias, the loopback port, the client key — and the
/// host key pinned.
///
/// Loopback only: on 0.0.0.0 this would publish a shell inside the sandbox to
/// the local network, inverting the point of the project.
///
/// `StrictHostKeyChecking yes` against a known_hosts omh writes, keyed by
/// `HostKeyAlias`, so that what answers on the port has to be the sandbox omh
/// launched. The port is a hash of two public names, and any local process
/// can listen on it; with `StrictHostKeyChecking no` — the previous setting,
/// justified by "host keys are regenerated whenever the image is rebuilt" —
/// whatever did was trusted. The key is omh's now, per repo, and does not
/// change when the image does.
pub fn config_block(alias: &str, port: u16, key: &Path, known_hosts: &Path) -> String {
    format!(
        "Host {alias}\n  \
         HostName 127.0.0.1\n  \
         Port {port}\n  \
         User agent\n  \
         IdentityFile {}\n  \
         IdentitiesOnly yes\n  \
         HostKeyAlias {alias}\n  \
         StrictHostKeyChecking yes\n  \
         UserKnownHostsFile {}\n  \
         LogLevel ERROR\n",
        config_path(key),
        config_path(known_hosts)
    )
}

/// A path as one `ssh_config` argument.
///
/// Quoted, because the paths are under `~/.omh/keys` and a home directory may
/// have a space in it, which ssh reads as a second argument — OpenSSH 10.3
/// then refuses the whole config, and this block is included from
/// `~/.ssh/config`. Inside the quotes ssh's tokeniser takes `\` as an escape,
/// so `\` and `"` are escaped; `%` is doubled because both keywords expand
/// `%` tokens, and a lone one is `unknown key` at the next connection.
fn config_path(path: &Path) -> String {
    let mut out = String::from('"');
    for c in path.display().to_string().chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The known_hosts entry for one alias: `alias type key`, without the comment
/// ssh-keygen appends to a `.pub` — a fourth field is read as part of the key.
pub fn known_hosts_line(alias: &str, pubkey: &str) -> String {
    let mut fields = pubkey.split_whitespace();
    let kind = fields.next().unwrap_or("");
    let key = fields.next().unwrap_or("");
    format!("{alias} {kind} {key}")
}

/// omh's known_hosts, rewritten whole on every attach like the config include:
/// one line per live session, all carrying the repo's one host key.
pub fn write_known_hosts(path: &Path, lines: &[String]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out =
        String::from("# Generated by omh. Do not edit — rewritten on every `omh sNN attach`.\n");
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    crate::shadow::replace_record(path, out)?;
    Ok(())
}

/// Rewrite the managed include. Only omh's file is ever touched.
pub fn write_hosts(path: &Path, blocks: &[String]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out =
        String::from("# Generated by omh. Do not edit — rewritten on every `omh sNN attach`.\n\n");
    for block in blocks {
        out.push_str(block);
        out.push('\n');
    }
    crate::shadow::replace_record(path, out)?;
    Ok(())
}

/// Host blocks for every session under the shared omh root.
///
/// The managed SSH include is global, so rebuilding it from one checkout alone
/// would erase aliases belonging to every other checkout.
pub fn config_blocks(root: &Path) -> Result<Vec<String>> {
    let worktrees = root.join("worktrees");
    let repos = match std::fs::read_dir(&worktrees) {
        Ok(repos) => repos,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", worktrees.display())),
    };
    let mut blocks = Vec::new();
    for repo in repos {
        let repo = repo?;
        if !repo.file_type()?.is_dir() {
            continue;
        }
        let repo_id = repo.file_name().to_string_lossy().into_owned();
        let key = root.join("keys").join(&repo_id).join("id_ed25519");
        let known_hosts = root.join("keys").join(&repo_id).join("known_hosts");
        if !key.is_file() || !known_hosts.is_file() {
            continue;
        }
        let sessions = std::fs::read_dir(repo.path())
            .with_context(|| format!("reading {}", repo.path().display()))?;
        for session in sessions {
            let session = session?;
            if !session.file_type()?.is_dir() {
                continue;
            }
            let id = session.file_name().to_string_lossy().into_owned();
            blocks.push(config_block(
                &host_alias(&repo_id, &id),
                recorded_port(&root.join("run").join(&repo_id), &id)
                    .unwrap_or_else(|| port(&repo_id, &id)),
                &key,
                &known_hosts,
            ));
        }
    }
    blocks.sort();
    Ok(blocks)
}

/// Add the `Include` to `~/.ssh/config` if absent, preserving everything else.
///
/// **As bytes, and only after a read that worked.** This is the one file here
/// that is not omh's, and it was read with `read_to_string(..)
/// .unwrap_or_default()` — so a config omh *could not read* was taken for an
/// empty one, and the write below put one line where the whole file had been.
/// One byte outside UTF-8 is enough to fail that read, and ssh, which reads
/// the file as bytes, never objected to it. Absent is the only failure that
/// means *start from nothing*.
///
/// **Replaced, not written in place.** `fs::write` truncates first, so a full
/// disk or a kill between the two halves leaves a prefix of somebody's ssh
/// config. The new file is written beside the old one and renamed over it —
/// over what a link *names*, not over the link, because `~/.ssh/config` is a
/// link into a dotfiles checkout often enough that replacing it with a plain
/// file is its own way of losing the config. The mode travels with it.
pub fn ensure_include(ssh_config: &Path) -> Result<()> {
    let existing = match std::fs::read(ssh_config) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "omh could not read {}, so it will not rewrite it",
                    ssh_config.display()
                )
            })
        }
    };
    if existing
        .split(|byte| *byte == b'\n')
        .any(|line| line.trim_ascii() == INCLUDE_LINE.as_bytes())
    {
        return Ok(());
    }
    // Where the bytes actually live. A config that is not there yet is its own
    // target; one that is resolves through whatever links lead to it.
    let target = match std::fs::canonicalize(ssh_config) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ssh_config.to_path_buf(),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("resolving where {} lives", ssh_config.display()))
        }
    };
    let parent = target.parent().context("an ssh config has a directory")?;
    std::fs::create_dir_all(parent)?;
    // Prepended: ssh applies the first matching block, so an Include placed
    // after someone's `Host *` would never win.
    let mut out = Vec::from(INCLUDE_LINE.as_bytes());
    out.extend_from_slice(b"\n\n");
    out.extend_from_slice(&existing);

    use std::io::Write as _;
    let mut part = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("writing beside {}", target.display()))?;
    part.write_all(&out)?;
    // A temporary file is `0600`, which is right for a config omh creates and
    // wrong for one it found: the file keeps whatever mode it had.
    if let Ok(meta) = std::fs::metadata(&target) {
        std::fs::set_permissions(part.path(), meta.permissions())?;
    }
    part.persist(&target)
        .map_err(|e| e.error)
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(())
}

pub fn url(alias: &str) -> String {
    format!("ssh://{alias}/work")
}

/// Per-repo key. Generated once, never leaves the host.
pub fn ensure_key(dir: &Path) -> Result<PathBuf> {
    keygen(&dir.join("id_ed25519"), "omh")
}

/// The sandbox's own host key, one per repo, made once. Every session of the
/// checkout presents it; `write_known_hosts` pins it under each session's alias.
pub fn ensure_host_key(keys: &Path) -> Result<PathBuf> {
    keygen(&host_key_dir(keys).join("ssh_host_ed25519_key"), "omh-host")
}

fn keygen(key: &Path, comment: &str) -> Result<PathBuf> {
    if key.exists() {
        return Ok(key.to_path_buf());
    }
    std::fs::create_dir_all(key.parent().unwrap())?;
    let out = std::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", comment, "-f"])
        .arg(key)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "ssh-keygen: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(key.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_unique_per_repo_and_session() {
        assert_ne!(host_alias("a", "s01"), host_alias("b", "s01"));
        assert_ne!(host_alias("a", "s01"), host_alias("a", "s02"));
    }

    /// An IDE bookmark points at the alias, and the alias resolves through the
    /// port. A port that moved between restarts would break every saved window.
    /// Pinned FNV vectors: a toolchain bump must not move a session's port.
    #[test]
    fn ports_are_stable_across_toolchains() {
        assert_eq!(port("repo", "s01"), 60466);
        assert_eq!(port("repo", "s02"), 64997);
        assert_eq!(port("alpha", "s01"), 51718);
    }

    #[test]
    fn a_port_already_taken_is_not_handed_out_twice() {
        let d = tempfile::tempdir().unwrap();
        let base = port("repo", "s01");
        let p = assigned_port(d.path(), "repo", "s01", &|p| p == base).unwrap();
        assert_ne!(p, base, "the base was held, so a free one above it");
        assert_eq!(p, base + 1);
    }

    #[test]
    fn a_recorded_port_is_never_recomputed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("s01")).unwrap();
        std::fs::write(d.path().join("s01/port"), "50000").unwrap();
        assert_eq!(
            assigned_port(d.path(), "repo", "s01", &|_| true).unwrap(),
            50000,
            "the record wins over the hash and the probe"
        );
        assert_eq!(recorded_port(d.path(), "s01"), Some(50000));
        assert_eq!(recorded_port(d.path(), "s02"), None);
    }

    #[test]
    fn ports_differ_between_sessions_and_repos() {
        assert_ne!(port("repo", "s01"), port("repo", "s02"));
        assert_ne!(port("alpha", "s01"), port("beta", "s01"));
    }

    /// Below 1024 needs root; above 65535 does not exist. Staying in the
    /// ephemeral range also avoids stomping on real services.
    #[test]
    fn ports_land_in_the_ephemeral_range() {
        for session in ["s01", "s02", "s99", "doctor"] {
            let p = port("some-repo", session);
            assert!((49152..=65535).contains(&p), "{session} → {p}");
        }
    }

    /// Publishing on 0.0.0.0 would expose a shell inside the sandbox to the
    /// local network, inverting the entire point of the project.
    #[test]
    fn the_host_block_binds_loopback_only() {
        let block = config_block(
            "omh-x-s01",
            49200,
            Path::new("/k/id_ed25519"),
            Path::new("/k/known_hosts"),
        );
        assert!(block.contains("HostName 127.0.0.1"), "got: {block}");
        assert!(!block.contains("0.0.0.0"));
    }

    #[test]
    fn the_host_block_names_the_alias_port_user_and_key() {
        let block = config_block(
            "omh-x-s01",
            49200,
            Path::new("/k/id_ed25519"),
            Path::new("/k/known_hosts"),
        );
        assert!(block.contains("Host omh-x-s01"));
        assert!(block.contains("Port 49200"));
        assert!(block.contains("User agent"));
        assert!(block.contains("/k/id_ed25519"));
    }

    /// Container host keys change on every rebuild; without this every
    /// reconnect stops with a scary mismatch the user cannot act on.
    /// The client trusts the key omh generated for the sandbox and no other.
    ///
    /// This used to say `StrictHostKeyChecking no` with `/dev/null` for a
    /// known_hosts, on the reasoning that the sandbox's host key was
    /// ephemeral. It was, and that was the hole: the port is computable by
    /// any local process — `port()` is a hash of two public names — so
    /// whatever listened on it was trusted with the agent's session. The key
    /// is omh's now, per repo, mounted in and installed by the entrypoint,
    /// and the block pins it through an alias-keyed known_hosts omh writes.
    #[test]
    fn the_host_block_pins_the_key_omh_generated() {
        let block = config_block(
            "omh-x-s01",
            49200,
            Path::new("/k/id_ed25519"),
            Path::new("/k/known_hosts"),
        );
        assert!(block.contains("StrictHostKeyChecking yes"), "got: {block}");
        assert!(block.contains("HostKeyAlias omh-x-s01"), "got: {block}");
        assert!(
            block.contains("UserKnownHostsFile \"/k/known_hosts\""),
            "got: {block}"
        );
        assert!(
            !block.contains("/dev/null"),
            "nothing is thrown away: {block}"
        );
    }

    /// A key path with a space in it is one argument to ssh, and a `%` in it is
    /// a percent sign rather than a token.
    ///
    /// The block reaches ssh through the `Include` in `~/.ssh/config`, so a
    /// line ssh cannot parse is not omh's alone to suffer: OpenSSH 10.3 stops
    /// every connection on the machine — `github.com` included — with
    /// `keyword identityfile extra arguments at end of line … terminating`. A
    /// home directory with a space in it was enough, since the paths are under
    /// `~/.omh/keys`.
    #[test]
    fn a_key_path_with_a_space_or_a_percent_is_one_path_to_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("John Smith 100%/keys");
        let key = keys.join("id_ed25519");
        let known_hosts = keys.join("known_hosts");
        let block = config_block("omh-x-s01", 49200, &key, &known_hosts);
        let cfg = dir.path().join("config");
        std::fs::write(&cfg, &block).unwrap();
        let out = std::process::Command::new("ssh")
            .arg("-G")
            .arg("-F")
            .arg(&cfg)
            .arg("omh-x-s01")
            .output()
            .expect("ssh must be installed to run this test");
        assert!(
            out.status.success(),
            "ssh refused the block: {}\n{block}",
            String::from_utf8_lossy(&out.stderr)
        );
        // `-G` prints each value unquoted and before `%` expansion, which is
        // when `%%` becomes `%` — measured with OpenSSH 10.3.
        let read = String::from_utf8_lossy(&out.stdout);
        for (keyword, path) in [("identityfile", &key), ("userknownhostsfile", &known_hosts)] {
            let want = format!("{keyword} {}", path.display());
            assert!(
                read.lines().any(|l| l.replace("%%", "%") == want),
                "ssh did not read `{want}` back:\n{read}"
            );
        }
    }

    /// One line per alias, carrying the key and not the comment ssh-keygen
    /// appends — a known_hosts entry is `host type key`, and a fourth field is
    /// read as part of the key.
    #[test]
    fn the_known_hosts_line_names_the_alias_and_carries_the_public_key() {
        let line = known_hosts_line(
            "omh-x-s01",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample omh-host\n",
        );
        assert_eq!(
            line,
            "omh-x-s01 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample"
        );
    }

    /// The host key is made once per repo and then reused: every session of a
    /// checkout presents the same key, under its own alias.
    #[test]
    fn a_host_key_is_generated_once_per_repo() {
        let d = tempfile::tempdir().unwrap();
        let first = ensure_host_key(d.path()).unwrap();
        assert!(first.exists(), "private key at {}", first.display());
        assert!(first.with_extension("pub").exists(), "with its public half");
        let before = std::fs::read(&first).unwrap();
        let second = ensure_host_key(d.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(before, std::fs::read(&second).unwrap(), "not regenerated");
    }

    /// The known_hosts file is omh's, rewritten whole like the config include.
    #[test]
    fn known_hosts_is_replaced_wholesale() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("known_hosts");
        write_known_hosts(&f, &["omh-x-s01 ssh-ed25519 AAA".into()]).unwrap();
        write_known_hosts(&f, &["omh-x-s02 ssh-ed25519 AAA".into()]).unwrap();
        let body = std::fs::read_to_string(&f).unwrap();
        assert!(
            body.contains("omh-x-s02") && !body.contains("omh-x-s01"),
            "{body}"
        );
    }

    // ── the managed include ─────────────────────────────────────────────────

    #[test]
    fn the_managed_file_is_replaced_wholesale() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.d/omh");
        write_hosts(&f, &["Host a\n".into()]).unwrap();
        write_hosts(&f, &["Host b\n".into()]).unwrap();
        let body = std::fs::read_to_string(&f).unwrap();
        assert!(body.contains("Host b"));
        assert!(!body.contains("Host a"), "stale sessions must not linger");
    }

    #[test]
    fn managed_blocks_include_sessions_from_every_repository() {
        let d = tempfile::tempdir().unwrap();
        for repo in ["one-11111111", "two-22222222"] {
            std::fs::create_dir_all(d.path().join("worktrees").join(repo).join("s01")).unwrap();
            std::fs::create_dir_all(d.path().join("keys").join(repo)).unwrap();
            std::fs::write(d.path().join("keys").join(repo).join("id_ed25519"), "key").unwrap();
            std::fs::write(d.path().join("keys").join(repo).join("known_hosts"), "host").unwrap();
        }

        let blocks = config_blocks(d.path()).unwrap().join("\n");

        assert!(blocks.contains("Host omh-one-11111111-s01"), "{blocks}");
        assert!(blocks.contains("Host omh-two-22222222-s01"), "{blocks}");
    }

    #[test]
    fn the_managed_file_says_it_is_generated() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("omh");
        write_hosts(&f, &[]).unwrap();
        let body = std::fs::read_to_string(&f).unwrap();
        assert!(body.to_lowercase().contains("generated"), "got: {body}");
    }

    /// Someone's `~/.ssh/config` is not ours to rewrite. Append the one line and
    /// touch nothing else.
    #[test]
    fn adding_the_include_preserves_the_users_config() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("config");
        std::fs::write(&cfg, "Host work\n  User me\n").unwrap();

        ensure_include(&cfg).unwrap();

        let body = std::fs::read_to_string(&cfg).unwrap();
        assert!(body.contains("Host work"), "existing config survived");
        assert!(body.contains("User me"));
        assert!(body.contains(INCLUDE_LINE));
    }

    #[test]
    fn adding_the_include_twice_does_not_duplicate_it() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("config");
        ensure_include(&cfg).unwrap();
        ensure_include(&cfg).unwrap();
        let body = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(body.matches(INCLUDE_LINE).count(), 1);
    }

    /// `Include` must come before any `Host` block, or ssh applies the earlier
    /// block's settings to our alias.
    #[test]
    fn the_include_goes_at_the_top() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("config");
        std::fs::write(&cfg, "Host work\n  User me\n").unwrap();
        ensure_include(&cfg).unwrap();
        let body = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            body.find(INCLUDE_LINE) < body.find("Host work"),
            "Include must precede Host blocks:\n{body}"
        );
    }

    /// A config that is not UTF-8 keeps every byte it had.
    ///
    /// `ensure_include` read the file as a string and took *could not* for
    /// *is empty*: `read_to_string(..).unwrap_or_default()`, then a write of
    /// the one line omh adds onto that. A single byte outside UTF-8 — an
    /// accented name in a comment, saved in Latin-1 years ago — fails the
    /// read, and the user's whole ssh config was replaced by an `Include`.
    /// ssh itself reads that file as bytes and never minded.
    #[test]
    fn a_config_that_is_not_utf8_keeps_every_byte() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("config");
        let theirs: &[u8] = b"# r\xe9seau du bureau\nHost work\n  User me\n";
        std::fs::write(&cfg, theirs).unwrap();

        ensure_include(&cfg).unwrap();

        let now = std::fs::read(&cfg).unwrap();
        assert!(
            now.ends_with(theirs),
            "the user's config is gone: {:?}",
            String::from_utf8_lossy(&now)
        );
        assert!(
            now.starts_with(INCLUDE_LINE.as_bytes()),
            "and the line omh adds is still first"
        );
        // …and a second attach finds its own line in a file it cannot decode.
        ensure_include(&cfg).unwrap();
        assert_eq!(std::fs::read(&cfg).unwrap(), now, "added once, not twice");
    }

    /// A config reached through a link stays a link, with the mode it had.
    ///
    /// The other half of not writing in place. Replacing the file by rename
    /// is what keeps a crash from leaving half a config — and a rename onto
    /// `~/.ssh/config` replaces the *link* a dotfiles checkout put there,
    /// leaving the checkout's copy stale and a plain file where the link was.
    /// So the rename lands on what the link names.
    #[test]
    #[cfg(unix)]
    fn a_config_reached_through_a_link_stays_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("dotfiles").join("ssh-config");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "Host work\n  User me\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let cfg = d.path().join("config");
        std::os::unix::fs::symlink(&real, &cfg).unwrap();

        ensure_include(&cfg).unwrap();

        assert!(
            cfg.symlink_metadata().unwrap().file_type().is_symlink(),
            "the link was replaced by a file"
        );
        let body = std::fs::read_to_string(&real).unwrap();
        assert!(
            body.starts_with(INCLUDE_LINE) && body.ends_with("Host work\n  User me\n"),
            "and the file it names is the one that changed: {body}"
        );
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o640,
            "at the mode it had"
        );
    }

    /// And one omh could not read at all is left alone, with the reason said.
    #[test]
    fn a_config_omh_could_not_read_is_refused_rather_than_rewritten() {
        let d = tempfile::tempdir().unwrap();
        // A directory where the file should be: every read fails, and not
        // with *not found*.
        let cfg = d.path().join("config");
        std::fs::create_dir_all(cfg.join("something-of-theirs")).unwrap();

        let err = ensure_include(&cfg).expect_err("a config omh could not read");

        let err = format!("{err:#}");
        assert!(
            err.contains("could not read") && err.contains(&cfg.display().to_string()),
            "it says what it could not do, and to which file: {err}"
        );
        assert!(cfg.join("something-of-theirs").exists());
    }
}
