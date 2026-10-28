//! A harness's preferences, per session: the declared keys of its own config
//! (`[preferences]` in its adapter) kept per account, copied into each
//! session, and written back only as those keys.
//!
//! The account's config file is shared by every session on it, so whatever
//! one session wrote there — a server, `developer_instructions`, a provider —
//! the next session read. Here the account keeps only declared keys, at its
//! root where no mount reaches, and each session gets its own copy:
//!
//! ```text
//! ~/.omh/creds/<harness>/<account>/preferences/<file>   the account's kept values
//! ~/.omh/run/<repo>/<session>/.preferences/<harness>/
//!   account   the account this session launched with
//!   base/     the account's kept values when this session last synced
//!   reviewed/ the live values at the last decision
//!   live/     what the harness reads and writes, mounted at GUEST
//! ```

/// Where a container finds its session's `live/`, for a `link` adapter: the
/// account's file is a link to the same name here.
pub const GUEST: &str = "/omh/prefs";

use crate::adapter::{Adapter, Declared, KeyPath, PrefMount};
use crate::home::Home;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The value at `key`, if `doc` has one.
pub fn get<'a>(doc: &'a toml::Table, key: &KeyPath) -> Option<&'a toml::Value> {
    let (last, above) = key.split_last()?;
    let mut table = doc;
    for name in above {
        table = table.get(name)?.as_table()?;
    }
    table.get(last)
}

/// `doc` with `key` set to `value`, or removed when it is `None` — and a
/// table emptied by the removal removed with it, so a deletion leaves nothing
/// behind. A value in the way of a table is replaced by one.
pub fn set(doc: &mut toml::Table, key: &KeyPath, value: Option<toml::Value>) {
    let Some((first, rest)) = key.split_first() else {
        return;
    };
    if rest.is_empty() {
        match value {
            Some(v) => {
                doc.insert(first.clone(), v);
            }
            None => {
                doc.remove(first);
            }
        }
        return;
    }
    if value.is_none() && !doc.get(first).is_some_and(toml::Value::is_table) {
        return;
    }
    let entry = doc
        .entry(first.clone())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !entry.is_table() {
        *entry = toml::Value::Table(toml::Table::new());
    }
    if let toml::Value::Table(table) = entry {
        set(table, &rest.to_vec(), value);
        if table.is_empty() {
            doc.remove(first);
        }
    }
}

/// The declared keys of `doc`, and nothing else: what an account keeps and a
/// session is seeded with. Built key by key from parsed values, never copied
/// as bytes.
pub fn pick<'a>(doc: &toml::Table, keys: impl IntoIterator<Item = &'a KeyPath>) -> toml::Table {
    let mut out = toml::Table::new();
    for key in keys {
        if let Some(v) = get(doc, key) {
            set(&mut out, key, Some(v.clone()));
        }
    }
    out
}

/// A declared key whose value differs between two readings — *absent* is a
/// value: a key removed is a change to `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub key: KeyPath,
    pub from: Option<toml::Value>,
    pub to: Option<toml::Value>,
}

/// What changed from `before` to `after`, declared key by declared key.
pub fn changed<'a>(
    before: &toml::Table,
    after: &toml::Table,
    keys: impl IntoIterator<Item = &'a KeyPath>,
) -> Vec<Change> {
    keys.into_iter()
        .filter_map(|key| {
            let (from, to) = (get(before, key), get(after, key));
            (from != to).then(|| Change {
                key: key.clone(),
                from: from.cloned(),
                to: to.cloned(),
            })
        })
        .collect()
}

