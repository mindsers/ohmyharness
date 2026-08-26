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
    /// A single file rather than a directory. Docker needs to know, and a
    /// directory mounted as a file (or vice versa) fails in confusing ways.
    pub file: bool,
}

pub const DEFAULT_ACCOUNT: &str = "default";

/// Home inside the sandbox. Re-exported so callers here need not reach into
/// `image`, but there is exactly one definition.
pub use crate::image::GUEST_HOME;

pub fn dir(paths: &Paths, harness: &str, account: &str) -> PathBuf {
    paths.creds(harness).join(account)
}

/// Accounts captured for a harness, in name order.
pub fn accounts(paths: &Paths, adapter: &Adapter) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(paths.creds(&adapter.name)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_captured(paths, adapter, name))
        .collect();
    out.sort();
    out
}

/// Captured means credentials are actually present. An empty directory left by
/// an interrupted login must never look like a successful one.
pub fn is_captured(paths: &Paths, adapter: &Adapter, account: &str) -> bool {
    // Defined in terms of `unfilled` so the two answers can never disagree —
    // they did, and `omh auth` failed while `omh info` listed the account.
    unfilled(adapter, &dir(paths, &adapter.name, account), GUEST_HOME).is_empty()
}

/// Which account to use. Ambiguity is an error, never a guess — silently
/// picking the wrong identity is worse than stopping.
pub fn resolve(
    paths: &Paths,
    adapter: &Adapter,
    explicit: Option<&str>,
    configured: Option<&str>,
) -> Result<String> {
    let harness = &adapter.name;
    let available = accounts(paths, adapter);
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
             pick one with `omh set account <name>` or `-a <name>`",
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
            // A trailing slash claims a whole config directory. Naming
            // individual files means guessing which one holds the login, and
            // that guess is different on every platform.
            let is_dir = template.ends_with('/');
            let trimmed = template.trim_end_matches('/');
            let guest = crate::adapter::expand(trimmed, guest_home);
            // Storage mirrors the guest path, so the account directory is
            // legible rather than a pile of mangled names.
            let relative = trimmed.trim_start_matches("$HOME/");
            CredMount {
                host: account_dir.join(relative),
                guest,
                file: !is_dir,
            }
        })
        .collect()
}

