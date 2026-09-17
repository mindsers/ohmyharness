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
use anyhow::Context;
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

/// Where one account's captured login lives.
///
/// **The adapter, not the word that found it.** `Adapter::find` resolves
/// `<word>.toml` by filename and nothing requires that filename to match the
/// adapter's own `name`, so the two spellings are free to disagree — and this
/// is the one place where they disagreeing is silent. `accounts` and
/// `is_captured` read `creds/<adapter.name>/`; when this took a `&str`, every
/// caller outside this module handed it the typed word bar one, so a launch
/// resolved an account out of one directory and mounted another. The report named the
/// account, the agent started logged out, and the token it went on to obtain
/// was written where `omh auth` does not look.
///
/// Taking the `Adapter` is the whole fix: there is no longer a second string
/// to pass, so the two halves cannot be keyed differently.
pub fn dir(paths: &Paths, adapter: &Adapter, account: &str) -> PathBuf {
    paths.creds(adapter.name.as_str()).join(account)
}

/// Accounts captured for a harness, in name order.
///
/// **Absent is none; unreadable is an error.** This opened with
/// `let Ok(entries) = read_dir(..) else { return Vec::new() }` over a
/// `.flatten()`, so a `creds/<harness>/` omh could not read answered "no
/// accounts" — `resolve_for_launch` then returned `Ok(None)`, the launch
/// mounted no credentials and exited 0, and the agent started logged out with
/// its next token written where `omh auth` does not look. That is the symptom
/// [`dir`] above says it exists to end, reached by the other door.
///
/// The per-entry read matters on its own: one entry dropped silently is one
/// account fewer, and [`resolve`] then finds a single candidate where there
/// are two and picks an identity without asking — which is the guess its own
/// doc forbids, defeated by the read that feeds it.
pub fn accounts(paths: &Paths, adapter: &Adapter) -> Result<Vec<String>> {
    let at = paths.creds(adapter.name.as_str());
    let entries = match std::fs::read_dir(&at) {
        Ok(entries) => entries,
        // Nobody has run `omh auth` for this harness yet, which is an ordinary
        // state and the one `resolve` already has words for.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(anyhow::Error::new(e)).with_context(|| format!("reading {}", at.display()))
        }
    };
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", at.display()))?;
        // Not `Path::is_dir()`, which answers `false` for every error alike —
        // the same stat-failure-is-evidence rule `owning_checkout` follows.
        let meta = std::fs::metadata(entry.path())
            .with_context(|| format!("examining {}", entry.path().display()))?;
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_captured(paths, adapter, &name) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// Captured means credentials are actually present. An empty directory left by
/// an interrupted login must never look like a successful one.
pub fn is_captured(paths: &Paths, adapter: &Adapter, account: &str) -> bool {
    // Defined in terms of `unfilled` so the two answers can never disagree —
    // they did, and `omh auth` failed while `omh info` listed the account.
    unfilled(adapter, &dir(paths, adapter, account), GUEST_HOME).is_empty()
}