/// The keys of `doc` no declared key covers, by name and never by value —
/// values can be tokens. A table holding no declared key is named whole; one
/// holding some is named down to what is left out of it.
pub fn undeclared(doc: &toml::Table, keys: &[&KeyPath]) -> Vec<String> {
    fn walk(table: &toml::Table, at: &mut KeyPath, keys: &[&KeyPath], out: &mut Vec<String>) {
        for (name, value) in table {
            at.push(name.clone());
            let covered = keys.iter().any(|k| at.starts_with(k));
            let inside = keys.iter().any(|k| k.len() > at.len() && k.starts_with(at));
            match (covered, inside, value) {
                (true, _, _) => {}
                (false, true, toml::Value::Table(t)) => walk(t, at, keys, out),
                _ => out.push(dotted(at)),
            }
            at.pop();
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut Vec::new(), keys, &mut out);
    out
}

/// `key` as TOML spells it, a segment quoted where a bare one could not be.
pub fn dotted(key: &KeyPath) -> String {
    key.iter()
        .map(|s| {
            let bare = !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            match bare {
                true => s.clone(),
                false => format!("{:?}", s),
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

// ── On disk ─────────────────────────────────────────────────────────────────

/// The most of a preferences file omh reads. Codex's own config is a few
/// kilobytes; this is for a session that leaves a huge one to be read.
const CAP: u64 = 1 << 20;

/// At the account's root, where no mount reaches: its kept values, by file
/// name, and the files below.
const KEPT: &str = "preferences";

/// In [`KEPT`]: the account's files are links now, so a real file where one
/// should be was put there since — by a session, never by omh.
const MARKER: &str = "linked";

/// In [`KEPT`]: held while a session's values are kept into the account, or
/// the account's file is migrated.
const LOCK: &str = ".lock";

/// In a session's preferences home: the account it was seeded from.
const RECORD: &str = "account";

/// The directory a container mounts at [`GUEST`].
pub fn live(home: &Home) -> PathBuf {
    home.path().join("live")
}

/// The account's own file, relative to the account directory.
fn account_rel(d: &Declared) -> &str {
    d.path.trim_start_matches("$HOME/")
}

/// Where the account's file links to.
fn target(d: &Declared) -> PathBuf {
    Path::new(GUEST).join(&d.name)
}

/// `dir/rel`'s directory, held from `dir` down without following anything —
/// made where missing when `make`, else `None` where a directory on the way
/// is missing — and the file's name in it.
fn hold_parent<'a>(
    dir: &Path,
    rel: &'a str,
    make: bool,
) -> Result<Option<(crate::held::Dir, &'a OsStr)>> {
    let names: Vec<&OsStr> = Path::new(rel).iter().collect();
    let Some((leaf, above)) = names.split_last() else {
        anyhow::bail!("{rel} names no file");
    };
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    if make {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut held = match crate::held::Dir::open(dir) {
        Ok(d) => d,
        Err(e) if !make && gone(&e) => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("{} is not a directory omh can enter", dir.display()))
        }
    };
    let mut at = dir.to_path_buf();
    for name in above {
        at.push(name);
        let next = match make {
            true => held.sub_or_create(name),
            false => held.sub(name),
        };
        held = match next {
            Ok(d) => d,
            Err(e) if !make && gone(&e) => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "{} is not a directory omh can enter — a link there is refused, not \
                         followed",
                        at.display()
                    )
                })
            }
        };
    }
    Ok(Some((held, leaf)))
}

/// [`hold_parent`], made.
fn make_parent<'a>(dir: &Path, rel: &'a str) -> Result<(crate::held::Dir, &'a OsStr)> {
    hold_parent(dir, rel, true)?.context("a directory omh made is not there")
}

/// `name` in `dir`, read once: absent is `None`; anything but a regular file
/// within [`CAP`] — a link, a pipe, a directory, a huge file — is refused,
/// never followed or read through.
fn read_raw(dir: &crate::held::Dir, name: &OsStr) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    let file = match dir.file(name) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).context("it is a link or not a regular file, which omh does not follow")
        }
    };
    let mut raw = Vec::new();
    file.take(CAP + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() as u64 <= CAP, "it is larger than {CAP} bytes");
    Ok(Some(raw))
}

/// Bytes [`read_raw`] read, as TOML.
fn parse_doc(raw: &[u8]) -> Result<toml::Table> {
    crate::provider::parse(std::str::from_utf8(raw).context("it is not UTF-8")?)
}

/// [`read_raw`], as TOML.
fn read_doc(dir: &crate::held::Dir, name: &OsStr) -> Result<Option<toml::Table>> {
    read_raw(dir, name)?.map(|raw| parse_doc(&raw)).transpose()
}

/// The account's kept values for `d`, or none yet.
fn kept(account: &Path, d: &Declared) -> Result<toml::Table> {
    let shown = || format!("{}", account.join(KEPT).join(&d.name).display());
    let rel = format!("{KEPT}/{}", d.name);
    let Some((dir, leaf)) = hold_parent(account, &rel, false).with_context(shown)? else {
        return Ok(toml::Table::new());
    };
    read_doc(&dir, leaf)
        .map(Option::unwrap_or_default)
        .with_context(shown)
}

/// What has to happen to the account's file before a session can read its
/// own copy through the link.
#[derive(Debug)]
enum State {
    /// The link, and the marker: nothing to do.
    Ready,
    /// The link, and no marker: a migration that stopped after laying it.
    Finish,
    /// Nothing there yet: lay the link.
    Lay,
    /// The account's real file, before omh ever linked it: its declared keys
    /// become the account's kept values, the rest is dropped by name, and the
    /// bytes are backed up.
    Migrate { dropped: Vec<String>, raw: Vec<u8> },
    /// Something else where the link should be — `found` says what. Linked
    /// again; nothing in it is kept.
    Relink { found: String },
}

#[derive(Debug)]
struct File {
    declared: Declared,
    state: State,
    /// What a session is seeded with: the account's kept values, or what the
    /// migration is about to keep.
    kept: toml::Table,
}

/// An account's preferences, read and checked for a launch — before anything
/// a launch cannot take back, a dry run included. [`Account::apply`] does what
/// they need, after the reuse decision; [`Account::seed`] fills a session's
/// copy.
#[derive(Debug)]
pub struct Account {
    harness: String,
    dir: PathBuf,
    marked: bool,
    files: Vec<File>,
}

