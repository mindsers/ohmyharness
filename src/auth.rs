//! Credentials, and having more than one of them.
//!
//! An account is a captured snapshot of a harness's own credential files. It is
//! keyed by *harness* rather than by provider, because a harness is what we can
//! actually capture — two harnesses talking to the same provider still each
//! need their own login.
//!
//! Which account a session uses is a project-level setting, because that is how
//! it actually varies: this repo is work, that one is personal.

use crate::adapter::Adapter;
use crate::profile::Paths;
use anyhow::Result;
use std::path::PathBuf;

/// Where a credential file lives on the host and inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredMount {
    pub host: PathBuf,
    pub guest: PathBuf,
}

pub const DEFAULT_ACCOUNT: &str = "default";

pub fn dir(paths: &Paths, harness: &str, account: &str) -> PathBuf {
    paths.creds(harness).join(account)
}

/// Accounts captured for a harness, in name order.
pub fn accounts(paths: &Paths, harness: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(paths.creds(harness)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_captured(paths, harness, name))
        .collect();
    out.sort();
    out
}

/// Captured means credentials are actually present. An empty directory left by
/// an interrupted login must never look like a successful one.
pub fn is_captured(paths: &Paths, harness: &str, account: &str) -> bool {
    // Non-empty specifically: `prepare` leaves empty placeholders so bind
    // mounts land as files, and a placeholder is not a login.
    fn has_content(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                has_content(&p)
            } else {
                std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false)
            }
        })
    }
    has_content(&dir(paths, harness, account))
}

/// Which account to use. Ambiguity is an error, never a guess — silently
/// picking the wrong identity is worse than stopping.
pub fn resolve(
    paths: &Paths,
    harness: &str,
    explicit: Option<&str>,
    configured: Option<&str>,
) -> Result<String> {
    let available = accounts(paths, harness);
    if available.is_empty() {
        anyhow::bail!("no account for {harness} — run `omh auth {harness}` first");
    }

    if let Some(name) = explicit.or(configured) {
        if !available.iter().any(|a| a == name) {
            anyhow::bail!(
                "no account `{name}` for {harness}\n  captured: {}",
                available.join(", ")
            );
        }
        return Ok(name.to_string());
    }

    match available.len() {
        1 => Ok(available[0].clone()),
        // Guessing here would send work traffic through a personal account and
        // never say so.
        _ => anyhow::bail!(
            "{harness} has several accounts: {}\n  \
             pick one with `omh config set account <name>` or `-a <name>`",
            available.join(", ")
        ),
    }
}

/// Where an account's files mount inside the sandbox.
///
/// Deliberately **writable**: OAuth tokens are refreshed in place, and a
/// read-only mount would make every session start by failing to persist a new
/// token. The workspace invariant is about the repo, not about omh's own state.
pub fn mounts(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    guest_home: &str,
) -> Vec<CredMount> {
    adapter
        .creds
        .iter()
        .map(|template| {
            let guest = crate::adapter::expand(template, guest_home);
            // Storage mirrors the guest path, so the account directory is
            // legible rather than a pile of mangled names.
            let relative = template.trim_start_matches("$HOME/");
            CredMount { host: account_dir.join(relative), guest }
        })
        .collect()
}

/// Create the files a bind mount needs to land as files.
///
/// Docker turns a mount of a non-existent host path into a *directory*, so a
/// first login would write its token into a folder the harness cannot read.
/// Existing credentials are never touched.
pub fn prepare(adapter: &Adapter, account_dir: &std::path::Path, guest_home: &str) -> Result<()> {
    for cred in mounts(adapter, account_dir, guest_home) {
        if let Some(parent) = cred.host.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !cred.host.exists() {
            std::fs::write(&cred.host, "")?;
        }
    }
    Ok(())
}