/// Which account to use. Ambiguity is an error, never a guess — silently
/// picking the wrong identity is worse than stopping.
pub fn resolve(paths: &Paths, adapter: &Adapter, configured: Option<&str>) -> Result<String> {
    let harness = &adapter.name;
    let available = accounts(paths, adapter)?;
    if available.is_empty() {
        // The same `--import` offer as `logged_out`, and where it matters more:
        // this is the line a user with `account` set sees, and the setting is
        // one name for every harness.
        let import = if adapter.token.is_empty() {
            String::new()
        } else {
            format!(", or `omh auth {harness} --import` to copy this machine's login")
        };
        anyhow::bail!("no account for {harness} — run `omh auth {harness}` first{import}");
    }

    // One source. It was `explicit.or(configured)` — a global `-a` overriding
    // the setting for one invocation — and the flag recorded nothing, so a
    // session started with it could not be resumed without repeating it. What
    // is left is the setting, which every command already fell back to.
    if let Some(name) = configured {
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
             pick one with `omh set account <name>`",
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
    configured: Option<&str>,
) -> Result<Option<String>> {
    if configured.is_none() && accounts(paths, adapter)?.is_empty() {
        return Ok(None);
    }
    resolve(paths, adapter, configured).map(Some)
}

/// What a launch with no account says before handing over to the harness.
///
/// Starting logged out stays allowed — the harness prompts — but that prompt
/// names the harness's own login, which may be one that cannot finish in a
/// sandbox. Codex's did: `codex login` waits on the container's loopback.
/// `--import` is offered only where a login is a file omh can copy.
pub fn logged_out(adapter: &Adapter) -> String {
    let harness = &adapter.name;
    let import = if adapter.token.is_empty() {
        String::new()
    } else {
        format!(", or `omh auth {harness} --import` to copy this machine's login")
    };
    format!("no {harness} account — starting logged out. `omh auth {harness}` to log in{import}")
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
///
/// **Absent is empty; unreadable is content**, which is [`holds_content`]'s
/// rule one level up. This was `read_dir(dir).into_iter().flatten().flatten()`,
/// so a directory omh could not open — and an entry that failed mid-walk —
/// vanished into "nothing here", and the two predicates one function apart gave
/// opposite answers to the same question. For a directory-mount adapter, whose
/// `token` list is empty and which therefore reaches this rather than the early
/// return in [`unfilled`], that turned a completed login into *the login did
/// not complete*, naming a guest path the user has no way to fill.
fn has_real_content(dir: &std::path::Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Nothing was ever written here. `prepare` creates the mount points, so
        // this is a path no login has reached, not a read that failed.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    entries.into_iter().any(|e| match e {
        // `readdir` failing part-way through is the same answer as the open
        // failing: omh did not get to look, so it must not report absence.
        Err(_) => true,
        Ok(e) => {
            let p = e.path();
            if p.is_dir() {
                has_real_content(&p)
            } else {
                holds_content(&p)
            }
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
    // **One string, checked once.** The emptiness and dot checks read
    // `name.trim()` while the separator check read `name`, and [`dir`] joined
    // the raw one — so `"work "` passed as though it were `"work"` and then
    // named a directory `accounts` would list under a name nothing matches.
    // Refused rather than trimmed: [`resolve`] decides by membership of that
    // listing, so a name quietly rewritten at capture stops matching what the
    // user types afterwards.
    if name != name.trim() {
        anyhow::bail!(
            "an account name cannot begin or end with a space: `{name}` would \
             name a directory that does not read as `{}`",
            name.trim()
        );
    }
    if name.is_empty() {
        anyhow::bail!("an account needs a name");
    }
    if name == "." || name == ".." {
        anyhow::bail!("`{name}` is not an account name");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("an account name is a single name, not a path: `{name}`");
    }
    // The value reaches the terminal in every message about this account, and
    // `session::harness_of` records what an escape in one printed.
    if name.chars().any(char::is_control) {
        anyhow::bail!("an account name cannot hold control characters");
    }
    Ok(())
}

/// Copy a login the host already holds into an account, without a sandbox.
///
/// For a harness whose login cannot finish inside one — an OAuth redirect to a
/// port on the container's loopback, which the host's browser never reaches —
/// and for anyone already logged in on this machine. Returns the host files it
/// copied.
///
/// Only the adapter's `token` files, never the `creds` directory around them:
/// a config directory holds boot noise and omh's own mounts, and copying it
/// would bring both along and call the result a login. A harness with no
/// `token` files keeps its login somewhere a copy cannot prove, so it is
/// refused rather than guessed at.
///
/// Every file is checked before any is written, so a host missing one of two
/// leaves the account as it was.
pub fn import(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    host_home: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;
    let harness = &adapter.name;
    if adapter.token.is_empty() {
        anyhow::bail!(
            "{harness} keeps its login where omh cannot copy it — no file on its \
             own proves one — so there is nothing to import; run `omh auth {harness}`"
        );
    }
    let pairs: Vec<(PathBuf, PathBuf)> = adapter
        .token
        .iter()
        .map(|t| {
            let relative = t.trim_end_matches('/').trim_start_matches("$HOME/");
            (host_home.join(relative), account_dir.join(relative))
        })
        .collect();
    let missing: Vec<String> = pairs
        .iter()
        .filter(|(host, _)| !holds_content(host))
        .map(|(host, _)| format!("    {}", host.display()))
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "no {harness} login on this machine to import — nothing in:\n{}",
            missing.join("\n")
        );
    }
    for (host, account) in &pairs {
        if let Some(parent) = account.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(host, account)
            .with_context(|| format!("copying {} to {}", host.display(), account.display()))?;
        std::fs::set_permissions(account, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(pairs.into_iter().map(|(host, _)| host).collect())
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
            let p = dir(paths, &adapter, account).join(token.trim_start_matches("$HOME/"));
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
        let account = dir(&paths, &omp(), "personal");
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

    // ── importing a host login ──────────────────────────────────────────────

    fn codex() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "codex").unwrap()
    }

    /// A login as Codex writes it with an API key — measured, not invented, and
    /// not `{}`, which is the placeholder `holds_content` reads as no login.
    const CODEX_LOGIN: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-test"}"#;

    fn host_with(file: &str, content: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
        home
    }

    #[test]
    fn a_host_login_lands_in_the_named_account() {
        let (_d, paths) = fixture();
        let home = host_with(".codex/auth.json", CODEX_LOGIN);
        let account = dir(&paths, &codex(), "work");
        prepare(&codex(), &account, GUEST_HOME).unwrap();

        let copied = import(&codex(), &account, home.path()).unwrap();

        assert_eq!(copied, vec![home.path().join(".codex/auth.json")]);
        assert_eq!(
            std::fs::read_to_string(account.join(".codex/auth.json")).unwrap(),
            CODEX_LOGIN
        );
        assert!(is_captured(&paths, &codex(), "work"));
        assert_eq!(
            accounts(&paths, &codex()).unwrap(),
            vec!["work"],
            "into the account named, and no other"
        );
    }

    /// A token is a secret; the copy is readable by its owner alone, whatever
    /// the host file's mode was.
    #[test]
    fn an_imported_token_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths) = fixture();
        let home = host_with(".codex/auth.json", CODEX_LOGIN);
        std::fs::set_permissions(
            home.path().join(".codex/auth.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let account = dir(&paths, &codex(), "work");
        prepare(&codex(), &account, GUEST_HOME).unwrap();

        import(&codex(), &account, home.path()).unwrap();

        let mode = std::fs::metadata(account.join(".codex/auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// Nothing to import is a refusal that says where omh looked, and leaves
    /// no account behind that reads as captured.
    #[test]
    fn a_host_that_never_logged_in_imports_nothing() {
        let (_d, paths) = fixture();
        let home = tempfile::tempdir().unwrap();
        let account = dir(&paths, &codex(), "work");
        prepare(&codex(), &account, GUEST_HOME).unwrap();

        let err = import(&codex(), &account, home.path())
            .unwrap_err()
            .to_string();

        assert!(err.contains(".codex/auth.json"), "names the path: {err}");
        assert!(!is_captured(&paths, &codex(), "work"));
    }

    /// The placeholder `prepare` writes is not a login on the host either — an
    /// omh-prepared directory mounted somewhere would otherwise import as one.
    #[test]
    fn a_placeholder_on_the_host_is_not_a_login() {
        let (_d, paths) = fixture();
        let home = host_with(".codex/auth.json", "{}");
        let account = dir(&paths, &codex(), "work");
        prepare(&codex(), &account, GUEST_HOME).unwrap();

        assert!(import(&codex(), &account, home.path()).is_err());
        assert!(!is_captured(&paths, &codex(), "work"));
    }

    /// A login in two files, one of them missing on the host, leaves the
    /// account as it was — half a login is not a login, and a copied half would
    /// overwrite a working account's file with one that no longer matches.
    #[test]
    fn half_a_login_imports_nothing() {
        let (_d, paths) = fixture();
        let two: Adapter = toml::from_str(
            r#"
            name = "two"
            bin = "two"
            install = "x"
            creds = ["$HOME/.two/"]
            token = ["$HOME/.two/a.json", "$HOME/.two/b.json"]
            [capabilities.rules]
            path = "/work/AGENTS.md"
            render = "concat"
            "#,
        )
        .unwrap();
        let home = host_with(".two/a.json", CODEX_LOGIN);
        let account = dir(&paths, &two, "work");
        prepare(&two, &account, GUEST_HOME).unwrap();

        let err = import(&two, &account, home.path()).unwrap_err().to_string();

        assert!(err.contains(".two/b.json"), "names what is missing: {err}");
        assert!(
            !account.join(".two/a.json").exists(),
            "and wrote neither file"
        );
    }

    /// A harness whose login is not a file cannot be copied as one. omp keeps
    /// credentials in SQLite beside boot noise; copying the directory would
    /// import settings and telemetry and call it a login.
    #[test]
    fn a_login_that_is_not_a_file_cannot_be_imported() {
        let (_d, paths) = fixture();
        let home = host_with(".omp/agent/agent.db", "SQLite format 3\0…");
        let account = dir(&paths, &omp(), "work");

        let err = import(&omp(), &account, home.path())
            .unwrap_err()
            .to_string();

        assert!(err.contains("omp"), "names the harness: {err}");
        assert!(
            !account.join(".omp/agent/agent.db").exists(),
            "and copies nothing"
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
        assert!(accounts(&paths, &claude()).unwrap().is_empty());
    }

    #[test]
    fn accounts_are_listed_by_name() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(
            accounts(&paths, &claude()).unwrap(),
            vec!["personal", "work"]
        );
    }

    #[test]
    fn accounts_are_kept_apart_per_harness() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        assert!(accounts(&paths, &opencode()).unwrap().is_empty());
    }

    /// Regression in spirit: an interrupted `omh auth` used to leave a directory
    /// behind that reported as authed.
    #[test]
    fn an_empty_account_directory_is_not_captured() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(dir(&paths, &claude(), "work")).unwrap();
        assert!(!is_captured(&paths, &claude(), "work"));
        assert!(
            accounts(&paths, &claude()).unwrap().is_empty(),
            "and it is not listed"
        );
    }

    // ── choosing ────────────────────────────────────────────────────────────

    #[test]
    fn the_configured_account_is_used_when_there_are_several() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve(&paths, &claude(), Some("personal")).unwrap(),
            "personal"
        );
    }

    #[test]
    fn a_single_account_needs_no_choosing() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(resolve(&paths, &claude(), None).unwrap(), "personal");
    }

    /// Two identities and no stated preference is exactly when guessing is
    /// most expensive — you would send work traffic through a personal account
    /// and never notice.
    #[test]
    fn several_accounts_with_no_preference_is_an_error_that_lists_them() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        capture(&paths, "claude", "personal");
        let err = resolve(&paths, &claude(), None).unwrap_err().to_string();
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
        let err = resolve(&paths, &claude(), None).unwrap_err().to_string();
        assert!(err.contains("omh auth claude"), "got: {err}");
    }

    #[test]
    fn naming_an_account_that_was_never_captured_is_an_error() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "work");
        let err = resolve(&paths, &claude(), Some("nope"))
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
        let account = dir(&paths, &claude(), "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        assert!(
            !is_captured(&paths, &claude(), "work"),
            "`{{}}` is something the harness can parse, not something you logged into"
        );
    }

    #[test]
    fn preparing_alone_does_not_count_as_captured() {
        let (_d, paths) = fixture();
        prepare(&claude(), &dir(&paths, &claude(), "work"), "/home/agent").unwrap();
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
        assert_eq!(resolve_for_launch(&paths, &claude(), None).unwrap(), None);
    }

    /// Regression: `-a work` for an account that does not exist silently ran
    /// with no credentials at all, so the session started logged out and said
    /// nothing about why.
    #[test]
    fn naming_a_missing_account_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        let err = resolve_for_launch(&paths, &claude(), Some("work")).unwrap_err();
        assert!(err.to_string().contains("work"), "got: {err}");
    }

    #[test]
    fn a_configured_account_that_is_missing_also_stops_the_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert!(resolve_for_launch(&paths, &claude(), Some("work")).is_err());
    }

    #[test]
    fn the_only_account_is_used_at_launch() {
        let (_d, paths) = fixture();
        capture(&paths, "claude", "personal");
        assert_eq!(
            resolve_for_launch(&paths, &claude(), None)
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
        assert!(resolve_for_launch(&paths, &claude(), None).is_err());
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

    /// An account name is the name it prints.
    ///
    /// The checks split across two strings: `.`, `..` and emptiness were tested
    /// against `name.trim()` while the separators were tested against `name` —
    /// and `dir` then joined the **raw** one. So `"work "` and `"work"` both
    /// passed and named two different directories, one of which no listing of
    /// the other would ever match. That is the same "two spellings, one
    /// meaning" split `auth::dir` was just changed to make unrepresentable for
    /// harnesses, one argument along.
    ///
    /// Refused rather than trimmed, which is the choice `resolve` already makes
    /// about accounts: it compares against the directory listing, so a name
    /// silently rewritten at capture would stop matching what the user typed
    /// afterwards. One string, checked once.
    ///
    /// Control characters go with it for the reason `session::harness_of`
    /// gives: the value reaches the terminal in error messages, and a marker
    /// carrying an ANSI escape has printed one here before.
    #[test]
    fn an_account_name_is_the_name_it_prints() {
        for name in ["work ", " work", "work\n", "wo\u{7}rk", "work\t"] {
            assert!(
                validate_name(name).is_err(),
                "`{name:?}` must be rejected — it names a directory that no \
                 listing of the name it looks like would match"
            );
        }

        // The invariant the rejections exist to hold: anything accepted is
        // exactly what `dir` will join.
        for name in ["work", "personal", "acme-corp", "user.name", "a_b"] {
            validate_name(name).unwrap();
            assert_eq!(
                name,
                name.trim(),
                "an accepted name has to be its own trim, or `dir` joins a \
                 different string from the one that was checked"
            );
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
        let account = dir(&paths, &claude(), "work");
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
        assert!(
            accounts(&paths, &claude()).unwrap().is_empty(),
            "nor listed"
        );
    }

    #[test]
    fn a_written_token_is_a_login() {
        let (_d, paths) = fixture();
        let account = dir(&paths, &claude(), "work");
        prepare(&claude(), &account, "/home/agent").unwrap();
        std::fs::write(account.join(".claude/.credentials.json"), r#"{"t":"x"}"#).unwrap();

        assert!(unfilled(&claude(), &account, "/home/agent").is_empty());
        assert!(is_captured(&paths, &claude(), "work"));
        assert_eq!(accounts(&paths, &claude()).unwrap(), vec!["work"]);
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
        let account = dir(&paths, &claude(), "work");
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
        let account = dir(&paths, &claude(), "work");
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

    /// The same rule one level up: a credential **directory** omh cannot read
    /// is *present*, not empty.
    ///
    /// [`holds_content`] decided this for a file — "a credential omh cannot
    /// read is still a credential" — and [`has_real_content`] answered the
    /// opposite for the directory it walks, so an unreadable
    /// `creds/<harness>/<account>/.omp/agent` made a completed login report as
    /// *the login did not complete*, naming a guest path the user has no way
    /// to fill. A directory-mount adapter is the only one that reaches it:
    /// `claude` names `token` files and never gets past the early return.
    ///
    /// Skipped under root, which reads through `0o000` — the test would pass
    /// without testing anything.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_credential_directory_is_not_mistaken_for_an_empty_one() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (_d, paths) = fixture();
        let account = dir(&paths, &omp(), "personal");
        let agent = account.join(".omp/agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("agent.db"), "SQLite format 3\0…").unwrap();
        // Restores the mode it observed, so a panic cannot leave a `0o000`
        // directory `TempDir` then fails to remove.
        let _restore = Restore::unreadable(&agent).unwrap();

        assert!(
            unfilled(&omp(), &account, "/home/agent").is_empty(),
            "a credential directory omh cannot read still holds credentials"
        );
    }

    /// Mode guard for the fixture above: `0o000` on the way in, the observed
    /// mode back on drop.
    #[cfg(unix)]
    struct Restore {
        dir: PathBuf,
        mode: u32,
    }

    #[cfg(unix)]
    impl Restore {
        fn unreadable(dir: &Path) -> std::io::Result<Self> {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir)?.permissions().mode();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000))?;
            Ok(Self {
                dir: dir.to_path_buf(),
                mode,
            })
        }
    }

    #[cfg(unix)]
    impl Drop for Restore {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(self.mode));
        }
    }
}