/// `adapter`'s preferences on the account at `account`, or `None` for a
/// harness that declares none. Refused — with nothing changed — when the
/// account's file cannot be migrated safely: it is not one omh reads, or it
/// selects a model provider the account keeps nowhere else, which every
/// session would otherwise quietly stop using.
pub fn check(adapter: &Adapter, account: &Path) -> Result<Option<Account>> {
    let Some(prefs) = adapter.preferences.as_ref() else {
        return Ok(None);
    };
    let PrefMount::Link = prefs.mount;
    let kept_dir = account.join(KEPT);
    let marked = kept_dir.join(MARKER).is_file();
    let mut files = Vec::new();
    for declared in prefs.declared() {
        let rel = account_rel(&declared);
        let shown = account.join(rel);
        let found = hold_parent(account, rel, false)?;
        let entry = match &found {
            Some((dir, leaf)) => dir
                .entry(leaf)
                .with_context(|| format!("looking at {}", shown.display()))?,
            None => crate::held::Entry::Absent,
        };
        let (state, kept) = match entry {
            crate::held::Entry::Link(to) if to == target(&declared) => (
                if marked { State::Ready } else { State::Finish },
                self::kept(account, &declared)?,
            ),
            crate::held::Entry::Absent => (State::Lay, self::kept(account, &declared)?),
            crate::held::Entry::File if !marked => {
                let cannot = || format!("{} cannot be migrated", shown.display());
                let (dir, leaf) = found.as_ref().context("a file with no directory")?;
                let raw = read_raw(dir, leaf)
                    .with_context(cannot)?
                    .unwrap_or_default();
                let doc = parse_doc(&raw).with_context(cannot)?;
                refuse_a_lost_provider(adapter, account, &shown, &doc)?;
                let keys: Vec<_> = declared.all().collect();
                let dropped = undeclared(&doc, &keys);
                (State::Migrate { dropped, raw }, pick(&doc, keys))
            }
            crate::held::Entry::File => (
                State::Relink {
                    found: "a file".into(),
                },
                self::kept(account, &declared)?,
            ),
            crate::held::Entry::Link(to) => (
                State::Relink {
                    found: format!("a link to {}", to.display()),
                },
                self::kept(account, &declared)?,
            ),
            crate::held::Entry::Dir | crate::held::Entry::Other => anyhow::bail!(
                "{} is not a file — a session's copy of it is linked there, and omh will not \
                 lay a link over whatever this is. Remove it and launch again",
                shown.display()
            ),
        };
        files.push(File {
            declared,
            state,
            kept,
        });
    }
    Ok(Some(Account {
        harness: adapter.name.as_str().to_string(),
        dir: account.to_path_buf(),
        marked,
        files,
    }))
}

/// Codex reads its model provider from the file a migration takes it out
/// of. A provider the account's provider file does not hold would be lost
/// with it, and every later session would send its turns to OpenAI — so a
/// selected provider other than OpenAI, with no provider file to take its
/// place, refuses the launch, naming the command that keeps it.
fn refuse_a_lost_provider(
    adapter: &Adapter,
    account: &Path,
    shown: &Path,
    doc: &toml::Table,
) -> Result<()> {
    let Some(selected) = doc.get("model_provider").and_then(|p| p.as_str()) else {
        return Ok(());
    };
    let reads = adapter
        .policy
        .as_ref()
        .is_some_and(|p| p.format.reads_provider());
    if selected == "openai" || !reads || crate::provider::read(account)?.is_some() {
        return Ok(());
    }
    let url = doc
        .get("model_providers")
        .and_then(|p| p.get(selected))
        .and_then(|p| p.get("base_url"))
        .and_then(|u| u.as_str())
        .map(|u| format!(", at {}", crate::provider::redacted_url(u)))
        .unwrap_or_default();
    let name = account
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    anyhow::bail!(
        "{} selects `model_provider = {}`{url}, and sessions no longer read it from there — \
         this launch would go to OpenAI instead. Keep it as the account's provider:\n\n    \
         omh auth {} --name {name} --provider {}\n\nor remove `model_provider` from that file \
         to use OpenAI",
        shown.display(),
        crate::out::untrusted(&format!("{selected:?}")),
        adapter.name,
        shown.display(),
    )
}

impl Account {
    /// Whether [`Account::apply`] changes the account's file where a session
    /// from before the link could still be reading it — which it would then
    /// rename over the link, or keep on reading as the shared file.
    pub fn needs_quiet(&self) -> bool {
        !self.marked
            && self
                .files
                .iter()
                .any(|f| !matches!(f.state, State::Ready | State::Finish))
    }

    /// One line for `omh doctor`: what the next launch does to the account's
    /// files, or what the account keeps.
    pub fn summary(&self) -> String {
        let pending = self.pending();
        if !pending.is_empty() {
            return pending.join("; ");
        }
        let kept: Vec<String> = self
            .files
            .iter()
            .map(|f| format!("{} — the account keeps {}", f.declared.name, names(&f.kept)))
            .collect();
        crate::out::untrusted(&format!(
            "{}'s config is per session: {}",
            self.harness,
            kept.join("; ")
        ))
    }