/// Create the files a bind mount needs to land as files.
///
/// Also lays down mountpoints for any capability nested inside a credential
/// directory: Docker refuses to create one inside a bind-mounted host directory
/// ("is outside of rootfs") and the whole launch fails.
///
/// Docker turns a mount of a non-existent host path into a *directory*, so a
/// first login would write its token into a folder the harness cannot read.
/// An existing credential is never touched; an empty file left by an older omh
/// is repaired, because empty is not a login and is what breaks the harness.
pub fn prepare(adapter: &Adapter, account_dir: &std::path::Path, guest_home: &str) -> Result<()> {
    let creds = mounts(adapter, account_dir, guest_home);

    // Docker refuses to create a mountpoint inside a bind-mounted host
    // directory, so anything omh will mount *inside* a credential directory has
    // to exist on the host first or the launch fails outright.
    for binding in adapter.capabilities.values() {
        let guest = crate::adapter::expand(&binding.path, guest_home);
        let Some(cred) = creds
            .iter()
            .find(|c| !c.file && guest.starts_with(&c.guest))
        else {
            continue;
        };
        let Ok(relative) = guest.strip_prefix(&cred.guest) else {
            continue;
        };
        let point = cred.host.join(relative);
        if binding.render == crate::adapter::Render::Dir {
            std::fs::create_dir_all(&point)?;
        } else {
            if let Some(parent) = point.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !point.exists() {
                std::fs::write(&point, placeholder(&point))?;
            }
        }
    }

    for cred in creds {
        if cred.file {
            if let Some(parent) = cred.host.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Empty counts as absent: it is not a login, and an empty JSON file
            // is what makes a harness refuse to start.
            let empty = std::fs::metadata(&cred.host)
                .map(|m| m.len() == 0)
                .unwrap_or(true);
            if empty {
                std::fs::write(&cred.host, placeholder(&cred.host))?;
            }
        } else {
            std::fs::create_dir_all(&cred.host)?;
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
    adapter: &Adapter,
    explicit: Option<&str>,
    configured: Option<&str>,
) -> Result<Option<String>> {
    let asked = explicit.or(configured).is_some();
    if !asked && accounts(paths, adapter).is_empty() {
        return Ok(None);
    }
    resolve(paths, adapter, explicit, configured).map(Some)
}

/// Does this file hold more than what `prepare` put there?
///
/// Read as bytes, and an unreadable file counts as *present*: a credential omh
/// cannot read is still a credential, and calling it empty makes a successful
/// login report as incomplete.
fn holds_content(path: &std::path::Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => !is_placeholder(&String::from_utf8_lossy(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// Any file below `dir` holding more than what `prepare` put there.
fn has_real_content(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let p = e.path();
            if p.is_dir() {
                has_real_content(&p)
            } else {
                holds_content(&p)
            }
        })
}

/// Whether a file holds nothing but what `prepare` put there.
fn is_placeholder(content: &str) -> bool {
    matches!(content.trim(), "" | "{}")
}

/// Content a placeholder must have to be parseable. Empty is already valid TOML
/// and YAML; it is not valid JSON, and a harness that parses its config on
/// startup refuses to run rather than treating it as absent:
///
///   Claude configuration file at /home/agent/.claude.json is corrupted:
///   JSON Parse error: Unexpected EOF
fn placeholder(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => "{}",
        _ => "",
    }
}

/// Whether the files on the host can decide this login at all.
///
/// `unfilled` reports what is *missing*, and for an adapter naming `token`
/// files that answers the question outright. For a [`Probe`] adapter it cannot:
/// the credentials are somewhere `unfilled` has no way to read — which is the
/// entire reason the probe exists — so it falls through to "does the creds
/// directory hold anything", and that is true from the harness's first start.
///
/// omp shipped in exactly that state. `unfilled` came back empty because
/// `~/.omp/agent/` held `agent.db`, written by starting omp and never by
/// logging in, and `omh auth omp` announced a captured account to a user who
/// had opened the harness, run nothing, and quit. The probe was added to
/// prevent that false positive and was wired into `doctor` alone, leaving the
/// function its own documentation cites still answering the old way.
///
/// [`Probe`]: crate::adapter::Probe
pub fn decided_by_files(adapter: &Adapter) -> bool {
    // An adapter with neither is the pre-existing weak path: nothing to stat
    // and nothing to ask, so the directory heuristic is all there is. It stays
    // "decided" because saying otherwise would make every such adapter
    // unconfirmable without giving anyone a way to confirm it.
    !adapter.token.is_empty() || adapter.token_probe.is_none()
}

/// Declared credential files that still hold nothing but a placeholder.
///
/// "Something was written" is too weak a success test: a harness writes its
/// default config just by *starting*, so an aborted login leaves a plausible
/// looking account with no token in it. Every file the adapter declares has to
/// have been filled in.
pub fn unfilled(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    guest_home: &str,
) -> Vec<PathBuf> {
    // The adapter names what proves a login. Falling back to "does the config
    // directory hold anything" reports success for boot noise — a harness fills
    // that directory just by starting.
    if !adapter.token.is_empty() {
        return adapter
            .token
            .iter()
            .map(|t| {
                let guest = crate::adapter::expand(t.trim_end_matches('/'), guest_home);
                let host = account_dir.join(t.trim_end_matches('/').trim_start_matches("$HOME/"));
                (host, guest)
            })
            .filter(|(host, _)| !holds_content(host))
            .map(|(_, guest)| guest)
            .collect();
    }

    mounts(adapter, account_dir, guest_home)
        .into_iter()
        .filter(|c| {
            if c.file {
                !holds_content(&c.host)
            } else {
                !has_real_content(&c.host)
            }
        })
        .map(|c| c.guest)
        .collect()
}

/// Reject an account name that is not a single path component.
///
/// `auth::dir` joins this onto the creds root, and credentials mount
/// **writable** — so `omh auth claude --name ../../..` would resolve to `~`
/// and hand the agent the user's real credential store. `Path::join` with an absolute
/// path discards the prefix entirely, which needs no traversal at all.
pub fn validate_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        anyhow::bail!("an account needs a name");
    }
    if trimmed == "." || trimmed == ".." {
        anyhow::bail!("`{name}` is not an account name");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("an account name is a single name, not a path: `{name}`");
    }
    Ok(())
}

