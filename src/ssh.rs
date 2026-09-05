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

use anyhow::Result;
use std::path::{Path, PathBuf};

pub const INCLUDE_LINE: &str = "Include ~/.ssh/config.d/omh";

/// SSH host alias for a session.
pub fn host_alias(repo: &str, session: &str) -> String {
    format!("omh-{repo}-{session}")
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
        key.display(),
        known_hosts.display()
    )
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
        String::from("# Generated by omh. Do not edit — rewritten on every `omh s attach`.\n");
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Rewrite the managed include. Only omh's file is ever touched.
pub fn write_hosts(path: &Path, blocks: &[String]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out =
        String::from("# Generated by omh. Do not edit — rewritten on every `omh s attach`.\n\n");
    for block in blocks {
        out.push_str(block);
        out.push('\n');
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Add the `Include` to `~/.ssh/config` if absent, preserving everything else.
pub fn ensure_include(ssh_config: &Path) -> Result<()> {
    let existing = std::fs::read_to_string(ssh_config).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == INCLUDE_LINE) {
        return Ok(());
    }
    std::fs::create_dir_all(ssh_config.parent().unwrap())?;
    // Prepended: ssh applies the first matching block, so an Include placed
    // after someone's `Host *` would never win.
    let mut out = String::from(INCLUDE_LINE);
    out.push_str("\n\n");
    out.push_str(&existing);
    std::fs::write(ssh_config, out)?;
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
            block.contains("UserKnownHostsFile /k/known_hosts"),
            "got: {block}"
        );
        assert!(
            !block.contains("/dev/null"),
            "nothing is thrown away: {block}"
        );
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
}