    /// What [`Account::apply`] would do, for a preview and `omh doctor`.
    pub fn pending(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|f| match &f.state {
                State::Migrate { dropped, .. } => Some(format!(
                    "{}'s {} on this account becomes per session at the next launch — keeping {}{}",
                    self.harness,
                    f.declared.name,
                    names(&f.kept),
                    match dropped.is_empty() {
                        true => String::new(),
                        false => format!(", dropping {}", dropped.join(", ")),
                    }
                )),
                State::Relink { found } => Some(format!(
                    "{} is {found}, not the link omh laid — the next launch links it again and \
                     keeps nothing in it",
                    self.dir.join(account_rel(&f.declared)).display()
                )),
                _ => None,
            })
            .map(|s| crate::out::untrusted(&s))
            .collect()
    }

    /// Do what the account's files need, under the account's lock: back the
    /// real file up, keep its declared keys, lay the link, then the marker —
    /// in that order, so each point a run can stop at leaves an account the
    /// next launch finishes. `running` names the sessions still mounting the
    /// account, asked only when something changes where one could read it;
    /// any is refused, and so is not being able to tell.
    pub fn apply(&self, running: &dyn Fn() -> Result<Vec<String>>) -> Result<Vec<String>> {
        if self.files.iter().all(|f| matches!(f.state, State::Ready)) {
            return Ok(Vec::new());
        }
        let (kept_dir, _) = make_parent(&self.dir, &format!("{KEPT}/{LOCK}"))?;
        let _lock = kept_dir
            .lock(OsStr::new(LOCK))
            .with_context(|| format!("locking {}", self.dir.join(KEPT).display()))?;
        if self.needs_quiet() {
            let running = running().map_err(|e| {
                e.context(format!(
                    "omh could not tell whether a session on {} is still running, and \
                     changing {}'s config under one would let it rename over the link",
                    self.dir.display(),
                    self.harness
                ))
            })?;
            anyhow::ensure!(
                running.is_empty(),
                "{} is in use by {}, started before {}'s config was per session — stop it \
                 first (`omh sNN down`), and launch again",
                self.dir.display(),
                running.join(", "),
                self.harness
            );
        }
        let mut notes = Vec::new();
        for f in &self.files {
            let rel = account_rel(&f.declared);
            let (dir, leaf) = make_parent(&self.dir, rel)?;
            let name = OsStr::new(&f.declared.name);
            match &f.state {
                State::Ready => continue,
                State::Finish | State::Lay => {}
                State::Migrate { dropped, raw } => {
                    let backup = format!("{}.before-omh", f.declared.name);
                    kept_dir
                        .replace_private(OsStr::new(&backup), raw)
                        .with_context(|| format!("backing up {}", self.dir.join(rel).display()))?;
                    kept_dir.replace_private(name, toml::to_string(&f.kept)?.as_bytes())?;
                    notes.push(crate::out::untrusted(&format!(
                        "{}'s {} is per session from now on — kept {}{}; the file as it was is \
                         at {}",
                        self.harness,
                        f.declared.name,
                        names(&f.kept),
                        match dropped.is_empty() {
                            true => String::new(),
                            false => format!(", dropped {}", dropped.join(", ")),
                        },
                        self.dir.join(KEPT).join(&backup).display()
                    )));
                }
                State::Relink { found } => notes.push(crate::out::untrusted(&format!(
                    "{} was {found}, not the link omh laid — a session put it there; linked \
                     again, and nothing in it kept",
                    self.dir.join(rel).display()
                ))),
            }
            dir.link_over(leaf, &target(&f.declared))
                .with_context(|| format!("linking {}", self.dir.join(rel).display()))?;
        }
        kept_dir.replace(OsStr::new(MARKER), b"")?;
        Ok(notes)
    }

    /// Fill a session's copy: on a kept home, once — a relaunch continues
    /// from the session's own `live`, unless the session's account has
    /// changed, which keeps what it changed into its old account first and
    /// then seeds from this one; a scratch home every time. Written after the
    /// reuse decision and before the container starts: never into a directory
    /// a running container has mounted, and never missing when one mounts it.
    pub fn seed(&self, adapter: &Adapter, home: &Home) -> Result<Vec<String>> {
        let root = home.path();
        std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
        let mut notes = Vec::new();
        let recorded = std::fs::read_to_string(root.join(RECORD)).ok();
        let this = self.dir.display().to_string();
        let whole = self
            .files
            .iter()
            .all(|f| live(home).join(&f.declared.name).is_file());
        if let Home::Kept(_) = home {
            match recorded.as_deref() {
                Some(r) if r == this && whole => return Ok(notes),
                Some(r) if r != this => {
                    notes.extend(keep(adapter, home)?);
                    notes.push(format!(
                        "this session's {} preferences were seeded again, from {}",
                        self.harness,
                        self.dir.display()
                    ));
                }
                _ => {}
            }
        }
        for part in ["base", "reviewed", "live"] {
            let dir = root.join(part);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("clearing {}", dir.display())),
            }
            std::fs::create_dir_all(&dir)?;
            let held = crate::held::Dir::open(&dir)?;
            for f in &self.files {
                held.replace(
                    OsStr::new(&f.declared.name),
                    toml::to_string(&f.kept)?.as_bytes(),
                )?;
            }
        }
        crate::held::Dir::open(root)?.replace(OsStr::new(RECORD), this.as_bytes())?;
        Ok(notes)
    }
}

