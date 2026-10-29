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
    // An account holding only its model provider is one too: a provider's
    // token is the login for a harness pointed at it (#197 §6).
    let dir = dir(paths, adapter, account);
    let provider = adapter
        .provider
        .as_ref()
        .is_some_and(|p| dir.join(crate::provider::file(p.format)).is_file());
    provider || unfilled(adapter, &dir, GUEST_HOME).is_empty()
}

/// Which account to use. Ambiguity is an error, never a guess — silently
/// picking the wrong identity is worse than stopping.
pub fn resolve(paths: &Paths, adapter: &Adapter, configured: Option<&str>) -> Result<String> {
    let home = dirs::home_dir().context("no home directory")?;
    resolve_with_home(paths, adapter, configured, &home)
}

/// [`resolve`], with the host home the `--import` offer is checked against.
pub fn resolve_with_home(
    paths: &Paths,
    adapter: &Adapter,
    configured: Option<&str>,
    host_home: &std::path::Path,
) -> Result<String> {
    let harness = &adapter.name;
    let available = accounts(paths, adapter)?;
    if available.is_empty() {
        // The same `--import` offer as `logged_out`, and where it matters more:
        // this is the line a user with `account` set sees, and the setting is
        // one name for every harness.
        let import = import_offer(adapter, host_home);
        anyhow::bail!("no account for {harness} — run `omh auth {harness}` first{import}");
    }

    // One source. It was `explicit.or(configured)` — a global `-a` overriding
    // the setting for one invocation — and the flag recorded nothing, so a
    // session started with it could not be resumed without repeating it. What
    // is left is the setting, which every command already fell back to.
    if let Some(name) = configured {
        if !available.iter().any(|a| a == name) {
            anyhow::bail!(
                "no account `{name}` for {harness}\n  captured: {}\n  \
                 omh set account {harness}:<name>   one for {harness} alone",
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
/// directory, and for any path `[policy] mask` hides there: Docker refuses to
/// create one inside a bind-mounted host directory ("is outside of rootfs")
/// and the whole launch fails.
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
    // A masked path is a file mount like a rendered document's, and so is the
    // file a provider render hides.
    let masked = adapter
        .policy
        .iter()
        .flat_map(|p| p.mask.iter())
        .map(|path| (path, Point::Empty));
    let provided = adapter
        .provider
        .iter()
        .flat_map(|p| p.path.iter())
        .map(|path| (path, Point::File));
    let bound = adapter.capabilities.values().map(|b| match b.render {
        crate::adapter::Render::Dir => (&b.path, Point::Dir),
        _ => (&b.path, Point::File),
    });
    for (path, point) in bound.chain(masked).chain(provided) {
        let guest = crate::adapter::expand(path, guest_home);
        let Some(cred) = creds
            .iter()
            .find(|c| !c.file && guest.starts_with(&c.guest))
        else {
            continue;
        };
        let Ok(relative) = guest.strip_prefix(&cred.guest) else {
            continue;
        };
        mountpoint(&cred.host, relative, point)?;
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

/// Remove from the account each file `[provider] drop` names: a cache the
/// harness rebuilds, which a session could have written to carry a
/// provider's endpoint into the next one.
///
/// One name at a time through [`crate::held`], from the account down: a link
/// on the way is not entered, a link at the name is the link removed, and a
/// directory there is refused by name. What is not there is already gone.
pub fn drop_carried(adapter: &Adapter, account_dir: &std::path::Path) -> Result<()> {
    let Some(provider) = &adapter.provider else {
        return Ok(());
    };
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    'files: for drop in &provider.drop {
        let rel = std::path::Path::new(drop.trim_start_matches("$HOME/"));
        let names: Vec<&std::ffi::OsStr> = rel.iter().collect();
        let Some((leaf, above)) = names.split_last() else {
            continue;
        };
        let refuse = |e: std::io::Error| {
            anyhow::anyhow!(
                "{} could not be removed ({e}) — a cache omp rebuilds, which a session can \
                 write to carry a provider's address into the next one. Remove it and launch \
                 again",
                account_dir.join(rel).display()
            )
        };
        let mut dir = match crate::held::Dir::open(account_dir) {
            Ok(d) => d,
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(refuse(e)),
        };
        for name in above {
            dir = match dir.sub(name) {
                Ok(d) => d,
                Err(e) if gone(&e) => continue 'files,
                Err(e) => return Err(refuse(e)),
            };
        }
        dir.remove(leaf).map_err(refuse)?;
    }
    Ok(())
}

/// What a mountpoint is for: a directory, a file a document lands on (laid
/// with a placeholder the harness can parse), or a file a mask hides behind
/// (laid empty — a harness may read a non-empty one wherever the mask is not
/// mounted, and Claude prefers a non-empty `~/.claude/.config.json` to
/// `~/.claude.json`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Point {
    Dir,
    File,
    Empty,
}

/// A mountpoint at `relative` under a credential directory's `root`, which a
/// session can write: a directory, or a file for a bind mount to land on.
///
/// Laid one name at a time through [`crate::held`], from `root` down — the
/// mount source itself, which no session can replace. Every directory on the
/// way is entered or made without following a link, and a file is made only
/// where nothing has its name. A regular file already there keeps its bytes
/// (the mount hides them). Anything else on the way — a link, dangling or not,
/// a file where a directory goes, a directory or a pipe where a file goes —
/// may be a session's, and is refused by name rather than written through or
/// mounted over.
fn mountpoint(root: &std::path::Path, relative: &std::path::Path, point: Point) -> Result<()> {
    use std::path::Component;
    let refuse = |at: &std::path::Path, what: &str, e: std::io::Error| {
        anyhow::anyhow!(
            "{} {what} — a mount lands there, and omh will not make anything through or \
             over whatever this is ({e}). Remove it and launch again",
            at.display()
        )
    };
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let mut dir = crate::held::Dir::open(root)
        .map_err(|e| refuse(root, "is not a directory omh can enter", e))?;
    let names: Vec<&std::ffi::OsStr> = relative
        .components()
        .map(|c| match c {
            Component::Normal(n) => Ok(n),
            _ => Err(anyhow::anyhow!(
                "{} is not a path under {}",
                relative.display(),
                root.display()
            )),
        })
        .collect::<Result<_>>()?;
    let (leaf, above) = match point == Point::Dir {
        true => (None, &names[..]),
        false => match names.split_last() {
            Some((leaf, above)) => (Some(*leaf), above),
            None => anyhow::bail!("{} names no file", relative.display()),
        },
    };
    let mut at = root.to_path_buf();
    for name in above {
        at.push(name);
        dir = dir
            .sub_or_create(name)
            .map_err(|e| refuse(&at, "is not a directory omh can make or enter", e))?;
    }
    let Some(leaf) = leaf else {
        return Ok(());
    };
    at.push(leaf);
    match dir.file(leaf) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => dir
            .create_new(
                leaf,
                match point {
                    Point::Empty => b"",
                    _ => placeholder(&at).as_bytes(),
                },
            )
            .map_err(|e| refuse(&at, "could not be made", e)),
        Err(e) => Err(refuse(&at, "is not a regular file", e)),
    }
}

pub use crate::home::Home;

/// A session's copies of the account's `[policy] copy` files: read, checked
/// and scrubbed, not yet written.
///
/// Two steps because a launch can take neither back once it has gone further:
/// these are made before a container is removed or a worktree made, so an
/// account file no session can be seeded from refuses the launch first — a dry
/// run included — and written by [`Seeds::write`] only after the reuse
/// decision, never into a file bind-mounted into a running container.
#[derive(Debug)]
pub struct Seeds {
    /// Each copy, relative to the home, and what it is seeded with.
    files: Vec<(String, String)>,
    /// The account, and a digest of each seed: another account, or the same
    /// one captured again by `omh auth`, or an omh that scrubs differently,
    /// seeds again.
    stamp: String,
}

/// The session's seeds from `account_dir`, or `None` when this harness copies
/// nothing.
pub fn seeds(adapter: &Adapter, account_dir: &std::path::Path) -> Result<Option<Seeds>> {
    let Some(policy) = adapter.policy.as_ref().filter(|p| !p.copy.is_empty()) else {
        return Ok(None);
    };
    let mut files = Vec::new();
    let mut stamp = account_dir.display().to_string();
    for copy in &policy.copy {
        let rel = copy.trim_start_matches("$HOME/");
        let mut doc = account_document(account_dir, rel)
            .map_err(|why| unusable(adapter, account_dir, rel, why))?;
        crate::render::scrub(policy.format, &mut doc);
        let body = serde_json::to_string_pretty(&doc)?;
        stamp.push_str(&format!(
            "\n{rel} {:016x}",
            crate::hash::fnv1a_64(body.as_bytes())
        ));
        files.push((rel.to_string(), body));
    }
    Ok(Some(Seeds { files, stamp }))
}

impl Seeds {
    /// Write the copies into `home`, unless a kept home already holds these
    /// seeds and a copy of each that still parses — the session's own, which
    /// a relaunch continues from. Written whole and atomically, never through
    /// a link, readable by the user alone: a copy holds the account's API key
    /// when it has one.
    pub fn write(&self, home: &Home) -> Result<()> {
        let dir = home.path();
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let stamped = dir.join(SEEDED_FROM);
        let same = match std::fs::read_to_string(&stamped) {
            Ok(s) => s == self.stamp,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e).with_context(|| format!("reading {}", stamped.display())),
        };
        let intact = self.files.iter().all(|(rel, _)| copy_intact(dir, rel));
        if matches!(home, Home::Kept(_)) && same && intact {
            return Ok(());
        }
        for (rel, body) in &self.files {
            place(dir, rel, body)?;
        }
        place(dir, SEEDED_FROM, &self.stamp)
    }
}

/// A copy the harness can read: there, not empty, and a JSON object. One that
/// is gone, emptied or cut short is seeded again rather than handed over.
fn copy_intact(dir: &std::path::Path, rel: &str) -> bool {
    read_account_file(dir, rel).is_ok_and(|raw| {
        !raw.trim().is_empty()
            && serde_json::from_str::<serde_json::Value>(&raw).is_ok_and(|d| d.is_object())
    })
}

/// `body` at `rel` under `dir`, atomically and without following a link, mode
/// `0600`.
fn place(dir: &std::path::Path, rel: &str, body: &str) -> Result<()> {
    let at = dir.join(rel);
    let names: Vec<&std::ffi::OsStr> = std::path::Path::new(rel).iter().collect();
    let Some((leaf, above)) = names.split_last() else {
        anyhow::bail!("{rel} names no file");
    };
    let mut held = crate::held::Dir::open(dir)?;
    for name in above {
        held = held.sub_or_create(name)?;
    }
    held.replace_private(leaf, body.as_bytes())
        .with_context(|| format!("writing {}", at.display()))
}

/// The account's `~/.claude.json` and its kin: read as the agent's, and a JSON
/// object, or the reason it is not one. `prepare`'s placeholder, or nothing at
/// all, is the empty object.
fn account_document(dir: &std::path::Path, rel: &str) -> Result<serde_json::Value> {
    let raw = read_account_file(dir, rel)?;
    if is_placeholder(&raw) {
        return Ok(serde_json::json!({}));
    }
    let doc: serde_json::Value = serde_json::from_str(&raw).context("it is not JSON")?;
    anyhow::ensure!(doc.is_object(), "it is not a JSON object");
    Ok(doc)
}

/// Why `rel` under the account cannot seed a session, and the command that
/// repairs it: `omh auth` sets an unusable file aside before its login.
fn unusable(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    rel: &str,
    why: anyhow::Error,
) -> anyhow::Error {
    let name = account_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    anyhow::anyhow!(
        "{} cannot seed a session: {why:#}. A session before this omh could have \
         written it — `omh auth {} --name {name}` sets it aside and captures the \
         account again",
        account_dir.join(rel).display(),
        adapter.name,
    )
}

/// Scrub the account's own copied files in place, for a login — which mounts
/// them, not a copy, and so would run beside the tokens it writes whatever an
/// earlier session left there. A file no session could be seeded from is set
/// aside for the login to write again. Returns what it said it did.
pub fn scrub_account(adapter: &Adapter, account_dir: &std::path::Path) -> Result<Vec<String>> {
    let Some(policy) = adapter.policy.as_ref().filter(|p| !p.copy.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut said = Vec::new();
    for copy in &policy.copy {
        let rel = copy.trim_start_matches("$HOME/");
        let at = account_dir.join(rel);
        match account_document(account_dir, rel) {
            Ok(mut doc) => {
                if read_account_file(account_dir, rel).is_ok_and(|raw| is_placeholder(&raw)) {
                    continue;
                }
                let before = doc.clone();
                crate::render::scrub(policy.format, &mut doc);
                if doc != before {
                    place(account_dir, rel, &serde_json::to_string_pretty(&doc)?)?;
                }
            }
            Err(why) => {
                let aside = account_dir.join(format!(
                    "{rel}.omh-set-aside-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or_default()
                ));
                std::fs::rename(&at, &aside)
                    .with_context(|| format!("setting {} aside", at.display()))?;
                said.push(format!(
                    "set {} aside as {} ({why:#}) — the login writes it again",
                    at.display(),
                    aside.display()
                ));
            }
        }
    }
    Ok(said)
}

/// Beside a session's copies: the account they were seeded from, and what.
const SEEDED_FROM: &str = ".omh-seeded-from";

/// Far more than any `~/.claude.json` measured (the largest on one machine,
/// 231 KB), and a bound on what a session could make a launch read.
const COPY_CAP: u64 = 16 << 20;

/// `rel` under `dir`, read as a file a session could have written, through
/// [`crate::held::read_under`] and never past [`COPY_CAP`]. Absent is empty —
/// `prepare` lays a placeholder anyway.
fn read_account_file(dir: &std::path::Path, rel: &str) -> Result<String> {
    crate::held::read_under(dir, rel, COPY_CAP)
}

/// The account a launch of this harness is configured with, if any — its own
/// entry in `[account]`, or the one name for every harness.
pub fn configured(paths: &Paths, adapter: &Adapter) -> Result<Option<String>> {
    crate::config::account_for(paths, adapter.name.as_str())
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
/// `--import` is offered only where it would work — see [`import_offer`].
pub fn logged_out(adapter: &Adapter, host_home: &std::path::Path) -> String {
    let harness = &adapter.name;
    let import = import_offer(adapter, host_home);
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
pub(crate) fn is_placeholder(content: &str) -> bool {
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
/// Only the adapter's `token` files, never the config directory around them:
/// on the host that directory also holds your config, skills and history,
/// which omh stages separately, and copying it would call all of that a login.
/// A harness with no `token` files keeps its login somewhere a copy cannot
/// prove, so it is refused rather than guessed at.
///
/// Every host file is read before anything is written, each is staged beside
/// its destination at `0600`, and only once all are staged are they renamed
/// into place — so a file omh cannot read, or a staging write that fails,
/// leaves the account as it was rather than half-replaced. The renames
/// themselves are one per file, not one for the set.
pub fn import(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    host_home: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    let harness = &adapter.name;
    if adapter.token.is_empty() {
        anyhow::bail!(
            "{harness} keeps its login where omh cannot copy it — no file on its \
             own proves one — so there is nothing to import; run `omh auth {harness}`"
        );
    }
    let pairs = token_pairs(adapter, account_dir, host_home)?;

    let mut logins = Vec::new();
    let mut missing = Vec::new();
    for (host, _) in &pairs {
        match read_login(host) {
            HostLogin::Present(bytes) => logins.push(bytes),
            HostLogin::Absent => missing.push(format!("    {}", host.display())),
            HostLogin::DanglingLink(to) => anyhow::bail!(
                "{} is a link to {}, which does not exist — nothing was imported",
                host.display(),
                to.display()
            ),
            HostLogin::Unreadable(e) => anyhow::bail!(
                "could not read {}: {e} — nothing was imported",
                host.display()
            ),
        }
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "no {harness} login file to import — nothing in:\n{}\n\
             {harness} may keep its login elsewhere (a keychain, or a home set \
             by its own environment variable); `omh auth {harness}` logs in \
             instead",
            missing.join("\n")
        );
    }

    let mut staged: Vec<(PathBuf, &PathBuf)> = Vec::new();
    let written = (|| -> Result<()> {
        for ((_, account), bytes) in pairs.iter().zip(&logins) {
            let parent = account.parent().unwrap_or(account_dir);
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
            let temp = staging_path(account);
            write_private(&temp, bytes).with_context(|| format!("writing {}", temp.display()))?;
            staged.push((temp, account));
        }
        for (temp, account) in &staged {
            std::fs::rename(temp, account)
                .with_context(|| format!("replacing {}", account.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        for (temp, _) in &staged {
            let _ = std::fs::remove_file(temp);
        }
        return Err(e);
    }
    Ok(pairs.into_iter().map(|(host, _)| host).collect())
}

/// Each `token` file's host path and its place in the account.
///
/// Only paths under `$HOME/` have both. Anything else joins onto either root
/// as the same absolute path, and a copy onto itself empties the file.
fn token_pairs(
    adapter: &Adapter,
    account_dir: &std::path::Path,
    host_home: &std::path::Path,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    adapter
        .token
        .iter()
        .map(|t| {
            let Some(relative) = t.trim_end_matches('/').strip_prefix("$HOME/") else {
                anyhow::bail!(
                    "{}'s token `{t}` is not under `$HOME/`, so it has no place in \
                     an account to import into",
                    adapter.name
                );
            };
            Ok((host_home.join(relative), account_dir.join(relative)))
        })
        .collect()
}

/// What a host-side token path holds, as far as an import is concerned.
enum HostLogin {
    Present(Vec<u8>),
    /// Absent, or only the placeholder `prepare` writes.
    Absent,
    DanglingLink(PathBuf),
    Unreadable(std::io::Error),
}

/// Read a host token, telling "nothing there" from "could not look".
///
/// Not [`holds_content`], which counts an unreadable file as present: right
/// for `unfilled`, where a credential omh cannot read is still one, and wrong
/// here, where the bytes are the thing being copied.
fn read_login(path: &std::path::Path) -> HostLogin {
    match std::fs::read(path) {
        Ok(bytes) if is_placeholder(&String::from_utf8_lossy(&bytes)) => HostLogin::Absent,
        Ok(bytes) => HostLogin::Present(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match std::fs::read_link(path) {
            Ok(to) => HostLogin::DanglingLink(to),
            Err(_) => HostLogin::Absent,
        },
        Err(e) => HostLogin::Unreadable(e),
    }
}

/// Where a token is staged before it replaces the one in the account: beside
/// it, so the rename stays on one filesystem.
fn staging_path(account: &std::path::Path) -> PathBuf {
    let mut name = account.file_name().unwrap_or_default().to_os_string();
    name.push(".omh-import");
    account.with_file_name(name)
}

/// Write a file that is private from its first byte — never the host file's
/// mode, even for the moment before a `chmod`.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The `--import` offer, where one would work.
///
/// Only where the host holds a login `import` would copy. Claude Code on macOS
/// keeps its login in the Keychain, so offering `--import` on the strength of
/// the adapter alone sent every logged-in Mac user to a command that refuses.
pub fn import_offer(adapter: &Adapter, host_home: &std::path::Path) -> String {
    let harness = &adapter.name;
    let importable = !adapter.token.is_empty()
        && token_pairs(adapter, host_home, host_home).is_ok_and(|pairs| {
            pairs
                .iter()
                .all(|(host, _)| matches!(read_login(host), HostLogin::Present(_)))
        });
    if importable {
        format!(", or `omh auth {harness} --import` to copy this machine's login")
    } else {
        String::new()
    }
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

    /// A login as Codex 0.154.0 writes it after `codex login --with-api-key`
    /// (key shortened), and not `{}`, the placeholder read as no login.
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

    /// An adapter whose login is two files, for what one file cannot show.
    fn two() -> Adapter {
        toml::from_str(
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
        .unwrap()
    }

    const OLD_LOGIN: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-old"}"#;

    /// A working account with `OLD_LOGIN` in both of `two`'s files.
    fn captured_two(paths: &Paths) -> PathBuf {
        let account = dir(paths, &two(), "work");
        prepare(&two(), &account, GUEST_HOME).unwrap();
        for f in [".two/a.json", ".two/b.json"] {
            std::fs::write(account.join(f), OLD_LOGIN).unwrap();
        }
        account
    }

    /// A token omh cannot read is not "nothing there", and not a reason to
    /// half-replace a working account.
    ///
    /// `holds_content` counts an unreadable file as present — right for
    /// `unfilled`, where a credential omh cannot read is still a credential —
    /// so the check before writing let it through, the first file was copied
    /// over a working login, and the second failed. A directory stands in for
    /// the unreadable file because it fails to read for root too, which a mode
    /// of `000` does not.
    #[test]
    fn a_token_omh_cannot_read_leaves_a_working_account_whole() {
        let (_d, paths) = fixture();
        let account = captured_two(&paths);
        let home = host_with(".two/a.json", CODEX_LOGIN);
        std::fs::create_dir_all(home.path().join(".two/b.json")).unwrap();

        let err = import(&two(), &account, home.path())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("could not read"),
            "says it could not look: {err}"
        );
        assert!(err.contains(".two/b.json"), "and where: {err}");
        for f in [".two/a.json", ".two/b.json"] {
            assert_eq!(
                std::fs::read_to_string(account.join(f)).unwrap(),
                OLD_LOGIN,
                "{f} was replaced by an import that failed"
            );
        }
    }

    /// A write that fails part-way leaves the account whole and nothing staged.
    ///
    /// The failure has to land *after* the first file is staged, or there is
    /// nothing to clean up and this proves nothing: `b.json`'s staging path is
    /// taken by a non-empty directory, which no `remove_file` clears and no
    /// `create_new` opens — for root too.
    #[test]
    fn a_write_that_fails_leaves_the_account_whole_and_nothing_staged() {
        let (_d, paths) = fixture();
        let account = captured_two(&paths);
        let home = host_with(".two/a.json", CODEX_LOGIN);
        std::fs::write(home.path().join(".two/b.json"), CODEX_LOGIN).unwrap();
        let blocker = account.join(".two/b.json.omh-import");
        std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();

        let err = import(&two(), &account, home.path()).unwrap_err();

        assert!(
            format!("{err:#}").contains("b.json.omh-import"),
            "names what it could not write: {err:#}"
        );
        assert!(
            !account.join(".two/a.json.omh-import").exists(),
            "a staged file was left behind"
        );
        for f in [".two/a.json", ".two/b.json"] {
            assert_eq!(
                std::fs::read_to_string(account.join(f)).unwrap(),
                OLD_LOGIN,
                "{f} was replaced by an import that failed"
            );
        }
    }

    /// A re-import replaces the login it finds, whole.
    #[test]
    fn a_reimport_replaces_the_login_it_finds() {
        let (_d, paths) = fixture();
        let account = captured_two(&paths);
        let home = host_with(".two/a.json", CODEX_LOGIN);
        std::fs::write(home.path().join(".two/b.json"), CODEX_LOGIN).unwrap();

        import(&two(), &account, home.path()).unwrap();

        for f in [".two/a.json", ".two/b.json"] {
            assert_eq!(
                std::fs::read_to_string(account.join(f)).unwrap(),
                CODEX_LOGIN
            );
        }
    }

    /// A link to nothing is a broken link, not a machine that never logged in.
    #[test]
    fn a_dangling_link_is_named_as_one() {
        let (_d, paths) = fixture();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::os::unix::fs::symlink(
            home.path().join("unmounted/auth.json"),
            home.path().join(".codex/auth.json"),
        )
        .unwrap();
        let account = dir(&paths, &codex(), "work");

        let err = import(&codex(), &account, home.path())
            .unwrap_err()
            .to_string();

        assert!(err.contains("link"), "names the link: {err}");
        assert!(
            err.contains("unmounted/auth.json"),
            "and where it points: {err}"
        );
    }

    /// A token outside `$HOME` has no account-side path: both joins resolve to
    /// the same absolute file, and a copy onto itself empties it.
    #[test]
    fn a_token_outside_home_is_refused_and_left_alone() {
        let (_d, paths) = fixture();
        let elsewhere = tempfile::tempdir().unwrap();
        let file = elsewhere.path().join("token.json");
        std::fs::write(&file, CODEX_LOGIN).unwrap();
        let abs: Adapter = toml::from_str(&format!(
            r#"
            name = "abs"
            bin = "abs"
            install = "x"
            creds = ["$HOME/.abs/"]
            token = [{:?}]
            [capabilities.rules]
            path = "/work/AGENTS.md"
            render = "concat"
            "#,
            file.display().to_string()
        ))
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let account = dir(&paths, &abs, "work");

        let err = import(&abs, &account, home.path()).unwrap_err().to_string();

        assert!(err.contains("$HOME"), "says why: {err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), CODEX_LOGIN);
    }

    /// When `import` refuses for want of a file, it says what it checked — the
    /// login may live where omh does not look, and "no login" would be a guess.
    #[test]
    fn a_missing_login_names_only_what_was_checked() {
        let (_d, paths) = fixture();
        let home = tempfile::tempdir().unwrap();
        let account = dir(&paths, &codex(), "work");

        let err = import(&codex(), &account, home.path())
            .unwrap_err()
            .to_string();

        assert!(
            !err.contains("no codex login on this machine"),
            "claims more than it checked: {err}"
        );
        assert!(
            err.contains("elsewhere"),
            "allows for a login it cannot see: {err}"
        );
    }

    // ── the `--import` offer ────────────────────────────────────────────────

    /// Offered where an import would work, and only there. Claude Code on
    /// macOS keeps its login in the Keychain, so a logged-in Mac has no
    /// `.credentials.json` — and a launch that offered `--import` anyway sent
    /// every such user to a command that refuses.
    #[test]
    fn import_is_offered_only_where_it_would_succeed() {
        let with = host_with(".codex/auth.json", CODEX_LOGIN);
        let without = tempfile::tempdir().unwrap();
        let placeholder = host_with(".codex/auth.json", "{}");
        let omp_home = host_with(".omp/agent/agent.db", "SQLite format 3\0…");

        assert!(import_offer(&codex(), with.path()).contains("omh auth codex --import"));
        assert_eq!(import_offer(&codex(), without.path()), "");
        assert_eq!(import_offer(&codex(), placeholder.path()), "");
        assert_eq!(import_offer(&omp(), omp_home.path()), "");
    }

    /// Both messages that name `omh auth` carry the offer — the logged-out
    /// warning and the refusal a configured account gets.
    #[test]
    fn the_offer_reaches_both_messages() {
        let with = host_with(".codex/auth.json", CODEX_LOGIN);
        assert!(logged_out(&codex(), with.path()).contains("--import"));

        let (_d, paths) = fixture();
        let err = resolve_with_home(&paths, &codex(), None, with.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--import"), "got: {err}");
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

    /// A path the policy masks inside the account needs its mountpoint for the
    /// same reason — and one that is already there is the harness's, which
    /// the mask hides and never rewrites.
    #[test]
    fn a_masked_path_in_a_credential_directory_gets_a_mountpoint_and_keeps_its_bytes() {
        let a = codex();
        let masks = &a.policy.as_ref().unwrap().mask;
        assert!(!masks.is_empty());

        let d = tempfile::tempdir().unwrap();
        prepare(&a, d.path(), "/home/agent").unwrap();
        for mask in masks {
            let rel = mask.trim_start_matches("$HOME/");
            assert!(d.path().join(rel).is_file(), "{mask} has no mountpoint");
            // Empty, never `{}`: a harness may read a non-empty file there
            // wherever the mask is not mounted — Claude prefers a non-empty
            // `~/.claude/.config.json` to `~/.claude.json`.
            assert_eq!(std::fs::read(d.path().join(rel)).unwrap(), b"", "{mask}");
        }

        let d = tempfile::tempdir().unwrap();
        for mask in masks {
            let point = d.path().join(mask.trim_start_matches("$HOME/"));
            std::fs::create_dir_all(point.parent().unwrap()).unwrap();
            std::fs::write(&point, "the harness's own").unwrap();
        }
        prepare(&a, d.path(), "/home/agent").unwrap();
        for mask in masks {
            let point = d.path().join(mask.trim_start_matches("$HOME/"));
            assert_eq!(std::fs::read_to_string(point).unwrap(), "the harness's own");
        }
    }

    /// The file omp reads providers from, which the provider render hides,
    /// needs its mountpoint in the account like a mask — and one already there
    /// keeps its bytes, which a launch reads to see whether it defines
    /// providers of its own. A link there is refused, never written through.
    #[test]
    fn omps_models_file_gets_a_mountpoint_and_keeps_its_bytes() {
        let a = omp();
        let rel = a.provider.as_ref().unwrap().path.as_deref().unwrap();
        let rel = rel.trim_start_matches("$HOME/");

        let d = tempfile::tempdir().unwrap();
        prepare(&a, d.path(), "/home/agent").unwrap();
        assert!(d.path().join(rel).is_file(), "{rel} has no mountpoint");

        let d = tempfile::tempdir().unwrap();
        let point = d.path().join(rel);
        std::fs::create_dir_all(point.parent().unwrap()).unwrap();
        std::fs::write(&point, "providers: {}\n").unwrap();
        prepare(&a, d.path(), "/home/agent").unwrap();
        assert_eq!(std::fs::read_to_string(&point).unwrap(), "providers: {}\n");

        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("elsewhere");
        std::fs::write(&target, "kept").unwrap();
        let point = d.path().join("account").join(rel);
        std::fs::create_dir_all(point.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &point).unwrap();
        assert!(prepare(&a, &d.path().join("account"), "/home/agent").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");
    }

    /// omp's model cache can carry a provider's endpoint into the next
    /// session, and omp rebuilds it: each file `[provider] drop` names goes
    /// before a launch, and nothing else of the account's. A link there is the
    /// link, never what it points at; a directory there is not a file omh
    /// removes, and is named.
    #[test]
    fn a_dropped_file_goes_and_nothing_else_does() {
        let a = omp();
        let drops: Vec<&str> = a
            .provider
            .as_ref()
            .unwrap()
            .drop
            .iter()
            .map(|d| d.trim_start_matches("$HOME/"))
            .collect();
        assert!(!drops.is_empty());
        let d = tempfile::tempdir().unwrap();
        let agent = d.path().join(".omp/agent");
        std::fs::create_dir_all(&agent).unwrap();
        for rel in &drops {
            std::fs::write(d.path().join(rel), "cached").unwrap();
        }
        std::fs::write(agent.join("agent.db"), "logins").unwrap();
        drop_carried(&a, d.path()).unwrap();
        for rel in &drops {
            assert!(!d.path().join(rel).exists(), "{rel} is still there");
        }
        assert_eq!(
            std::fs::read_to_string(agent.join("agent.db")).unwrap(),
            "logins"
        );
        // Again, with nothing left to drop.
        drop_carried(&a, d.path()).unwrap();
        // An account with no omp directory yet.
        drop_carried(&a, &d.path().join("fresh")).unwrap();

        let target = d.path().join("elsewhere");
        std::fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(drops[0])).unwrap();
        drop_carried(&a, d.path()).unwrap();
        assert!(std::fs::symlink_metadata(d.path().join(drops[0])).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");

        std::fs::create_dir(d.path().join(drops[0])).unwrap();
        let e = format!("{:#}", drop_carried(&a, d.path()).unwrap_err());
        assert!(e.contains(drops[0]), "{e}");
    }

    /// The account was writable in every session before the mask, so what
    /// sits at a masked path may be a session's: a link is never written
    /// through, and neither it nor a directory there is taken for the file.
    #[test]
    fn a_masked_path_that_is_not_a_file_is_refused_and_never_written_through() {
        let a = codex();
        let mask = a.policy.as_ref().unwrap().mask[0].trim_start_matches("$HOME/");

        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("account");
        let point = account.join(mask);
        std::fs::create_dir_all(point.parent().unwrap()).unwrap();
        let outside = d.path().join("outside");
        std::os::unix::fs::symlink(&outside, &point).unwrap();
        let err = prepare(&a, &account, "/home/agent")
            .unwrap_err()
            .to_string();
        assert!(!outside.exists(), "a write went through the link");
        assert!(err.contains("cloud-config-bundle-cache.json"), "{err}");

        let d = tempfile::tempdir().unwrap();
        let point = d.path().join(mask);
        std::fs::create_dir_all(&point).unwrap();
        let err = prepare(&a, d.path(), "/home/agent")
            .unwrap_err()
            .to_string();
        assert!(err.contains("cloud-config-bundle-cache.json"), "{err}");
    }

    /// Every directory between the credential root and a mountpoint is the
    /// session's too: a link anywhere on the way is refused by name, and
    /// nothing is made where it points — a file's mountpoint or a directory's.
    #[test]
    fn a_link_above_a_mountpoint_is_refused_and_never_written_through() {
        let omp = Adapter::find(Path::new(ADAPTERS), "omp").unwrap();
        let d = tempfile::tempdir().unwrap();
        let account = d.path().join("account");
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(account.join(".omp/agent")).unwrap();
        std::os::unix::fs::symlink(&outside, account.join(".omp/agent/hooks")).unwrap();
        let err = prepare(&omp, &account, "/home/agent").unwrap_err();
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            0,
            "something was made through the link"
        );
        assert!(format!("{err:#}").contains("hooks"), "{err:#}");

        // A directory capability, under a linked parent.
        let claude = claude();
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(d.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&outside, d.path().join(".claude/skills")).unwrap();
        let err = prepare(&claude, d.path(), "/home/agent").unwrap_err();
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(format!("{err:#}").contains("skills"), "{err:#}");
    }

    /// A link to a regular file is still a link: refused, never kept as the
    /// file it points at.
    #[test]
    fn a_link_to_a_regular_file_at_a_mountpoint_is_refused() {
        let a = codex();
        let mask = a.policy.as_ref().unwrap().mask[0].trim_start_matches("$HOME/");
        let d = tempfile::tempdir().unwrap();
        let point = d.path().join(mask);
        std::fs::create_dir_all(point.parent().unwrap()).unwrap();
        std::fs::write(d.path().join("real.json"), "{}").unwrap();
        std::os::unix::fs::symlink(d.path().join("real.json"), &point).unwrap();
        let err = prepare(&a, d.path(), "/home/agent")
            .unwrap_err()
            .to_string();
        assert!(err.contains("cloud-config-bundle-cache.json"), "{err}");
    }

    // ── a session's own copies ──────────────────────────────────────────────

    fn copied(home: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap()
    }

    /// The account's `~/.claude.json`, as a session before this one could
    /// have left it: a server of its own, and identity worth keeping.
    fn planted_account() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        prepare(&claude(), d.path(), "/home/agent").unwrap();
        std::fs::write(
            d.path().join(".claude.json"),
            r#"{"userID":"u","mcpServers":{"planted":{"command":"omh","env":{"PATH":"/x"}}}}"#,
        )
        .unwrap();
        d
    }

    /// Seeds from `account` into a kept home, as a launch does.
    fn seed(account: &Path, home: &Path) -> Result<()> {
        seeds(&claude(), account)?
            .expect("claude copies its .claude.json")
            .write(&Home::Kept(home.to_path_buf()))
    }

    #[test]
    fn a_session_is_seeded_its_own_copy_without_what_names_a_server() {
        let account = planted_account();
        let home = tempfile::tempdir().unwrap();
        seed(account.path(), home.path()).unwrap();
        let doc = copied(home.path());
        assert_eq!(doc["userID"], "u", "identity comes from the account: {doc}");
        assert!(doc.get("mcpServers").is_none(), "{doc}");
        // It keeps the account's API key, if it has one: no other user's.
        let mode = std::fs::metadata(home.path().join(".claude.json"))
            .unwrap()
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.mode() & 0o777, 0o600, "the copy is the user's alone");
    }

    /// Seeded once: a relaunch continues from what the session made of its
    /// copy — unless the account is another one, its file changed, or the copy
    /// is gone or broken, each of which reseeds. A scratch home always does.
    #[test]
    fn a_copy_is_seeded_once_per_session_and_account() {
        let account = planted_account();
        let home = tempfile::tempdir().unwrap();
        seed(account.path(), home.path()).unwrap();
        std::fs::write(home.path().join(".claude.json"), r#"{"theme":"mine"}"#).unwrap();
        seed(account.path(), home.path()).unwrap();
        assert_eq!(copied(home.path())["theme"], "mine", "a relaunch reseeded");

        // Another account whose scrubbed file is the same: still another.
        let other = planted_account();
        seed(other.path(), home.path()).unwrap();
        assert!(
            copied(home.path()).get("theme").is_none(),
            "another account kept the old copy"
        );

        std::fs::write(home.path().join(".claude.json"), r#"{"theme":"mine"}"#).unwrap();
        seeds(&claude(), other.path())
            .unwrap()
            .unwrap()
            .write(&Home::Scratch(home.path().to_path_buf()))
            .unwrap();
        assert!(
            copied(home.path()).get("theme").is_none(),
            "a scratch home kept a copy"
        );

        // `omh auth` capturing the same account again reaches the sessions.
        std::fs::write(
            other.path().join(".claude.json"),
            r#"{"userID":"relogged"}"#,
        )
        .unwrap();
        seed(other.path(), home.path()).unwrap();
        assert_eq!(
            copied(home.path())["userID"],
            "relogged",
            "a re-login never reached it"
        );

        // A copy deleted, emptied or cut short is the harness's to read on
        // the next start: it is seeded again rather than handed over broken.
        for broken in [None, Some(""), Some(r#"{"trunc"#)] {
            match broken {
                None => std::fs::remove_file(home.path().join(".claude.json")).unwrap(),
                Some(b) => std::fs::write(home.path().join(".claude.json"), b).unwrap(),
            }
            seed(other.path(), home.path()).unwrap();
            assert_eq!(
                copied(home.path())["userID"],
                "relogged",
                "{broken:?} was kept"
            );
        }
    }

    /// The account's file was writable from every session before this: read
    /// as the agent's, and refused — by name, with the cause and the one
    /// command that repairs it — rather than seeded as something it is not.
    #[test]
    fn an_account_file_that_cannot_seed_a_session_is_refused_with_its_remedy() {
        let account = planted_account();
        let elsewhere = account.path().join("elsewhere.json");
        std::fs::write(&elsewhere, r#"{"userID":"followed"}"#).unwrap();
        std::fs::remove_file(account.path().join(".claude.json")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, account.path().join(".claude.json")).unwrap();
        let huge = format!("{{\"pad\":\"{}\"}}", "x".repeat(20 << 20));
        let cases: [(&[u8], &str); 5] = [
            (b"{ not json", "not JSON"),
            (b"[]", "not a JSON object"),
            (b"\"x\"", "not a JSON object"),
            (b"{\"userID\":\"u\xfe\"}", "not UTF-8"),
            (huge.as_bytes(), "larger than omh reads"),
        ];
        let name = account
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let remedy = format!("omh auth claude --name {name}");

        let err = format!("{:#}", seeds(&claude(), account.path()).err().unwrap());
        assert!(
            err.contains(".claude.json") && err.contains(&remedy),
            "{err}"
        );
        assert!(!err.contains("followed"), "a link was followed: {err}");

        for (body, cause) in cases {
            let account = planted_account();
            std::fs::write(account.path().join(".claude.json"), body).unwrap();
            let name = account
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let err = format!("{:#}", seeds(&claude(), account.path()).err().unwrap());
            assert!(err.contains(cause), "{cause}: {err}");
            assert!(
                err.contains(&format!("omh auth claude --name {name}")),
                "{err}"
            );
        }
    }

    /// `prepare`'s placeholder is no identity: the copy holds only what omh adds.
    #[test]
    fn a_placeholder_account_file_seeds_an_empty_copy() {
        let account = tempfile::tempdir().unwrap();
        prepare(&claude(), account.path(), "/home/agent").unwrap();
        let home = tempfile::tempdir().unwrap();
        seed(account.path(), home.path()).unwrap();
        let mut empty = serde_json::json!({});
        crate::render::scrub(crate::adapter::PolicyFormat::ClaudeManaged, &mut empty);
        assert_eq!(
            copied(home.path()),
            empty,
            "no identity, and only what omh adds"
        );
    }

    /// A login mounts the account's own file, so it is scrubbed in place first
    /// — a planted helper or environment would otherwise run beside the tokens
    /// being written — and one no session could be seeded from is set aside,
    /// said so, and left for the login to write again.
    #[test]
    fn a_login_scrubs_the_accounts_own_file_and_sets_an_unusable_one_aside() {
        let account = planted_account();
        let mut doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(account.path().join(".claude.json")).unwrap(),
        )
        .unwrap();
        doc["apiKeyHelper"] = "/home/agent/.claude/x.sh".into();
        std::fs::write(account.path().join(".claude.json"), doc.to_string()).unwrap();
        let said = scrub_account(&claude(), account.path()).unwrap();
        assert!(said.is_empty(), "{said:?}");
        let after: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(account.path().join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(after["userID"], "u", "{after}");
        assert!(
            after.get("apiKeyHelper").is_none() && after.get("mcpServers").is_none(),
            "{after}"
        );

        let account = planted_account();
        std::fs::write(account.path().join(".claude.json"), "{ not json").unwrap();
        let said = scrub_account(&claude(), account.path()).unwrap();
        assert!(said.iter().any(|s| s.contains("aside as")), "{said:?}");
        assert!(!account.path().join(".claude.json").exists());
        let aside: Vec<_> = std::fs::read_dir(account.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".claude.json.omh-set-aside"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
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