/// Resolve for an actual launch.
///
/// Not being logged in yet is fine — you should be able to run a harness and
/// let it prompt. But an account you *named* and do not have is a mistake worth
/// stopping for, and so is having two identities with no stated preference.
pub fn resolve_for_launch(
    paths: &Paths,
    harness: &str,
    explicit: Option<&str>,
    configured: Option<&str>,
) -> Result<Option<String>> {
    let asked = explicit.or(configured).is_some();
    if !asked && accounts(paths, harness).is_empty() {
        return Ok(None);
    }
    resolve(paths, harness, explicit, configured).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths { root: d.path().join("home"), repo: d.path().join("repo") };
        (d, paths)
    }

    fn capture(paths: &Paths, harness: &str, account: &str) {
        let p = dir(paths, harness, account).join(".claude/.credentials.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "{\"token\":\"x\"}").unwrap();
    }

    fn claude() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    // ── listing ─────────────────────────────────────────────────────────────

    #[test]
    fn there_are_no_accounts_before_any_login() {
        let (_d, paths) = fixture();
        assert!(accounts(&paths, "claude").is_empty());
    }

    #[test]
    fn accounts_are_listed_by_name() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(accounts(&paths, "claude"), vec!["personal", "work"]);
    }

    #[test]
    fn accounts_are_kept_apart_per_harness() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        assert!(accounts(&paths, "opencode").is_empty());
    }

    /// Regression in spirit: an interrupted `omh auth` used to leave a directory
    /// behind that reported as authed.
    #[test]
    fn an_empty_account_directory_is_not_captured() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(dir(&paths, "claude", "work")).unwrap();
        assert!(!is_captured(&paths, "claude", "work"));
        assert!(accounts(&paths, "claude").is_empty(), "and it is not listed");
    }

    // ── choosing ────────────────────────────────────────────────────────────

    #[test]
    fn an_explicit_account_wins() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(resolve(&paths, "claude", Some("work"), Some("personal")).unwrap(), "work");
    }

    #[test]
    fn the_configured_account_is_used_when_there_are_several() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(resolve(&paths, "claude", None, Some("personal")).unwrap(), "personal");
    }

    #[test]
    fn a_single_account_needs_no_choosing() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(resolve(&paths, "claude", None, None).unwrap(), "personal");
    }

    /// Two identities and no stated preference is exactly when guessing is
    /// most expensive — you would send work traffic through a personal account
    /// and never notice.
    #[test]
    fn several_accounts_with_no_preference_is_an_error_that_lists_them() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        let err = resolve(&paths, "claude", None, None).unwrap_err().to_string();
        assert!(err.contains("work") && err.contains("personal"), "got: {err}");
        assert!(err.contains("omh config set account"), "must say how to fix it: {err}");
    }

    #[test]
    fn no_accounts_at_all_points_at_omh_auth() {
        let (_d, paths) = fixture();
        let err = resolve(&paths, "claude", None, None).unwrap_err().to_string();
        assert!(err.contains("omh auth claude"), "got: {err}");
    }

    #[test]
    fn naming_an_account_that_was_never_captured_is_an_error() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        let err = resolve(&paths, "claude", Some("nope"), None).unwrap_err().to_string();
        assert!(err.contains("nope"), "got: {err}");
    }

    // ── mounting ────────────────────────────────────────────────────────────

    #[test]
    fn credentials_mount_where_the_harness_actually_looks() {
        let account = PathBuf::from("/host/creds/claude/work");
        let m = mounts(&claude(), &account, "/home/agent");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].guest, Path::new("/home/agent/.claude/.credentials.json"));
    }

    /// Storage mirrors the guest path, so `ls ~/.omh/creds/claude/work` is
    /// legible instead of a directory of mangled names.
    #[test]
    fn stored_credentials_mirror_the_path_they_came_from() {
        let account = PathBuf::from("/host/creds/claude/work");
        let m = mounts(&claude(), &account, "/home/agent");
        assert_eq!(m[0].host, account.join(".claude/.credentials.json"));
    }

    #[test]
    fn an_adapter_with_no_credentials_mounts_nothing() {
        let bare: Adapter = toml::from_str(
            r#"
            name = "bare"
            bin = "bare"
            install = "x"
            [capabilities.rules]
            path = "/work/AGENTS.md"
            render = "concat"
            "#,
        )
        .unwrap();
        assert!(mounts(&bare, Path::new("/x"), "/home/agent").is_empty());
    }

    // ── preparing for a first login ─────────────────────────────────────────

    #[test]
    fn preparing_creates_the_files_a_bind_mount_needs() {
        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("work");
        prepare(&claude(), &account, "/home/agent").unwrap();

        let f = account.join(".claude/.credentials.json");
        assert!(f.exists(), "missing {f:?}");
        assert!(f.is_file(), "must be a file, or docker mounts a directory over it");
    }

    #[test]
    fn preparing_never_clobbers_an_existing_login() {
        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("work");
        let f = account.join(".claude/.credentials.json");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "{\"token\":\"keep-me\"}").unwrap();

        prepare(&claude(), &account, "/home/agent").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{\"token\":\"keep-me\"}");
    }

    #[test]
    fn preparing_alone_does_not_count_as_captured() {
        let (_d, paths) = fixture();
        prepare(&claude(), &dir(&paths, "claude", "work"), "/home/agent").unwrap();
        assert!(
            !is_captured(&paths, "claude", "work"),
            "empty placeholder files are not a login"
        );
    }

    // ── resolving at launch ─────────────────────────────────────────────────

    /// You must be able to run a harness before you have ever logged in — the
    /// harness itself will prompt.
    #[test]
    fn launching_without_any_account_is_allowed() {
        let (_d, paths) = fixture();
        assert_eq!(resolve_for_launch(&paths, "claude", None, None).unwrap(), None);
    }

    /// Regression: `-a work` for an account that does not exist silently ran
    /// with no credentials at all, so the session started logged out and said
    /// nothing about why.
    #[test]
    fn naming_a_missing_account_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        let err = resolve_for_launch(&paths, "claude", Some("work"), None).unwrap_err();
        assert!(err.to_string().contains("work"), "got: {err}");
    }

    #[test]
    fn a_configured_account_that_is_missing_also_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert!(resolve_for_launch(&paths, "claude", None, Some("work")).is_err());
    }

    #[test]
    fn the_only_account_is_used_at_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve_for_launch(&paths, "claude", None, None).unwrap().as_deref(),
            Some("personal")
        );
    }

    #[test]
    fn two_identities_and_no_preference_still_stops() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert!(resolve_for_launch(&paths, "claude", None, None).is_err());
    }
}