/// The keys `doc` holds, dotted, or `nothing`.
fn names(doc: &toml::Table) -> String {
    let mut out = Vec::new();
    fn walk(t: &toml::Table, at: &mut KeyPath, out: &mut Vec<String>) {
        for (k, v) in t {
            at.push(k.clone());
            match v {
                toml::Value::Table(inner) if !inner.is_empty() => walk(inner, at, out),
                _ => out.push(dotted(at)),
            }
            at.pop();
        }
    }
    walk(doc, &mut Vec::new(), &mut out);
    match out.is_empty() {
        true => "nothing".into(),
        false => out.join(", "),
    }
}

/// Until the review (#197 §4): what a session changed among its declared
/// keys, kept into its account without asking — changed keys only, last
/// writer wins, each value another session had kept since named as replaced.
/// Run when the harness exits in the terminal, at `down` and at `rm`, and
/// after a login. What the session wrote that is not a preference is named
/// and left where it is.
///
/// `live` is read as the sandbox's: through `held`, capped, regular files
/// only. One that cannot be read is an error — "could not tell what this
/// session changed" — never "nothing changed".
pub fn keep(adapter: &Adapter, home: &Home) -> Result<Vec<String>> {
    let Some(prefs) = adapter.preferences.as_ref() else {
        return Ok(Vec::new());
    };
    let root = home.path();
    let account = match std::fs::read_to_string(root.join(RECORD)) {
        Ok(a) => PathBuf::from(a),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", root.join(RECORD).display()))
        }
    };
    let shown_account = account
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (kept_dir, _) = make_parent(&account, &format!("{KEPT}/{LOCK}"))?;
    let _lock = kept_dir.lock(OsStr::new(LOCK))?;
    let live_dir = crate::held::Dir::open(&live(home));
    let mut notes = Vec::new();
    for d in prefs.declared() {
        let name = OsStr::new(&d.name);
        let could_not_tell = |e: anyhow::Error| {
            e.context(format!(
                "could not tell what this session changed in its {} — nothing was kept",
                d.name
            ))
        };
        let live_dir = live_dir
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .map_err(could_not_tell)?;
        let live = read_doc(live_dir, name)
            .map_err(could_not_tell)?
            .unwrap_or_default();
        let base =
            read_doc(&crate::held::Dir::open(&root.join("base"))?, name)?.unwrap_or_default();
        let mut account_kept = read_doc(&kept_dir, name)?.unwrap_or_default();
        let changes = changed(&base, &live, d.all());
        let mut kept_keys = Vec::new();
        for c in &changes {
            if get(&account_kept, &c.key) != c.from.as_ref() {
                notes.push(crate::out::untrusted(&format!(
                    "{} replaces the value another session kept on account `{shown_account}`",
                    dotted(&c.key)
                )));
            }
            set(&mut account_kept, &c.key, c.to.clone());
            kept_keys.push(dotted(&c.key));
        }
        if !changes.is_empty() {
            kept_dir.replace_private(name, toml::to_string(&account_kept)?.as_bytes())?;
            notes.push(crate::out::untrusted(&format!(
                "kept {} {} preference{} on account `{shown_account}`: {}",
                changes.len(),
                adapter.name,
                if changes.len() == 1 { "" } else { "s" },
                kept_keys.join(", ")
            )));
        }
        let keys: Vec<_> = d.all().collect();
        let synced = toml::to_string(&pick(&live, keys.iter().copied()))?;
        for part in ["base", "reviewed"] {
            crate::held::Dir::open(&root.join(part))?.replace(name, synced.as_bytes())?;
        }
        let left = undeclared(&live, &keys);
        if !left.is_empty() {
            notes.push(crate::out::untrusted(&format!(
                "not kept, not preferences: {}",
                left.join(", ")
            )));
        }
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::key_path;

    fn table(raw: &str) -> toml::Table {
        toml::from_str(raw).unwrap()
    }

    fn keys(list: &[&str]) -> Vec<KeyPath> {
        list.iter().map(|k| key_path(k).unwrap()).collect()
    }

    const ACCOUNT: &str = r#"
model = "gpt-5.5"
developer_instructions = "planted"
model_provider = "evil"

[tui]
theme = "dark"
notifications = true
keymap = { quit = "q" }

[mcp_servers.x]
command = "/tmp/x"

[model_providers.evil]
base_url = "https://example.invalid"
"#;

    /// Only declared keys, nested or whole, and nothing around them.
    #[test]
    fn what_is_kept_is_the_declared_keys_alone() {
        let declared = keys(&["model", "tui.theme", "tui.keymap", "service_tier"]);
        let kept = pick(&table(ACCOUNT), &declared);
        assert_eq!(
            kept,
            table("model = \"gpt-5.5\"\n[tui]\ntheme = \"dark\"\nkeymap = { quit = \"q\" }\n")
        );
    }

    /// A change is a declared key whose value differs, absent included; a key
    /// nobody declared is not one, however it changed.
    #[test]
    fn a_change_is_a_declared_key_whose_value_differs() {
        let declared = keys(&["model", "tui.theme", "service_tier"]);
        let before = table("model = \"a\"\nservice_tier = \"flex\"\n");
        let after = table("model = \"b\"\ndeveloper_instructions = \"x\"\n[tui]\ntheme = \"t\"\n");
        let got = changed(&before, &after, &declared);
        assert_eq!(
            got,
            vec![
                Change {
                    key: keys(&["model"]).remove(0),
                    from: Some("a".into()),
                    to: Some("b".into()),
                },
                Change {
                    key: keys(&["tui.theme"]).remove(0),
                    from: None,
                    to: Some("t".into()),
                },
                Change {
                    key: keys(&["service_tier"]).remove(0),
                    from: Some("flex".into()),
                    to: None,
                },
            ]
        );
        assert!(changed(&before, &before, &declared).is_empty());
    }

    /// Setting builds the tables on the way; removing takes an emptied table
    /// with it, and leaves a missing key missing.
    #[test]
    fn a_removed_key_leaves_no_empty_table() {
        let mut doc = toml::Table::new();
        let theme = keys(&["tui.theme"]).remove(0);
        set(&mut doc, &theme, Some("t".into()));
        assert_eq!(doc, table("[tui]\ntheme = \"t\"\n"));
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc}");
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc}");
        let mut doc = table("tui = 1\n");
        set(&mut doc, &theme, Some("t".into()));
        assert_eq!(doc, table("[tui]\ntheme = \"t\"\n"));
    }

    /// What is left out is named, a table no declared key reaches named
    /// whole, and nothing is named by value.
    #[test]
    fn what_is_left_out_is_named_by_key() {
        let declared = keys(&["model", "tui.theme", "tui.keymap"]);
        let refs: Vec<&KeyPath> = declared.iter().collect();
        let mut names = undeclared(&table(ACCOUNT), &refs);
        names.sort();
        assert_eq!(
            names,
            vec![
                "developer_instructions",
                "mcp_servers",
                "model_provider",
                "model_providers",
                "tui.notifications",
            ]
        );
        let odd = table("[notice]\n\"hide_gpt-5.1-codex-max_migration_prompt\" = true\n");
        assert_eq!(undeclared(&odd, &[]), vec!["notice".to_string()]);
        assert_eq!(
            dotted(&key_path("notice.\"a.b\"").unwrap()),
            "notice.\"a.b\""
        );
    }

    // ── On disk ──

    fn codex() -> Adapter {
        Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            "codex",
        )
        .unwrap()
    }

    fn none_running() -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// An account dir under a temp root, named `work`, and its own file.
    fn account() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/codex/work");
        let file = account.join(".codex/config.toml");
        (root, account, file)
    }

    fn link_of(path: &Path) -> Option<PathBuf> {
        std::fs::read_link(path).ok()
    }

    const TARGET: &str = "/omh/prefs/config.toml";

    /// A new account's file is the link from its first launch, with the
    /// marker that says a real file there later is not omh's.
    #[test]
    fn a_new_account_is_linked_from_the_start() {
        let (_root, account, file) = account();
        let checked = check(&codex(), &account).unwrap().unwrap();
        assert!(!account.exists(), "checking made {}", account.display());
        checked.apply(&none_running).unwrap();
        assert_eq!(link_of(&file), Some(PathBuf::from(TARGET)));
        assert!(account.join("preferences/linked").is_file());
        assert!(check(&codex(), &account)
            .unwrap()
            .unwrap()
            .apply(&|| { panic!("a ready account asks about running sessions") })
            .unwrap()
            .is_empty());
    }

    /// The account's real file, once: its declared keys kept, the rest
    /// dropped and named by key, the bytes backed up `0600` where Codex never
    /// loads a profile from — and then the link.
    #[test]
    fn a_real_file_is_migrated_once_and_what_is_dropped_is_named() {
        use std::os::unix::fs::PermissionsExt;
        let (_root, account, file) = account();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let original = "model = \"gpt-5.5\"\ndeveloper_instructions = \"sk-planted\"\n\
                        [tui]\ntheme = \"dark\"\n[mcp_servers.x]\ncommand = \"/tmp/x\"\n";
        std::fs::write(&file, original).unwrap();

        let checked = check(&codex(), &account).unwrap().unwrap();
        let pending = checked.pending().join("\n");
        assert!(
            pending.contains("developer_instructions") && pending.contains("mcp_servers"),
            "{pending}"
        );
        let notes = checked.apply(&none_running).unwrap().join("\n");
        assert!(
            notes.contains("dropped developer_instructions, mcp_servers"),
            "{notes}"
        );
        assert!(!notes.contains("sk-planted"), "a value was named: {notes}");

        assert_eq!(link_of(&file), Some(PathBuf::from(TARGET)));
        let kept = std::fs::read_to_string(account.join("preferences/config.toml")).unwrap();
        assert_eq!(
            toml::from_str::<toml::Table>(&kept).unwrap(),
            table("model = \"gpt-5.5\"\n[tui]\ntheme = \"dark\"\n")
        );
        let backup = account.join("preferences/config.toml.before-omh");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let mode = std::fs::metadata(&backup).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!backup.to_string_lossy().ends_with(".config.toml"));
    }

    /// A launch stopped at any point leaves an account the next one finishes:
    /// before the link, the real file is migrated again to the same result;
    /// after it, only the marker is left to write.
    #[test]
    fn a_migration_stopped_part_way_is_finished_by_the_next_launch() {
        let (_root, account, file) = account();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "model = \"a\"\nnotify = [\"x\"]\n").unwrap();
        // Stopped after keeping, before the link.
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(account.join("preferences/config.toml"), "model = \"a\"\n").unwrap();
        check(&codex(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap();
        assert_eq!(link_of(&file), Some(PathBuf::from(TARGET)));
        assert_eq!(
            std::fs::read_to_string(account.join("preferences/config.toml")).unwrap(),
            "model = \"a\"\n"
        );
        // Stopped after the link, before the marker.
        std::fs::remove_file(account.join("preferences/linked")).unwrap();
        let checked = check(&codex(), &account).unwrap().unwrap();
        assert!(
            !checked.needs_quiet(),
            "the link is there: nothing a session reads changes"
        );
        checked.apply(&|| panic!("asked")).unwrap();
        assert!(account.join("preferences/linked").is_file());
    }

    /// A provider the account would lose with the file refuses the launch,
    /// naming the command that keeps it — and nothing is touched. OpenAI, or
    /// a provider the account's provider file already holds, migrates.
    #[test]
    fn a_provider_with_nowhere_to_go_refuses_before_anything_changes() {
        let (_root, account, file) = account();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let azure = "model_provider = \"azure\"\n[model_providers.azure]\nname = \"Azure\"\n\
                     base_url = \"https://u:pa55@example.invalid/openai?key=k\"\n";
        std::fs::write(&file, azure).unwrap();
        let err = format!("{:#}", check(&codex(), &account).unwrap_err());
        assert!(
            err.contains("azure")
                && err.contains("omh auth codex --name work --provider")
                && err.contains(&file.display().to_string()),
            "{err}"
        );
        assert!(
            !err.contains("pa55") && !err.contains("key=k"),
            "a secret: {err}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), azure);
        assert!(!account.join("preferences").exists());

        crate::provider::write(
            &account,
            &table("model_provider = \"azure\"\n[model_providers.azure]\nname = \"Azure\"\n"),
        )
        .unwrap();
        check(&codex(), &account).unwrap().unwrap();
        std::fs::remove_file(account.join("provider.toml")).unwrap();
        std::fs::write(&file, "model_provider = \"openai\"\n").unwrap();
        check(&codex(), &account).unwrap().unwrap();
    }

    /// A session from before the link still mounting the account would
    /// rename over it, or keep reading the shared file: one running refuses
    /// the migration by name, and so does not being able to tell.
    #[test]
    fn a_running_session_or_not_knowing_stops_the_migration() {
        let (_root, account, file) = account();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "model = \"a\"\n").unwrap();
        let checked = check(&codex(), &account).unwrap().unwrap();
        let err = format!(
            "{:#}",
            checked
                .apply(&|| Ok(vec!["omh-repo-s01".into()]))
                .unwrap_err()
        );
        assert!(err.contains("omh-repo-s01"), "{err}");
        let err = format!(
            "{:#}",
            checked
                .apply(&|| anyhow::bail!("no labels on this runtime"))
                .unwrap_err()
        );
        assert!(err.contains("could not tell"), "{err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "model = \"a\"\n");
        assert!(!account.join("preferences/config.toml").exists());
    }

    /// After the marker, a real file where the link goes was put there by a
    /// session: linked again, named, and nothing in it kept. A link elsewhere
    /// the same.
    #[test]
    fn after_the_link_a_real_file_is_tampering_and_never_kept() {
        let (_root, account, file) = account();
        check(&codex(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap();
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, "model = \"planted\"\n").unwrap();
        let notes = check(&codex(), &account)
            .unwrap()
            .unwrap()
            .apply(&|| panic!("asked"))
            .unwrap()
            .join("\n");
        assert!(
            notes.contains("a file") && notes.contains("nothing in it kept"),
            "{notes}"
        );
        assert_eq!(link_of(&file), Some(PathBuf::from(TARGET)));
        assert!(kept(&account, &codex().preferences.unwrap().declared()[0])
            .unwrap()
            .is_empty());
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &file).unwrap();
        let notes = check(&codex(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap()
            .join("\n");
        assert!(notes.contains("a link to /etc/passwd"), "{notes}");
        assert_eq!(link_of(&file), Some(PathBuf::from(TARGET)));
    }

    /// The directory the file is in is held from the account down: a link
    /// there is refused, never followed to lay a link or read a file.
    #[test]
    fn a_link_above_the_file_is_refused() {
        let (root, account, file) = account();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("config.toml"), "model = \"x\"\n").unwrap();
        std::fs::create_dir_all(&account).unwrap();
        std::os::unix::fs::symlink(&elsewhere, account.join(".codex")).unwrap();
        let err = format!("{:#}", check(&codex(), &account).unwrap_err());
        assert!(err.contains(".codex"), "{err}");
        assert!(!file.parent().unwrap().join("config.toml").is_symlink());
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("config.toml")).unwrap(),
            "model = \"x\"\n"
        );
    }

    /// A session is seeded once from the account, and a relaunch continues
    /// from its own copy; another account keeps what the session changed into
    /// the old one first, then seeds from the new.
    #[test]
    fn a_session_is_seeded_once_and_again_only_for_another_account() {
        let (root, work, _) = account();
        let personal = root.path().join("creds/codex/personal");
        for (acct, model) in [(&work, "w"), (&personal, "p")] {
            std::fs::create_dir_all(acct.join("preferences")).unwrap();
            std::fs::write(
                acct.join("preferences/config.toml"),
                format!("model = \"{model}\"\n"),
            )
            .unwrap();
        }
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/codex"));
        let at_live = live(&home).join("config.toml");
        let seed = |acct: &Path| {
            let checked = check(&codex(), acct).unwrap().unwrap();
            checked.apply(&none_running).unwrap();
            checked.seed(&codex(), &home).unwrap()
        };
        seed(&work);
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"w\"\n"
        );
        std::fs::write(&at_live, "model = \"mine\"\n").unwrap();
        seed(&work);
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"mine\"\n"
        );

        let notes = seed(&personal).join("\n");
        assert!(
            notes.contains("kept 1 codex preference on account `work`: model"),
            "{notes}"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("preferences/config.toml")).unwrap(),
            "model = \"mine\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"p\"\n"
        );
    }

    /// What a session changed among its declared keys reaches the account,
    /// alone: what it wrote that is not a preference is named and left; a
    /// value another session kept meanwhile is named as replaced; and a live
    /// copy the sandbox turned into a link is "could not tell", with the
    /// account untouched.
    #[test]
    fn what_a_session_keeps_is_its_changed_preferences_alone() {
        let (root, work, _) = account();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/codex"));
        std::fs::create_dir_all(work.join("preferences")).unwrap();
        std::fs::write(work.join("preferences/config.toml"), "model = \"a\"\n").unwrap();
        let checked = check(&codex(), &work).unwrap().unwrap();
        checked.apply(&none_running).unwrap();
        checked.seed(&codex(), &home).unwrap();
        std::fs::write(
            live(&home).join("config.toml"),
            "model = \"b\"\ndeveloper_instructions = \"planted\"\n[tui]\ntheme = \"t\"\n",
        )
        .unwrap();
        // Another session kept a model meanwhile.
        std::fs::write(work.join("preferences/config.toml"), "model = \"other\"\n").unwrap();

        let notes = keep(&codex(), &home).unwrap().join("\n");
        assert!(
            notes.contains("model replaces the value another session kept"),
            "{notes}"
        );
        assert!(
            notes.contains("not kept, not preferences: developer_instructions"),
            "{notes}"
        );
        assert!(!notes.contains("planted"), "{notes}");
        let kept = std::fs::read_to_string(work.join("preferences/config.toml")).unwrap();
        assert_eq!(
            toml::from_str::<toml::Table>(&kept).unwrap(),
            table("model = \"b\"\n[tui]\ntheme = \"t\"\n")
        );
        // Kept once: the same change is not kept again.
        std::fs::write(work.join("preferences/config.toml"), "model = \"later\"\n").unwrap();
        keep(&codex(), &home).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("preferences/config.toml")).unwrap(),
            "model = \"later\"\n"
        );

        std::fs::remove_file(live(&home).join("config.toml")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", live(&home).join("config.toml")).unwrap();
        let err = format!("{:#}", keep(&codex(), &home).unwrap_err());
        assert!(err.contains("could not tell"), "{err}");
        assert_eq!(
            std::fs::read_to_string(work.join("preferences/config.toml")).unwrap(),
            "model = \"later\"\n"
        );
    }
}