/// Did the login complete? Pure so the decision can be tested apart from the
/// process that produced it.
pub fn login_outcome(runtime_ok: bool, unfilled: &[PathBuf]) -> Result<()> {
    // Order matters: a runtime that never started leaves the previous
    // credentials in place, which would otherwise read as a fresh login.
    if !runtime_ok {
        anyhow::bail!("the sandbox exited with an error — nothing was captured");
    }
    if !unfilled.is_empty() {
        anyhow::bail!(
            "the login did not complete — still empty:\n{}",
            unfilled
                .iter()
                .map(|p| format!("    {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: d.path().join("home"),
            repo: d.path().join("repo"),
        };
        (d, paths)
    }

    /// Write what the adapter declares as proof of a login.
    fn capture(paths: &Paths, harness: &str, account: &str) {
        let adapter = if harness == "claude" {
            claude()
        } else {
            opencode()
        };
        for token in &adapter.token {
            let p = dir(paths, harness, account).join(token.trim_start_matches("$HOME/"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{\"token\":\"x\"}").unwrap();
        }
    }

    fn claude() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    fn omp() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "omp").unwrap()
    }

    /// A harness whose credentials omh cannot stat is never *decided* by them.
    ///
    /// `unfilled` answers "is anything obviously missing", and for an adapter
    /// with `token` files that is the whole question. For a `token-probe`
    /// adapter it is not: omp keeps credentials in SQLite, so `unfilled` falls
    /// through to "does the creds directory hold anything", which is true from
    /// the harness's first boot — settings, model_perf, usage_history. An empty
    /// `unfilled` there means *nothing is obviously missing*, never *the login
    /// worked*, and `omh auth` said the second.
    #[test]
    fn a_probe_adapter_is_never_decided_by_its_files() {
        assert!(
            decided_by_files(&claude()),
            "claude names token files; those files are the answer"
        );
        assert!(
            !decided_by_files(&omp()),
            "omp keeps credentials in SQLite — the files cannot answer, which is \
             why it declares a probe at all"
        );
    }

    /// And the boot-noise case itself: an omp account that never logged in.
    ///
    /// This is the state the bug produced — the creds directory holds a
    /// database written by starting the harness, `unfilled` is empty, and the
    /// old code read that as a completed login.
    #[test]
    fn boot_noise_alone_does_not_decide_an_omp_login() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "omp", "personal");
        std::fs::create_dir_all(account.join(".omp/agent")).unwrap();
        // What omp writes just by starting: settings and telemetry, no token.
        std::fs::write(account.join(".omp/agent/agent.db"), "SQLite format 3\0…").unwrap();

        assert!(
            unfilled(&omp(), &account, "/home/agent").is_empty(),
            "the files are all present — this is exactly why they cannot decide"
        );
        assert!(
            !decided_by_files(&omp()),
            "so omh must not report this as a captured login on the files alone"
        );
    }

    fn opencode() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "opencode").unwrap()
    }

    fn adapter_with(creds: &[&str]) -> Adapter {
        let list = creds
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        toml::from_str(&format!(
            r#"
            name = "t"
            bin = "t"
            install = "x"
            creds = [{list}]
            [capabilities.rules]
            path = "/work/AGENTS.md"
            render = "concat"
            "#
        ))
        .unwrap()
    }

    // ── listing ─────────────────────────────────────────────────────────────

    #[test]
    fn there_are_no_accounts_before_any_login() {
        let (_d, paths) = fixture();
        assert!(accounts(&paths, &claude()).is_empty());
    }

    #[test]
    fn accounts_are_listed_by_name() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(accounts(&paths, &claude()), vec!["personal", "work"]);
    }

    #[test]
    fn accounts_are_kept_apart_per_harness() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        assert!(accounts(&paths, &opencode()).is_empty());
    }

    /// Regression in spirit: an interrupted `omh auth` used to leave a directory
    /// behind that reported as authed.
    #[test]
    fn an_empty_account_directory_is_not_captured() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(dir(&paths, "claude", "work")).unwrap();
        assert!(!is_captured(&paths, &claude(), "work"));
        assert!(
            accounts(&paths, &claude()).is_empty(),
            "and it is not listed"
        );
    }

    // ── choosing ────────────────────────────────────────────────────────────

    #[test]
    fn an_explicit_account_wins() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve(&paths, &claude(), Some("work"), Some("personal")).unwrap(),
            "work"
        );
    }

    #[test]
    fn the_configured_account_is_used_when_there_are_several() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve(&paths, &claude(), None, Some("personal")).unwrap(),
            "personal"
        );
    }

    #[test]
    fn a_single_account_needs_no_choosing() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(resolve(&paths, &claude(), None, None).unwrap(), "personal");
    }

    /// Two identities and no stated preference is exactly when guessing is
    /// most expensive — you would send work traffic through a personal account
    /// and never notice.
    #[test]
    fn several_accounts_with_no_preference_is_an_error_that_lists_them() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        let err = resolve(&paths, &claude(), None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("work") && err.contains("personal"),
            "got: {err}"
        );
        assert!(
            err.contains("omh set account"),
            "must say how to fix it: {err}"
        );
    }

    #[test]
    fn no_accounts_at_all_points_at_omh_auth() {
        let (_d, paths) = fixture();
        let err = resolve(&paths, &claude(), None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("omh auth claude"), "got: {err}");
    }

    #[test]
    fn naming_an_account_that_was_never_captured_is_an_error() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        let err = resolve(&paths, &claude(), Some("nope"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "got: {err}");
    }

    // ── mounting ────────────────────────────────────────────────────────────

    #[test]
    fn credentials_mount_where_the_harness_actually_looks() {
        let account = PathBuf::from("/host/creds/claude/work");
        let m = mounts(&claude(), &account, "/home/agent");
        assert!(
            m.iter().all(|c| c.guest.starts_with("/home/agent")),
            "got: {m:?}"
        );
        assert!(
            m.iter().any(|c| c.guest.ends_with(".claude")),
            "config dir: {m:?}"
        );
    }

    /// Storage mirrors the guest path, so `ls ~/.omh/creds/claude/work` is
    /// legible instead of a directory of mangled names.
    #[test]
    fn stored_credentials_mirror_the_path_they_came_from() {
        let account = PathBuf::from("/host/creds/claude/work");
        let m = mounts(&claude(), &account, "/home/agent");
        let store = m.iter().find(|c| c.guest.ends_with(".claude")).unwrap();
        assert_eq!(store.host, account.join(".claude"));
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
    fn preparing_creates_the_paths_a_bind_mount_needs() {
        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("work");
        prepare(
            &adapter_with(&["$HOME/.cfg/", "$HOME/.cfg.json"]),
            &account,
            "/home/agent",
        )
        .unwrap();

        assert!(account.join(".cfg").is_dir());
        let f = account.join(".cfg.json");
        assert!(
            f.is_file(),
            "must be a file, or docker mounts a directory over it"
        );
    }

    /// An empty file is valid TOML and valid YAML but *not* valid JSON, and a
    /// harness that parses its config on startup refuses to run:
    ///
    ///   Claude configuration file at /home/agent/.claude.json is corrupted:
    ///   JSON Parse error: Unexpected EOF
    ///
    /// A placeholder has to be parseable, not merely present.
    #[test]
    fn json_placeholders_are_parseable() {
        let d = tempfile::tempdir().unwrap();
        prepare(&adapter_with(&["$HOME/.cfg.json"]), d.path(), "/home/agent").unwrap();
        let body = std::fs::read_to_string(d.path().join(".cfg.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&body)
            .unwrap_or_else(|e| panic!("placeholder is not valid JSON ({e}): {body:?}"));
    }

    /// Empty is already valid for these, and inventing content could look like
    /// configuration the user did not write.
    #[test]
    fn non_json_placeholders_stay_empty() {
        let d = tempfile::tempdir().unwrap();
        prepare(&adapter_with(&["$HOME/.cfg.toml"]), d.path(), "/home/agent").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join(".cfg.toml")).unwrap(),
            ""
        );
    }

    /// The empty file a previous omh left behind is exactly what breaks the
    /// harness, and it is not a login, so replacing it loses nothing.
    #[test]
    fn an_empty_placeholder_left_by_an_older_omh_is_repaired() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".cfg.json"), "").unwrap();
        prepare(&adapter_with(&["$HOME/.cfg.json"]), d.path(), "/home/agent").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join(".cfg.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn preparing_never_clobbers_an_existing_login() {
        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("work");
        let f = account.join(".claude.json");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "{\"token\":\"keep-me\"}").unwrap();

        prepare(&claude(), &account, "/home/agent").unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "{\"token\":\"keep-me\"}"
        );
    }

    #[test]
    fn a_parseable_placeholder_is_still_not_a_login() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "claude", "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        assert!(
            !is_captured(&paths, &claude(), "work"),
            "`{{}}` is something the harness can parse, not something you logged into"
        );
    }

    #[test]
    fn preparing_alone_does_not_count_as_captured() {
        let (_d, paths) = fixture();
        prepare(&claude(), &dir(&paths, "claude", "work"), "/home/agent").unwrap();
        assert!(
            !is_captured(&paths, &claude(), "work"),
            "empty placeholder files are not a login"
        );
    }

    // ── resolving at launch ─────────────────────────────────────────────────

    /// You must be able to run a harness before you have ever logged in — the
    /// harness itself will prompt.
    #[test]
    fn launching_without_any_account_is_allowed() {
        let (_d, paths) = fixture();
        assert_eq!(
            resolve_for_launch(&paths, &claude(), None, None).unwrap(),
            None
        );
    }

    /// Regression: `-a work` for an account that does not exist silently ran
    /// with no credentials at all, so the session started logged out and said
    /// nothing about why.
    #[test]
    fn naming_a_missing_account_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        let err = resolve_for_launch(&paths, &claude(), Some("work"), None).unwrap_err();
        assert!(err.to_string().contains("work"), "got: {err}");
    }

    #[test]
    fn a_configured_account_that_is_missing_also_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert!(resolve_for_launch(&paths, &claude(), None, Some("work")).is_err());
    }

    #[test]
    fn the_only_account_is_used_at_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve_for_launch(&paths, &claude(), None, None)
                .unwrap()
                .as_deref(),
            Some("personal")
        );
    }

    #[test]
    fn two_identities_and_no_preference_still_stops() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert!(resolve_for_launch(&paths, &claude(), None, None).is_err());
    }

    // ── directories vs files ────────────────────────────────────────────────

    /// Guessing which single file holds a login is how omh got this wrong:
    /// Claude Code keeps tokens in one place and the account record in another,
    /// and neither is the same on every platform. A trailing slash lets an
    /// adapter claim a whole config directory instead of naming files.
    #[test]
    fn a_trailing_slash_declares_a_directory() {
        let a = adapter_with(&["$HOME/.claude/", "$HOME/.claude.json"]);
        let m = mounts(&a, Path::new("/acct"), "/home/agent");

        let d = m.iter().find(|c| c.guest.ends_with(".claude")).unwrap();
        assert!(
            !d.file,
            "a directory, so docker must not treat it as a file"
        );
        let f = m
            .iter()
            .find(|c| c.guest.ends_with(".claude.json"))
            .unwrap();
        assert!(f.file);
    }

    #[test]
    fn preparing_creates_directories_as_directories() {
        let d = tempfile::tempdir().unwrap();
        let a = adapter_with(&["$HOME/.claude/", "$HOME/.claude.json"]);
        prepare(&a, d.path(), "/home/agent").unwrap();

        assert!(d.path().join(".claude").is_dir(), "must be a directory");
        assert!(d.path().join(".claude.json").is_file(), "must be a file");
    }

    #[test]
    fn storage_still_mirrors_the_guest_path_for_directories() {
        let a = adapter_with(&["$HOME/.claude/"]);
        let m = mounts(&a, Path::new("/acct"), "/home/agent");
        assert_eq!(m[0].host, Path::new("/acct/.claude"));
    }

    /// The shipped adapter must capture the account record as well as the
    /// tokens. Claude Code keeps them in two different files, and mounting only
    /// one leaves a session that starts logged out.
    #[test]
    fn the_claude_adapter_captures_tokens_and_the_account_record() {
        let guests: Vec<String> = mounts(&claude(), Path::new("/acct"), "/home/agent")
            .iter()
            .map(|c| c.guest.display().to_string())
            .collect();
        assert!(
            guests.iter().any(|g| g.ends_with(".claude")),
            "tokens: {guests:?}"
        );
        assert!(
            guests.iter().any(|g| g.ends_with(".claude.json")),
            "account: {guests:?}"
        );
    }

    /// Credentials must be a *directory*, because a bind-mounted file cannot be
    /// replaced by rename:
    ///
    ///   mv: cannot move '.tmp1' to '.credentials.json': Device or resource busy
    ///
    /// Every tool that saves a token writes a temp file and renames over it, so
    /// mounting the file itself means the login succeeds and never persists.
    #[test]
    fn the_token_store_is_a_directory_not_a_file() {
        let m = mounts(&claude(), Path::new("/acct"), "/home/agent");
        let store = m
            .iter()
            .find(|c| c.guest.ends_with(".claude"))
            .expect("the config directory must be mounted");
        assert!(!store.file, "a mounted file cannot be renamed over");
    }

    /// Docker refuses to *create* a mountpoint inside a bind-mounted host
    /// directory ("is outside of rootfs"), so every capability that lands
    /// inside a credential directory needs its mountpoint prepared on the host
    /// first — otherwise the whole launch fails.
    #[test]
    fn capabilities_nested_in_a_credential_directory_get_mountpoints() {
        let d = tempfile::tempdir().unwrap();
        prepare(&claude(), d.path(), "/home/agent").unwrap();

        assert!(
            d.path().join(".claude/skills").is_dir(),
            "skills mountpoint"
        );
        assert!(
            d.path().join(".claude/commands").is_dir(),
            "commands mountpoint"
        );
        assert!(
            d.path().join(".claude/agents").is_dir(),
            "subagents mountpoint"
        );
        assert!(
            d.path().join(".claude/settings.json").is_file(),
            "hooks mountpoint"
        );
    }

    #[test]
    fn capabilities_outside_a_credential_directory_are_left_alone() {
        let d = tempfile::tempdir().unwrap();
        prepare(&claude(), d.path(), "/home/agent").unwrap();
        // rules live in the worktree, which omh mounts itself
        assert!(!d.path().join("work").exists());
    }

    // ── did the login actually happen ───────────────────────────────────────

    /// Regression: `omh auth` reported success after the harness merely wrote
    /// its default config on startup. The token file was still empty and the
    /// next session was logged out.
    #[test]
    fn a_config_written_by_merely_starting_is_not_a_login() {
        let d = tempfile::tempdir().unwrap();
        prepare(&claude(), d.path(), "/home/agent").unwrap();
        // what Claude Code writes just by booting
        std::fs::write(d.path().join(".claude.json"), r#"{"userID":"abc"}"#).unwrap();

        let missing = unfilled(&claude(), d.path(), "/home/agent");
        assert!(
            !missing.is_empty(),
            "the token was never written, so the login is not complete: {missing:?}"
        );
    }

    #[test]
    fn a_completed_login_leaves_nothing_unfilled() {
        let d = tempfile::tempdir().unwrap();
        prepare(&claude(), d.path(), "/home/agent").unwrap();
        std::fs::write(d.path().join(".claude.json"), r#"{"userID":"abc"}"#).unwrap();
        std::fs::write(
            d.path().join(".claude/.credentials.json"),
            r#"{"token":"t"}"#,
        )
        .unwrap();

        assert!(unfilled(&claude(), d.path(), "/home/agent").is_empty());
    }

    /// Asserted against what the adapter declares, not a hardcoded count — the
    /// number is a property of `claude.toml`, not of the behaviour.
    #[test]
    fn an_untouched_account_reports_every_declared_proof_unfilled() {
        let d = tempfile::tempdir().unwrap();
        let a = claude();
        prepare(&a, d.path(), "/home/agent").unwrap();
        assert_eq!(unfilled(&a, d.path(), "/home/agent").len(), a.token.len());
    }

    // ── account names are path components ───────────────────────────────────

    #[test]
    fn ordinary_account_names_are_accepted() {
        for name in ["work", "personal", "acme-corp", "user.name", "a_b"] {
            validate_name(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    /// Credentials mount writable, so an account name that escapes its
    /// directory hands the agent the user's real credential store — the
    /// inverse of the guarantee the worktree model exists to provide.
    #[test]
    fn an_account_name_cannot_escape_its_directory() {
        for name in ["..", "../..", "../../..", "a/../..", "work/sub"] {
            assert!(validate_name(name).is_err(), "`{name}` must be rejected");
        }
    }

    /// `Path::join` with an absolute path discards the prefix, so this needs no
    /// traversal to reach anywhere on the filesystem.
    #[test]
    fn an_absolute_account_name_is_rejected() {
        for name in ["/", "/etc", "/Users/someone/.claude"] {
            assert!(validate_name(name).is_err(), "`{name}` must be rejected");
        }
    }

    #[test]
    fn an_empty_or_dot_account_name_is_rejected() {
        for name in ["", ".", "   "] {
            assert!(validate_name(name).is_err(), "`{name:?}` must be rejected");
        }
    }

    // ── did the login happen ────────────────────────────────────────────────

    /// Regression: the container's exit status was discarded, so a docker
    /// failure (exit 125: bad mount, missing network) on an account that
    /// already had credentials read as a successful re-authentication.
    #[test]
    fn a_runtime_that_failed_is_never_a_successful_login() {
        let err = login_outcome(false, &[]).unwrap_err().to_string();
        assert!(!err.is_empty());
        assert!(
            !err.contains("did not complete"),
            "must not blame the user for a runtime failure: {err}"
        );
    }

    #[test]
    fn an_unfilled_credential_is_reported_with_its_path() {
        let err = login_outcome(true, &[PathBuf::from("/acct/.claude/.credentials.json")])
            .unwrap_err()
            .to_string();
        assert!(err.contains(".credentials.json"), "got: {err}");
    }

    #[test]
    fn a_clean_run_that_filled_everything_succeeds() {
        assert!(login_outcome(true, &[]).is_ok());
    }

    // ── what proves a login ─────────────────────────────────────────────────

    /// Regression, confirmed live: a harness fills its config directory just by
    /// starting — Claude Code writes `statsig/`, `projects/`, `todos/` on boot.
    /// Inferring the login from "does this directory hold anything" therefore
    /// reports success for a session that has no token at all.
    #[test]
    fn boot_noise_in_the_config_directory_is_not_a_login() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "claude", "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        std::fs::write(account.join(".claude.json"), r#"{"userID":"abc"}"#).unwrap();
        std::fs::create_dir_all(account.join(".claude/statsig")).unwrap();
        std::fs::write(account.join(".claude/statsig/session.123"), r#"{"s":"1"}"#).unwrap();

        assert!(
            !unfilled(&claude(), &account, "/home/agent").is_empty(),
            "no token was written, so the login is not complete"
        );
        assert!(
            !is_captured(&paths, &claude(), "work"),
            "and the account is not usable"
        );
        assert!(accounts(&paths, &claude()).is_empty(), "nor listed");
    }

    #[test]
    fn a_written_token_is_a_login() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "claude", "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        std::fs::write(account.join(".claude/.credentials.json"), r#"{"t":"x"}"#).unwrap();

        assert!(unfilled(&claude(), &account, "/home/agent").is_empty());
        assert!(is_captured(&paths, &claude(), "work"));
        assert_eq!(accounts(&paths, &claude()), vec!["work"]);
    }

    /// Every adapter has to name the file that proves a login; nothing else can
    /// distinguish a token from telemetry.
    #[test]
    fn shipped_adapters_declare_what_proves_a_login() {
        for name in ["claude", "opencode"] {
            let a = Adapter::find(Path::new(ADAPTERS), name).unwrap();
            assert!(
                !a.token.is_empty(),
                "{name} does not say what proves a login"
            );
        }
    }

    /// Regression: the two predicates disagreed, so `omh auth` could fail with
    /// "the login did not complete" while `omh info` listed the account and the
    /// next launch happily used it.
    #[test]
    fn captured_means_exactly_nothing_left_unfilled() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "claude", "work");
        prepare(&claude(), &account, "/home/agent").unwrap();

        for stage in ["", r#"{"userID":"a"}"#] {
            if !stage.is_empty() {
                std::fs::write(account.join(".claude.json"), stage).unwrap();
            }
            assert_eq!(
                is_captured(&paths, &claude(), "work"),
                unfilled(&claude(), &account, "/home/agent").is_empty(),
                "the two answers must never differ"
            );
        }
    }

    /// A credential omh cannot read as text is *present*, not absent. Treating
    /// it as empty makes a successful login report as incomplete, and hides the
    /// account from every later command.
    #[test]
    fn an_unreadable_credential_is_not_mistaken_for_an_empty_one() {
        let (_d, paths) = fixture();
        let account = dir(&paths, "claude", "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        std::fs::write(
            account.join(".claude/.credentials.json"),
            [0xff, 0xfe, 0x00],
        )
        .unwrap();

        assert!(
            unfilled(&claude(), &account, "/home/agent").is_empty(),
            "a non-UTF-8 token is still a token"
        );
        assert!(is_captured(&paths, &claude(), "work"));
    }
}
