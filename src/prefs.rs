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
//!   live/     what the harness reads and writes: mounted at GUEST (`link`),
//!             file by file where the harness reads each (`file`), or whole
//!             at the adapter's `root` (`dir`), beside what else the harness
//!             keeps there
//!   rest/     for a file that is the session's copy of an account file
//!             (`[policy] copy`), a digest of what the copy holds beside its
//!             preferences
//! ```
//!
//! Each format is read into one model, JSON's, which TOML's and YAML's map
//! into: a TOML datetime is carried as the tagged table `toml` serialises it
//! as, and written back as a datetime; YAML is read as omp reads it
//! (`provider::omp::parse`), a file with no document as no settings.

/// Where a container finds its session's `live/`, for a `link` adapter: the
/// account's file is a link to the same name here.
pub const GUEST: &str = "/omh/prefs";

pub mod review;

use crate::adapter::{Adapter, Declared, KeyPath, PrefMount};
use crate::home::Home;
use anyhow::{Context, Result};
use serde::Deserialize as _;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A preferences file's content, whichever format it is written in.
pub type Doc = serde_json::Map<String, serde_json::Value>;

/// A value in a [`Doc`]: opaque — omh compares, carries and shows it, never
/// reads inside it.
pub type Value = serde_json::Value;

pub use crate::adapter::PrefFormat as Format;

impl Format {
    /// `raw` as a document. An empty JSON file is the empty document — what
    /// Claude itself reads it as, and `auth::prepare`'s placeholder.
    pub fn parse(self, raw: &[u8]) -> Result<Doc> {
        let text = std::str::from_utf8(raw).context("it is not UTF-8")?;
        let value = match self {
            Format::Toml => serde_json::to_value(crate::provider::parse(text)?)?,
            Format::Json if text.trim().is_empty() => return Ok(Doc::new()),
            Format::Json => serde_json::from_str(text).context("it is not JSON")?,
            // As omp reads it: a file with no document is no settings.
            Format::Yaml => {
                match crate::provider::omp::parse(text).map_err(|e| anyhow::anyhow!("it {e}"))? {
                    Value::Null => return Ok(Doc::new()),
                    v => v,
                }
            }
        };
        match value {
            Value::Object(doc) => Ok(doc),
            _ => anyhow::bail!("it is not a JSON object"),
        }
    }

    /// `doc`, written in this format.
    pub fn render(self, doc: &Doc) -> Result<Vec<u8>> {
        match self {
            Format::Toml => {
                let table = toml::Table::deserialize(Value::Object(doc.clone()))
                    .context("it cannot be written as TOML")?;
                Ok(toml::to_string(&table)?.into_bytes())
            }
            Format::Json => {
                let mut out = serde_json::to_vec_pretty(doc)?;
                out.push(b'\n');
                Ok(out)
            }
            Format::Yaml => Ok(serde_saphyr::to_string(&Value::Object(doc.clone()))
                .context("it cannot be written as YAML")?
                .into_bytes()),
        }
    }
}

/// The value at `key`, if `doc` has one.
pub fn get<'a>(doc: &'a Doc, key: &KeyPath) -> Option<&'a Value> {
    let (last, above) = key.split_last()?;
    let mut table = doc;
    for name in above {
        table = table.get(name)?.as_object()?;
    }
    table.get(last)
}

/// `doc` with `key` set to `value`, or removed when it is `None` — and a
/// table emptied by the removal removed with it, so a deletion leaves nothing
/// behind. A value in the way of a table is replaced by one.
pub fn set(doc: &mut Doc, key: &KeyPath, value: Option<Value>) {
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
    if value.is_none() && !doc.get(first).is_some_and(Value::is_object) {
        return;
    }
    let entry = doc
        .entry(first.clone())
        .or_insert_with(|| Value::Object(Doc::new()));
    if !entry.is_object() {
        *entry = Value::Object(Doc::new());
    }
    if let Value::Object(table) = entry {
        set(table, &rest.to_vec(), value);
        if table.is_empty() {
            doc.remove(first);
        }
    }
}

/// The declared keys of `doc`, and nothing else: what an account keeps and a
/// session is seeded with. Built key by key from parsed values, never copied
/// as bytes.
pub fn pick<'a>(doc: &Doc, keys: impl IntoIterator<Item = &'a KeyPath>) -> Doc {
    let mut out = Doc::new();
    for key in keys {
        if let Some(v) = get(doc, key) {
            set(&mut out, key, Some(v.clone()));
        }
    }
    out
}

/// `rest` with each of `keys` as `values` has it — absent where `values` has
/// none: a session's copy of a file that holds more than its preferences.
fn compose<'a>(rest: &Doc, values: &Doc, keys: impl IntoIterator<Item = &'a KeyPath>) -> Doc {
    let mut out = rest.clone();
    for key in keys {
        set(&mut out, key, get(values, key).cloned());
    }
    out
}

/// A declared key whose value differs between two readings — *absent* is a
/// value: a key removed is a change to `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub key: KeyPath,
    pub from: Option<Value>,
    pub to: Option<Value>,
}

/// What changed from `before` to `after`, declared key by declared key.
pub fn changed<'a>(
    before: &Doc,
    after: &Doc,
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
pub fn undeclared(doc: &Doc, keys: &[&KeyPath]) -> Vec<String> {
    fn walk(table: &Doc, at: &mut KeyPath, keys: &[&KeyPath], out: &mut Vec<String>) {
        for (name, value) in table {
            at.push(name.clone());
            let covered = keys.iter().any(|k| at.starts_with(k));
            let inside = keys.iter().any(|k| k.len() > at.len() && k.starts_with(at));
            match (covered, inside, value) {
                (true, _, _) => {}
                (false, true, Value::Object(t)) => walk(t, at, keys, out),
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

/// The most of a preferences file omh reads: as much as `auth` reads of a
/// `~/.claude.json`, which a session's copy of one is. A bound on what a
/// session can make omh read.
const CAP: u64 = 16 << 20;

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

/// In a session's preferences home: a digest of each copy's rest, by name.
const REST: &str = "rest";

/// The directory a container mounts at [`GUEST`], or whose files it mounts
/// one by one.
pub fn live(home: &Home) -> PathBuf {
    home.path().join("live")
}

/// The account's own file, relative to the account directory — and to the
/// home, where the harness reads it.
pub fn account_rel(d: &Declared) -> &str {
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

/// [`read_raw`], parsed as `format`.
fn read_doc(dir: &crate::held::Dir, name: &OsStr, format: Format) -> Result<Option<Doc>> {
    read_raw(dir, name)?
        .map(|raw| format.parse(&raw))
        .transpose()
}

/// The account's kept values for `d`, or none yet.
fn kept(account: &Path, d: &Declared) -> Result<Doc> {
    kept_if_any(account, d).map(Option::unwrap_or_default)
}

/// The account's kept values for `d`, or `None` where it keeps no file for
/// it yet.
fn kept_if_any(account: &Path, d: &Declared) -> Result<Option<Doc>> {
    let shown = || format!("{}", account.join(KEPT).join(&d.name).display());
    let rel = format!("{KEPT}/{}", d.name);
    let Some((dir, leaf)) = hold_parent(account, &rel, false).with_context(shown)? else {
        return Ok(None);
    };
    read_doc(&dir, leaf, d.format()).with_context(shown)
}

/// What has to happen to the account's file before a session can read its
/// own copy.
#[derive(Debug)]
enum State {
    /// Nothing to do: for `link`, the link and the marker; for `file`, the
    /// account keeps a file for it.
    Ready,
    /// The link, and no marker: a migration that stopped after laying it.
    Finish,
    /// Nothing there yet: lay the link.
    Lay,
    /// The account's real file, before omh ever linked it: its declared keys
    /// become the account's kept values, the rest is dropped by name, and the
    /// bytes are backed up. `from`: where the file is absent, the one the
    /// harness read in its place (relative to the account), which goes once
    /// it is backed up — nothing reads it once the link is there.
    Migrate {
        dropped: Vec<String>,
        raw: Vec<u8>,
        from: Option<String>,
    },
    /// Something else where the link should be — `found` says what. Linked
    /// again; nothing in it is kept.
    Relink { found: String },
    /// `file`: the account keeps nothing for it yet. Its declared keys in the
    /// account's own file, where the harness wrote them before sessions had
    /// their own, become the account's kept values; the file is left as it is.
    Import,
}

#[derive(Debug)]
struct File {
    declared: Declared,
    state: State,
    /// What a session is seeded with: the account's kept values, or what the
    /// migration or the import is about to keep.
    kept: Doc,
}

/// An account's preferences, read and checked for a launch — before anything
/// a launch cannot take back, a dry run included. [`Account::apply`] does what
/// they need, after the reuse decision; [`Account::seed`] fills a session's
/// copy.
#[derive(Debug)]
pub struct Account {
    harness: String,
    dir: PathBuf,
    mount: PrefMount,
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
    let kept_dir = account.join(KEPT);
    let marked = kept_dir.join(MARKER).is_file();
    let mut files = Vec::new();
    for declared in prefs.declared() {
        let (state, kept) = match prefs.mount {
            PrefMount::Link => check_link(adapter, account, &declared, marked)?,
            PrefMount::File => check_file(account, &declared)?,
            PrefMount::Dir => (State::Ready, self::kept(account, &declared)?),
        };
        // The declared keys alone, whatever the file holds beside them.
        let kept = pick(&kept, declared.all());
        files.push(File {
            declared,
            state,
            kept,
        });
    }
    Ok(Some(Account {
        harness: adapter.name.as_str().to_string(),
        dir: account.to_path_buf(),
        mount: prefs.mount,
        marked,
        files,
    }))
}

/// A `link` file: what is where the link goes, and what a session is seeded
/// with.
fn check_link(
    adapter: &Adapter,
    account: &Path,
    declared: &Declared,
    marked: bool,
) -> Result<(State, Doc)> {
    let rel = account_rel(declared);
    let shown = account.join(rel);
    let found = hold_parent(account, rel, false)?;
    let entry = match &found {
        Some((dir, leaf)) => dir
            .entry(leaf)
            .with_context(|| format!("looking at {}", shown.display()))?,
        None => crate::held::Entry::Absent,
    };
    Ok(match entry {
        crate::held::Entry::Link(to) if to == target(declared) => (
            if marked { State::Ready } else { State::Finish },
            self::kept(account, declared)?,
        ),
        crate::held::Entry::Absent if !marked => match in_its_place(account, declared)? {
            Some((from, raw)) => {
                let format = Format::of(&from).unwrap_or(declared.format());
                migrate(
                    adapter,
                    account,
                    declared,
                    &account.join(&from),
                    raw,
                    format,
                    Some(from),
                )?
            }
            None => (State::Lay, self::kept(account, declared)?),
        },
        crate::held::Entry::Absent => (State::Lay, self::kept(account, declared)?),
        crate::held::Entry::File if !marked => {
            let (dir, leaf) = found.as_ref().context("a file with no directory")?;
            let raw = read_raw(dir, leaf)
                .with_context(|| format!("{} cannot be migrated", shown.display()))?
                .unwrap_or_default();
            migrate(
                adapter,
                account,
                declared,
                &shown,
                raw,
                declared.format(),
                None,
            )?
        }
        crate::held::Entry::File => (
            State::Relink {
                found: "a file".into(),
            },
            self::kept(account, declared)?,
        ),
        crate::held::Entry::Link(to) => (
            State::Relink {
                found: format!("a link to {}", to.display()),
            },
            self::kept(account, declared)?,
        ),
        crate::held::Entry::Dir | crate::held::Entry::Other => anyhow::bail!(
            "{} is not a file — a session's copy of it is linked there, and omh will not \
             lay a link over whatever this is. Remove it and launch again",
            shown.display()
        ),
    })
}

/// The file the harness reads in `declared`'s place while it is absent — the
/// first of its fallbacks the account holds, relative to the account — and
/// its bytes, read as the agent's: one that is not a regular file is refused
/// rather than passed over, since the harness reads through it.
fn in_its_place(account: &Path, declared: &Declared) -> Result<Option<(String, Vec<u8>)>> {
    for fallback in &declared.fallback {
        let rel = fallback.trim_start_matches("$HOME/");
        let cannot = || format!("{} cannot be migrated", account.join(rel).display());
        let Some((dir, leaf)) = hold_parent(account, rel, false).with_context(cannot)? else {
            continue;
        };
        if let Some(raw) = read_raw(&dir, leaf).with_context(cannot)? {
            return Ok(Some((rel.to_string(), raw)));
        }
    }
    Ok(None)
}

/// A real file's migration: its declared keys kept, the rest dropped by name.
fn migrate(
    adapter: &Adapter,
    account: &Path,
    declared: &Declared,
    shown: &Path,
    raw: Vec<u8>,
    format: Format,
    from: Option<String>,
) -> Result<(State, Doc)> {
    let doc = format
        .parse(&raw)
        .with_context(|| format!("{} cannot be migrated", shown.display()))?;
    refuse_a_lost_provider(adapter, account, shown, &doc)?;
    let keys: Vec<_> = declared.all().collect();
    let dropped = undeclared(&doc, &keys);
    Ok((State::Migrate { dropped, raw, from }, pick(&doc, keys)))
}

/// A `file` file: the account's kept values, or — the first time — the
/// declared keys of the account's own file, read as the agent's: through
/// `held`, capped, a regular file or nothing.
fn check_file(account: &Path, declared: &Declared) -> Result<(State, Doc)> {
    if let Some(kept) = kept_if_any(account, declared)? {
        return Ok((State::Ready, kept));
    }
    let rel = account_rel(declared);
    let shown = account.join(rel);
    let own = match hold_parent(account, rel, false)? {
        Some((dir, leaf)) => read_raw(&dir, leaf)
            .and_then(|raw| {
                raw.map(|r| declared.format().parse(&r))
                    .transpose()
                    .map(Option::unwrap_or_default)
            })
            .with_context(|| {
                format!(
                    "{} cannot be read for its preferences — remove it, and launch again",
                    shown.display()
                )
            })?,
        None => Doc::new(),
    };
    Ok((State::Import, pick(&own, declared.all())))
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
    doc: &Doc,
) -> Result<()> {
    let Some(selected) = doc.get("model_provider").and_then(|p| p.as_str()) else {
        return Ok(());
    };
    let reads = adapter
        .provider
        .as_ref()
        .is_some_and(|p| p.format == crate::adapter::ProviderFormat::CodexConfig);
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
         omh account login {} --name {name} --provider {}\n\nor remove `model_provider` from that file \
         to use OpenAI",
        shown.display(),
        crate::out::untrusted(&format!("{selected:?}")),
        adapter.name,
        shown.display(),
    )
}

/// Bytes a preferences file is written with: readable by the user alone for
/// a `file` adapter, whose copy of `~/.claude.json` holds the account's
/// identity, and as they always were for `link` and `dir`.
fn place(dir: &crate::held::Dir, name: &OsStr, body: &[u8], mount: PrefMount) -> Result<()> {
    match mount {
        PrefMount::File => dir.replace_private(name, body)?,
        PrefMount::Link => dir.replace(name, body)?,
        PrefMount::Dir => dir.replace(name, body)?,
    }
    Ok(())
}

impl Account {
    /// Whether [`Account::apply`] changes the account's file where a session
    /// from before the link could still be reading it — which it would then
    /// rename over the link, or keep on reading as the shared file.
    pub fn needs_quiet(&self) -> bool {
        !self.marked
            && self.mount == PrefMount::Link
            && self
                .files
                .iter()
                .any(|f| !matches!(f.state, State::Ready | State::Finish))
    }

    /// The account files a session's copy of which this harness reads as
    /// its preferences file — relative to the home — so that the copy is
    /// written here, around the session's own values, and nowhere else.
    pub fn copies(&self) -> Vec<String> {
        match self.mount {
            PrefMount::File => self
                .files
                .iter()
                .map(|f| account_rel(&f.declared).to_string())
                .collect(),
            PrefMount::Link | PrefMount::Dir => Vec::new(),
        }
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
            "{}'s preferences are per session: {}",
            self.harness,
            kept.join("; ")
        ))
    }

    /// What [`Account::apply`] would do, for a preview and `omh doctor`.
    pub fn pending(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|f| match &f.state {
                State::Migrate { dropped, from, .. } => Some(format!(
                    "{}'s {} on this account becomes per session at the next launch{} — keeping {}{}",
                    self.harness,
                    f.declared.name,
                    in_place_of(from.as_deref(), &self.harness, "reads"),
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
                State::Import => Some(format!(
                    "{}'s {} preferences on this account are kept apart from the next launch — \
                     keeping {}",
                    self.harness,
                    f.declared.name,
                    names(&f.kept)
                )),
                _ => None,
            })
            .map(|s| crate::out::untrusted(&s))
            .collect()
    }

    /// Do what the account's files need, under the account's lock: for
    /// `link`, back the real file up, keep its declared keys, lay the link,
    /// then the marker — in that order, so each point a run can stop at
    /// leaves an account the next launch finishes; for `file`, keep what the
    /// import found, once. `running` names the sessions still mounting the
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
        if self.mount == PrefMount::File {
            let mut notes = Vec::new();
            for f in &self.files {
                let name = OsStr::new(&f.declared.name);
                // Under the lock: a launch that took it first may have kept
                // the file already, and a session since then a value in it.
                if !matches!(f.state, State::Import)
                    || !matches!(kept_dir.entry(name), Ok(crate::held::Entry::Absent))
                {
                    continue;
                }
                kept_dir.replace_private(name, &f.declared.format().render(&f.kept)?)?;
                if !f.kept.is_empty() {
                    notes.push(crate::out::untrusted(&format!(
                        "{}'s {} preferences are kept apart on this account from now on — kept {}",
                        self.harness,
                        f.declared.name,
                        names(&f.kept)
                    )));
                }
            }
            return Ok(notes);
        }
        // Read under the lock: a launch that took it first may have finished
        // the migration this one was checked for — and its sandbox, now
        // running, is not one from before the link.
        if !self.marked
            && matches!(
                kept_dir.entry(OsStr::new(MARKER)),
                Ok(crate::held::Entry::File)
            )
        {
            return Ok(Vec::new());
        }
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
                State::Ready | State::Import => continue,
                State::Finish | State::Lay => {}
                State::Migrate { dropped, raw, from } => {
                    let source = from.as_deref().unwrap_or(rel);
                    let source_name = Path::new(source)
                        .file_name()
                        .context("a migrated file with no name")?;
                    let backup = format!("{}.before-omh", source_name.to_string_lossy());
                    kept_dir
                        .replace_private(OsStr::new(&backup), raw)
                        .with_context(|| {
                            format!("backing up {}", self.dir.join(source).display())
                        })?;
                    kept_dir.replace_private(name, &f.declared.format().render(&f.kept)?)?;
                    // Before the link: stopped between the two, the next
                    // launch finds neither file and lays the link over what
                    // was just kept.
                    if let Some(from) = from {
                        if let Some((at, leaf)) = hold_parent(&self.dir, from, false)? {
                            at.remove(leaf).with_context(|| {
                                format!("removing {}", self.dir.join(from).display())
                            })?;
                        }
                    }
                    notes.push(crate::out::untrusted(&format!(
                        "{}'s {} is per session from now on{} — kept {}{}; the file as it was is \
                         at {}",
                        self.harness,
                        f.declared.name,
                        in_place_of(from.as_deref(), &self.harness, "read"),
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
    /// changed: refused while the session has changes to review, which
    /// belong to its old account, then seeded afresh from this one; a scratch
    /// home every time. Written after the reuse decision and before the
    /// container starts: never into a directory a running container has
    /// mounted, and never missing when one mounts it.
    ///
    /// `rests` holds, by [`Account::copies`]' paths, what a copy holds beside
    /// its preferences — `auth::seeds`' scrubbed `~/.claude.json`. The copy is
    /// that, with the declared keys set from the account's kept values; on a
    /// relaunch whose rest has changed — the account captured again — from
    /// the session's own, which its review has not seen yet.
    ///
    /// `created`: this launch made the session, so whatever is already in
    /// its home is an earlier session's under the same id — seeded afresh,
    /// or refused while that one's changes are still to review.
    pub fn seed(
        &self,
        adapter: &Adapter,
        home: &Home,
        rests: &BTreeMap<String, String>,
        created: bool,
    ) -> Result<Vec<String>> {
        let root = home.path();
        std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
        let mut notes = Vec::new();
        let recorded = std::fs::read_to_string(root.join(RECORD)).ok();
        let this = self.dir.display().to_string();
        let mut rest_of = BTreeMap::new();
        for f in &self.files {
            let rest = match rests.get(account_rel(&f.declared)) {
                Some(body) => Format::Json.parse(body.as_bytes())?,
                None => Doc::new(),
            };
            let rest = compose(&rest, &Doc::new(), f.declared.all());
            let digest = match rests.contains_key(account_rel(&f.declared)) {
                true => Some(format!(
                    "{:016x}",
                    crate::hash::fnv1a_64(&Format::Json.render(&rest)?)
                )),
                false => None,
            };
            rest_of.insert(f.declared.name.clone(), (rest, digest));
        }
        if let (Home::Kept(_), true, Some(_)) = (home, created, recorded.as_deref()) {
            let left = review::unreviewed(adapter, home)?;
            anyhow::ensure!(
                left.is_empty(),
                "an earlier session under this id left {} preferences still to review — {} — in \
                 {}: review them (`omh <id> down` at a terminal) or drop them (`omh <id> rm \
                 --yes`) before a new session takes the id",
                self.harness,
                left.join(", "),
                root.display()
            );
        }
        if let (Home::Kept(_), false) = (home, created) {
            match recorded.as_deref() {
                Some(r) if r == this => return self.continue_from_live(home, &rest_of),
                Some(r) => {
                    let left = review::unreviewed(adapter, home)?;
                    anyhow::ensure!(
                        left.is_empty(),
                        "this session's {} preferences, from {r}, are still to review — {} — and \
                         would be lost to a launch on {this}: review them first (`omh <id> \
                         down` at a terminal), or drop them (`omh <id> rm --yes`)",
                        self.harness,
                        left.join(", ")
                    );
                    notes.push(format!(
                        "this session's {} preferences were seeded again, from {}",
                        self.harness,
                        self.dir.display()
                    ));
                }
                None => {}
            }
        }
        for part in ["base", "reviewed", "live", REST] {
            let dir = root.join(part);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("clearing {}", dir.display())),
            }
            std::fs::create_dir_all(&dir)?;
        }
        // What the account keeps, read again under its lock — the one a
        // review holds from its first reading to its last write — so a seed
        // never starts from a value a review was about to change.
        let (kept_dir, _) = make_parent(&self.dir, &format!("{KEPT}/{LOCK}"))?;
        let _lock = kept_dir
            .lock(OsStr::new(LOCK))
            .with_context(|| format!("locking {}", self.dir.join(KEPT).display()))?;
        for f in &self.files {
            let name = OsStr::new(&f.declared.name);
            let format = f.declared.format();
            let (rest, digest) = &rest_of[&f.declared.name];
            let kept = match kept_if_any(&self.dir, &f.declared)? {
                Some(now) => pick(&now, f.declared.all()),
                None => f.kept.clone(),
            };
            for part in ["base", "reviewed"] {
                let held = crate::held::Dir::open(&root.join(part))?;
                place(&held, name, &format.render(&kept)?, self.mount)?;
            }
            let held = crate::held::Dir::open(&live(home))?;
            let copy = compose(rest, &kept, f.declared.all());
            place(&held, name, &format.render(&copy)?, self.mount)?;
            if let Some(digest) = digest {
                crate::held::Dir::open(&root.join(REST))?.replace(name, digest.as_bytes())?;
            }
        }
        crate::held::Dir::open(root)?.replace(OsStr::new(RECORD), this.as_bytes())?;
        Ok(notes)
    }

    /// A relaunch on the same account: the session's own `live` stands. A
    /// copy whose rest has changed since it was written, or that is gone or
    /// no longer parses, is written again around the session's own values —
    /// as its last review left them where they cannot be read.
    fn continue_from_live(
        &self,
        home: &Home,
        rest_of: &BTreeMap<String, (Doc, Option<String>)>,
    ) -> Result<Vec<String>> {
        let root = home.path();
        let live_dir = crate::held::Dir::open(&live(home))?;
        let mut notes = Vec::new();
        for f in &self.files {
            let name = OsStr::new(&f.declared.name);
            let format = f.declared.format();
            let (rest, digest) = &rest_of[&f.declared.name];
            let had = std::fs::read_to_string(root.join(REST).join(name)).ok();
            let now = read_doc(&live_dir, name, format);
            let intact = matches!(now, Ok(Some(_)));
            if intact && had == *digest {
                continue;
            }
            let values = match now {
                Ok(Some(doc)) => doc,
                _ => {
                    notes.push(crate::out::untrusted(&format!(
                        "this session's {} could not be read, and is written again from its \
                         last review",
                        f.declared.name
                    )));
                    read_doc(
                        &crate::held::Dir::open(&root.join("reviewed"))?,
                        name,
                        format,
                    )?
                    .unwrap_or_default()
                }
            };
            let copy = compose(rest, &values, f.declared.all());
            place(&live_dir, name, &format.render(&copy)?, self.mount)?;
            if let Some(digest) = digest {
                std::fs::create_dir_all(root.join(REST))?;
                crate::held::Dir::open(&root.join(REST))?.replace(name, digest.as_bytes())?;
            }
        }
        Ok(notes)
    }
}

/// Where a migration reads the file the harness reads in a declared one's
/// place: that file, named — `reads` in the tense of the sentence.
fn in_place_of(from: Option<&str>, harness: &str, reads: &str) -> String {
    match from.and_then(|f| Path::new(f).file_name()) {
        Some(name) => format!(
            ", from the {} {harness} {reads} in its place",
            name.to_string_lossy()
        ),
        None => String::new(),
    }
}

/// The keys `doc` holds, dotted, or `nothing`.
fn names(doc: &Doc) -> String {
    let mut out = Vec::new();
    fn walk(t: &Doc, at: &mut KeyPath, out: &mut Vec<String>) {
        for (k, v) in t {
            at.push(k.clone());
            match v {
                Value::Object(inner) if !inner.is_empty() => walk(inner, at, out),
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

/// A login's preferences, kept into its account without asking: the user
/// chose them at the terminal, a theme picked at a first start — every
/// session's go through the review instead (`review`). Changed keys only,
/// last writer wins, each value another session had kept since named as
/// replaced. What the login wrote that is not a preference is named and left
/// where it is — except in a copy of an account file, whose other keys are
/// the harness's own bookkeeping and were never the login's to keep.
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
        let format = d.format();
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
        let live = read_doc(live_dir, name, format)
            .map_err(could_not_tell)?
            .unwrap_or_default();
        let base = read_doc(&crate::held::Dir::open(&root.join("base"))?, name, format)?
            .unwrap_or_default();
        let mut account_kept = read_doc(&kept_dir, name, format)?.unwrap_or_default();
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
            kept_dir.replace_private(name, &format.render(&account_kept)?)?;
            notes.push(crate::out::untrusted(&format!(
                "kept {} {} preference{} on account `{shown_account}`: {}",
                changes.len(),
                adapter.name,
                if changes.len() == 1 { "" } else { "s" },
                kept_keys.join(", ")
            )));
        }
        let keys: Vec<_> = d.all().collect();
        let synced = format.render(&pick(&live, keys.iter().copied()))?;
        for part in ["base", "reviewed"] {
            let held = crate::held::Dir::open(&root.join(part))?;
            place(&held, name, &synced, prefs.mount)?;
        }
        let copy = root.join(REST).join(name).is_file();
        let left = undeclared(&live, &keys);
        if !left.is_empty() && !copy {
            notes.push(crate::out::untrusted(&format!(
                "not kept, not preferences: {}",
                left.join(", ")
            )));
        }
    }
    Ok(notes)
}

/// What an account keeps for one preferences file.
#[derive(Debug)]
pub struct Kept {
    pub file: String,
    pub doc: Doc,
    /// `doc` by key, dotted, in the adapter's order.
    pub keys: Vec<(String, Value)>,
}

/// What the account at `account` keeps of `adapter`'s preferences, file by
/// file — the declared keys alone, as a launch seeds them. A file it keeps
/// nothing of is an empty document; one omh cannot read is an error naming
/// it, never nothing kept.
pub fn kept_by_file(adapter: &Adapter, account: &Path) -> Result<Vec<Kept>> {
    let Some(prefs) = adapter.preferences.as_ref() else {
        return Ok(Vec::new());
    };
    prefs
        .declared()
        .iter()
        .map(|d| {
            let doc = pick(&kept(account, d)?, d.all());
            let keys = d
                .all()
                .filter_map(|k| get(&doc, k).map(|v| (dotted(k), v.clone())))
                .collect();
            Ok(Kept {
                file: d.name.clone(),
                doc,
                keys,
            })
        })
        .collect()
}

/// What [`forget`] took from an account, or would.
#[derive(Debug)]
pub struct Forgot {
    /// The key, dotted.
    pub key: String,
    /// The file it was kept for.
    pub file: String,
    pub was: Value,
}

/// Take `key` from what the account at `account` keeps of `adapter`'s
/// preferences, and nothing else — under the lock a review and a seed hold,
/// so neither reads the file half written. `dry_run` reads and refuses as the
/// real run does, and writes nothing.
///
/// The account only. A session already started keeps its own copy, and its
/// review finds the account no longer keeping what it started from.
pub fn forget(
    adapter: &Adapter,
    account: &Path,
    shown: &str,
    key: &str,
    dry_run: bool,
) -> Result<Forgot> {
    let harness = adapter.name.as_str();
    let Some(prefs) = adapter.preferences.as_ref() else {
        anyhow::bail!("{harness} keeps no preferences on its accounts");
    };
    let path = crate::adapter::key_path(key).map_err(anyhow::Error::msg)?;
    let not_one = || {
        format!(
            "`{key}` is not one of {harness}'s preferences — `omh account` lists what each \
             account keeps"
        )
    };
    let declared = prefs.declared();
    let d = declared
        .iter()
        .find(|d| d.all().any(|k| *k == path))
        .with_context(not_one)?;
    let keeps_none = || format!("account `{shown}` keeps no {harness} {}", dotted(&path));
    let name = OsStr::new(&d.name);
    let at = || format!("{}", account.join(KEPT).join(&d.name).display());
    let rel = format!("{KEPT}/{}", d.name);
    let Some((dir, _)) = hold_parent(account, &rel, false).with_context(at)? else {
        anyhow::bail!(keeps_none());
    };
    let _lock = match dry_run {
        true => None,
        false => Some(
            dir.lock(OsStr::new(LOCK))
                .with_context(|| format!("locking {}", account.join(KEPT).display()))?,
        ),
    };
    let Some(now) = read_doc(&dir, name, d.format()).with_context(at)? else {
        anyhow::bail!(keeps_none());
    };
    let mut doc = pick(&now, d.all());
    let Some(was) = get(&doc, &path).cloned() else {
        anyhow::bail!(keeps_none());
    };
    set(&mut doc, &path, None);
    if !dry_run {
        dir.replace_private(name, &d.format().render(&doc)?)
            .with_context(at)?;
    }
    Ok(Forgot {
        key: dotted(&path),
        file: d.name.clone(),
        was,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::key_path;

    fn table(raw: &str) -> Doc {
        Format::Toml.parse(raw.as_bytes()).unwrap()
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
        let mut doc = Doc::new();
        let theme = keys(&["tui.theme"]).remove(0);
        set(&mut doc, &theme, Some("t".into()));
        assert_eq!(doc, table("[tui]\ntheme = \"t\"\n"));
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc:?}");
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc:?}");
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
            table(&kept),
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
                && err.contains("omh account login codex --name work --provider")
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
            &toml::from_str(
                "model_provider = \"azure\"\n[model_providers.azure]\nname = \"Azure\"\n",
            )
            .unwrap(),
        )
        .unwrap();
        check(&codex(), &account).unwrap().unwrap();
        std::fs::remove_file(account.join("provider.toml")).unwrap();
        std::fs::write(&file, "model_provider = \"openai\"\n").unwrap();
        check(&codex(), &account).unwrap().unwrap();
    }

    /// Two first launches on one account: the one that takes the lock second
    /// finds the migration done, and neither asks about running sessions —
    /// whose new-style sandbox it would otherwise take for an old one — nor
    /// migrates again.
    #[test]
    fn a_migration_another_launch_finished_is_not_done_twice() {
        let (_root, account, file) = account();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "model = \"a\"\n").unwrap();
        let first = check(&codex(), &account).unwrap().unwrap();
        let second = check(&codex(), &account).unwrap().unwrap();
        first.apply(&none_running).unwrap();
        let backup = account.join("preferences/config.toml.before-omh");
        std::fs::write(&backup, "the first launch's").unwrap();
        second.apply(&|| Ok(vec!["omh-repo-s01".into()])).unwrap();
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "the first launch's"
        );
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
    /// from its own copy; another account is refused while the session has a
    /// change to review — it belongs to the old one — and seeds from the new
    /// once it is reviewed.
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
            checked
                .seed(&codex(), &home, &BTreeMap::new(), false)
                .unwrap()
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

        let checked = check(&codex(), &personal).unwrap().unwrap();
        let err = format!(
            "{:#}",
            checked
                .seed(&codex(), &home, &BTreeMap::new(), false)
                .unwrap_err()
        );
        assert!(
            err.contains("are still to review — model") && err.contains("omh <id> rm --yes"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"mine\"\n"
        );
        let sitting = review::Sitting {
            id: "s01".into(),
            home: Home::Kept(home.path().to_path_buf()),
        };
        review::review(&codex(), &[sitting], review::Answers::DropAll).unwrap();
        let notes = seed(&personal).join("\n");
        assert!(notes.contains("seeded again"), "{notes}");
        assert_eq!(
            std::fs::read_to_string(work.join("preferences/config.toml")).unwrap(),
            "model = \"w\"\n"
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
        checked
            .seed(&codex(), &home, &BTreeMap::new(), false)
            .unwrap();
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
        assert_eq!(table(&kept), table("model = \"b\"\n[tui]\ntheme = \"t\"\n"));
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

    // ── Each format ──

    /// TOML's datetimes and JSON's `null` are values like any other: read
    /// into the one model and written back as what they were.
    #[test]
    fn each_format_reads_back_what_it_wrote() {
        let toml = Format::Toml
            .parse(b"at = 1979-05-27T07:32:00Z\n[tui]\ntheme = \"t\"\n")
            .unwrap();
        let written = String::from_utf8(Format::Toml.render(&toml).unwrap()).unwrap();
        assert!(written.contains("at = 1979-05-27T07:32:00Z"), "{written}");
        assert_eq!(Format::Toml.parse(written.as_bytes()).unwrap(), toml);

        let json = Format::Json
            .parse(br#"{"model":null,"modelSettings":{"x":{"effortLevel":"high"}}}"#)
            .unwrap();
        let written = Format::Json.render(&json).unwrap();
        assert_eq!(Format::Json.parse(&written).unwrap(), json);
        assert!(Format::Json.parse(b"").unwrap().is_empty());
        for bad in [&b"[1]"[..], b"{", b"\xff"] {
            assert!(Format::Json.parse(bad).is_err(), "{bad:?}");
        }

        let yaml = Format::Yaml
            .parse(b"modelRoles:\n  default: fake/a\ndefaultThinkingLevel: high\nenabledModels: [a, 'on']\nx: null\n")
            .unwrap();
        assert_eq!(yaml["enabledModels"], serde_json::json!(["a", "on"]));
        let written = Format::Yaml.render(&yaml).unwrap();
        assert_eq!(Format::Yaml.parse(&written).unwrap(), yaml);
        // What omp reads as no settings at all, and what nothing renders as.
        assert!(Format::Yaml.parse(b"").unwrap().is_empty());
        assert!(Format::Yaml.parse(b"# nothing\n").unwrap().is_empty());
        let empty = Format::Yaml.render(&Doc::new()).unwrap();
        assert!(Format::Yaml.parse(&empty).unwrap().is_empty());
        for bad in [
            &b"- 1\n"[..],
            b"a: [\n",
            b"\xff",
            b"a: 1\na: 2\n",
            b"a: &x {b: 1}\nc:\n  <<: *x\n",
            b"a: 1\n---\nb: 2\n",
            b"plain\n",
        ] {
            assert!(Format::Yaml.parse(bad).is_err(), "{bad:?}");
        }
    }

    // ── `link`, a YAML file omp reads in its place ──

    fn omp() -> Adapter {
        Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            "omp",
        )
        .unwrap()
    }

    /// An omp account dir under a temp root, its `config.yml`, and the
    /// `config.yaml` omp reads in that one's place.
    fn omp_account() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/omp/work");
        let agent = account.join(".omp/agent");
        std::fs::create_dir_all(&agent).unwrap();
        (
            root,
            account,
            agent.join("config.yml"),
            agent.join("config.yaml"),
        )
    }

    const OMP_ORIGINAL: &str = "modelRoles:\n  default: fake/a\ntheme:\n  dark: night\n\
                                extensions: [/tmp/planted.ts]\nauth:\n  broker:\n    token: sk-planted\n";

    /// omp's own file is migrated the way codex's is — declared keys kept,
    /// written as YAML, the rest dropped by name, the bytes backed up — and a
    /// `config.yaml` beside it, which omp never read, is left as it is.
    #[test]
    fn an_omp_account_is_migrated_and_kept_as_yaml() {
        let (_root, account, yml, yaml) = omp_account();
        std::fs::write(&yml, OMP_ORIGINAL).unwrap();
        std::fs::write(&yaml, "theme:\n  dark: other\n").unwrap();
        let notes = check(&omp(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap()
            .join("\n");
        assert!(notes.contains("dropped auth, extensions"), "{notes}");
        assert!(!notes.contains("sk-planted"), "a value was named: {notes}");
        assert_eq!(link_of(&yml), Some(PathBuf::from("/omh/prefs/config.yml")));
        let kept = std::fs::read(account.join("preferences/config.yml")).unwrap();
        assert_eq!(
            Format::Yaml.parse(&kept).unwrap(),
            Format::Yaml
                .parse(b"modelRoles:\n  default: fake/a\ntheme:\n  dark: night\n")
                .unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(account.join("preferences/config.yml.before-omh")).unwrap(),
            OMP_ORIGINAL
        );
        assert_eq!(
            std::fs::read_to_string(&yaml).unwrap(),
            "theme:\n  dark: other\n"
        );
    }

    /// Where `config.yml` is absent, omp reads `config.yaml`: that is the
    /// account's file then, migrated like one and taken out of the account
    /// — which every session mounts — once its bytes are backed up, since
    /// the link makes omp stop reading it.
    #[test]
    fn the_file_omp_reads_in_its_place_is_migrated_and_taken_away() {
        use std::os::unix::fs::PermissionsExt;
        let (_root, account, yml, yaml) = omp_account();
        std::fs::write(&yaml, OMP_ORIGINAL).unwrap();
        let checked = check(&omp(), &account).unwrap().unwrap();
        assert!(checked.needs_quiet(), "a session could be reading it");
        let pending = checked.pending().join("\n");
        assert!(
            pending.contains("config.yaml") && pending.contains("dropping auth, extensions"),
            "{pending}"
        );
        let notes = checked.apply(&none_running).unwrap().join("\n");
        assert!(
            notes.contains("config.yaml") && notes.contains("dropped auth, extensions"),
            "{notes}"
        );
        assert!(!notes.contains("sk-planted"), "a value was named: {notes}");
        assert_eq!(link_of(&yml), Some(PathBuf::from("/omh/prefs/config.yml")));
        assert!(!yaml.exists(), "config.yaml is still in the account");
        let kept = std::fs::read(account.join("preferences/config.yml")).unwrap();
        assert_eq!(
            Format::Yaml.parse(&kept).unwrap(),
            Format::Yaml
                .parse(b"modelRoles:\n  default: fake/a\ntheme:\n  dark: night\n")
                .unwrap()
        );
        let backup = account.join("preferences/config.yaml.before-omh");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), OMP_ORIGINAL);
        let mode = std::fs::metadata(&backup).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// Stopped before `config.yaml` goes, a launch migrates it again to the
    /// same result; stopped after, before the link, it lays the link over
    /// what was kept.
    #[test]
    fn a_migration_from_the_file_omp_reads_in_its_place_is_finished_by_the_next_launch() {
        let (_root, account, yml, yaml) = omp_account();
        std::fs::write(&yaml, OMP_ORIGINAL).unwrap();
        let want = Format::Yaml
            .parse(b"modelRoles:\n  default: fake/a\ntheme:\n  dark: night\n")
            .unwrap();
        // Stopped after the backup and the keep, before config.yaml went.
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(
            account.join("preferences/config.yml"),
            "theme:\n  dark: half\n",
        )
        .unwrap();
        check(&omp(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap();
        let kept = std::fs::read(account.join("preferences/config.yml")).unwrap();
        assert_eq!(Format::Yaml.parse(&kept).unwrap(), want);
        assert!(!yaml.exists());
        // Stopped after config.yaml went, before the link and the marker.
        std::fs::remove_file(&yml).unwrap();
        std::fs::remove_file(account.join("preferences/linked")).unwrap();
        check(&omp(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap();
        assert_eq!(link_of(&yml), Some(PathBuf::from("/omh/prefs/config.yml")));
        let kept = std::fs::read(account.join("preferences/config.yml")).unwrap();
        assert_eq!(Format::Yaml.parse(&kept).unwrap(), want);
        assert!(account.join("preferences/linked").is_file());
    }

    /// After the link, a `config.yaml` is a session's — omp does not read it
    /// while the link is there, and nothing in it is ever kept.
    #[test]
    fn after_the_link_the_file_omp_reads_in_its_place_is_never_kept() {
        let (_root, account, yml, yaml) = omp_account();
        check(&omp(), &account)
            .unwrap()
            .unwrap()
            .apply(&none_running)
            .unwrap();
        std::fs::remove_file(&yml).unwrap();
        std::fs::write(&yaml, OMP_ORIGINAL).unwrap();
        let checked = check(&omp(), &account).unwrap().unwrap();
        assert!(!checked.needs_quiet());
        checked.apply(&|| panic!("asked")).unwrap();
        assert_eq!(link_of(&yml), Some(PathBuf::from("/omh/prefs/config.yml")));
        let kept = std::fs::read(account.join("preferences/config.yml")).unwrap_or_default();
        assert!(
            Format::Yaml.parse(&kept).unwrap().is_empty(),
            "kept from it"
        );
        assert!(yaml.exists(), "a session's file is not omh's to remove");
    }

    // ── `dir` ──

    fn opencode() -> Adapter {
        Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            "opencode",
        )
        .unwrap()
    }

    /// A `dir` account has no file of its own to migrate or import — the
    /// harness's state directory was never in it — so a launch changes
    /// nothing in the account and asks about no running session. What the
    /// account keeps, declared keys alone, is what `omh doctor` names and
    /// what a launch seeds the session's directory with — an empty document
    /// for a file it keeps nothing of. What the harness
    /// writes beside them in its directory is the session's, and a relaunch
    /// keeps it.
    #[test]
    fn a_dir_account_seeds_the_sessions_directory_and_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/opencode/work");
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(
            account.join("preferences/model.json"),
            r#"{"recent":[{"providerID":"fake","modelID":"kept"}],"planted":true}"#,
        )
        .unwrap();
        let before: Vec<_> = walk(&account);
        let checked = check(&opencode(), &account).unwrap().unwrap();
        assert!(!checked.needs_quiet());
        assert!(checked.pending().is_empty(), "{:?}", checked.pending());
        let summary = checked.summary();
        assert!(
            summary.contains("model.json — the account keeps recent")
                && summary.contains("kv.json — the account keeps nothing")
                && !summary.contains("planted"),
            "{summary}"
        );
        assert!(checked
            .apply(&|| panic!("a dir account asks about running sessions"))
            .unwrap()
            .is_empty());
        assert_eq!(walk(&account), before, "the account changed");

        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/opencode"));
        checked
            .seed(&opencode(), &home, &BTreeMap::new(), true)
            .unwrap();
        let read = |part: &str, name: &str| -> Doc {
            Format::Json
                .parse(&std::fs::read(home.path().join(part).join(name)).unwrap())
                .unwrap()
        };
        let kept = Format::Json
            .parse(br#"{"recent":[{"providerID":"fake","modelID":"kept"}]}"#)
            .unwrap();
        for part in ["base", "reviewed", "live"] {
            assert_eq!(read(part, "model.json"), kept, "{part}");
            assert!(read(part, "kv.json").is_empty(), "{part}");
        }
        std::fs::write(live(&home).join("prompt-history.jsonl"), "{}\n").unwrap();
        checked
            .seed(&opencode(), &home, &BTreeMap::new(), false)
            .unwrap();
        assert!(live(&home).join("prompt-history.jsonl").is_file());
    }

    /// Every path under `dir`, sorted — what a launch must leave as it was.
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            out.push(entry.path());
            if entry.file_type().unwrap().is_dir() {
                out.extend(walk(&entry.path()));
            }
        }
        out.sort();
        out
    }

    // ── `file` ──

    fn claude() -> Adapter {
        Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            "claude",
        )
        .unwrap()
    }

    fn json(raw: &str) -> Doc {
        Format::Json.parse(raw.as_bytes()).unwrap()
    }

    fn read_json(path: &Path) -> Doc {
        json(&std::fs::read_to_string(path).unwrap())
    }

    /// The first launch keeps the declared keys of the account's own files —
    /// where Claude wrote them before sessions had their own — and nothing
    /// else, then never reads those files for preferences again.
    #[test]
    fn a_file_account_keeps_its_own_files_preferences_once() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(
            account.join(".claude.json"),
            r#"{"copyOnSelect":false,"oauthAccount":{"emailAddress":"a@b"},"projects":{}}"#,
        )
        .unwrap();

        let checked = check(&claude(), &account).unwrap().unwrap();
        let pending = checked.pending().join("\n");
        assert!(
            pending.contains(".claude.json preferences on this account are kept apart")
                && pending.contains("keeping copyOnSelect"),
            "{pending}"
        );
        assert!(!account.join("preferences").exists(), "checking wrote");
        let notes = checked.apply(&|| panic!("asked")).unwrap().join("\n");
        assert!(notes.contains("kept copyOnSelect"), "{notes}");
        assert_eq!(
            read_json(&account.join("preferences/.claude.json")),
            json(r#"{"copyOnSelect":false}"#)
        );
        assert!(read_json(&account.join("preferences/settings.json")).is_empty());

        std::fs::write(account.join(".claude.json"), r#"{"copyOnSelect":true}"#).unwrap();
        let again = check(&claude(), &account).unwrap().unwrap();
        assert!(again.pending().is_empty(), "{:?}", again.pending());
        again.apply(&|| panic!("asked")).unwrap();
        assert_eq!(
            read_json(&account.join("preferences/.claude.json")),
            json(r#"{"copyOnSelect":false}"#)
        );
    }

    /// A launch that took the lock first and kept the import is not undone by
    /// the one checked beside it.
    #[test]
    fn an_import_another_launch_finished_is_not_done_twice() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join(".claude.json"), r#"{"copyOnSelect":false}"#).unwrap();
        let first = check(&claude(), &account).unwrap().unwrap();
        let second = check(&claude(), &account).unwrap().unwrap();
        first.apply(&|| panic!("asked")).unwrap();
        std::fs::write(
            account.join("preferences/.claude.json"),
            r#"{"copyOnSelect":true}"#,
        )
        .unwrap();
        second.apply(&|| panic!("asked")).unwrap();
        assert_eq!(
            read_json(&account.join("preferences/.claude.json")),
            json(r#"{"copyOnSelect":true}"#)
        );
    }

    /// An account file that is not one to read — a link — refuses the
    /// launch, naming it, and nothing is kept.
    #[test]
    fn an_account_file_that_cannot_be_read_refuses_the_import() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(account.join(".claude")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", account.join(".claude/settings.json")).unwrap();
        let err = format!("{:#}", check(&claude(), &account).unwrap_err());
        assert!(
            err.contains(".claude/settings.json cannot be read for its preferences"),
            "{err}"
        );
        assert!(!account.join("preferences").exists());
    }

    /// A copy of an account file that is a preferences file too is the
    /// account file's rest — what `auth::seeds` scrubbed — with the declared
    /// keys from the account's store, whatever the rest held for them; `base`
    /// and `reviewed` hold the declared keys alone. Readable by the user
    /// alone: the rest is the account's identity.
    #[test]
    fn a_copy_is_its_rest_with_the_values_the_account_keeps() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(
            account.join("preferences/.claude.json"),
            r#"{"copyOnSelect":false,"oauthAccount":"planted"}"#,
        )
        .unwrap();
        std::fs::write(
            account.join("preferences/settings.json"),
            r#"{"model":"m"}"#,
        )
        .unwrap();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/claude"));
        let rests = BTreeMap::from([(
            ".claude.json".to_string(),
            r#"{"oauthAccount":{"emailAddress":"a@b"},"copyOnSelect":true}"#.to_string(),
        )]);
        let checked = check(&claude(), &account).unwrap().unwrap();
        assert_eq!(
            checked.copies(),
            vec![".claude.json", ".claude/settings.json"]
        );
        checked.seed(&claude(), &home, &rests, false).unwrap();

        let copy = live(&home).join(".claude.json");
        assert_eq!(
            read_json(&copy),
            json(r#"{"oauthAccount":{"emailAddress":"a@b"},"copyOnSelect":false}"#)
        );
        for part in ["base", "reviewed"] {
            assert_eq!(
                read_json(&home.path().join(part).join(".claude.json")),
                json(r#"{"copyOnSelect":false}"#),
                "{part}"
            );
        }
        assert_eq!(
            read_json(&live(&home).join("settings.json")),
            json(r#"{"model":"m"}"#)
        );
        for file in [".claude.json", "settings.json"] {
            let mode = std::fs::metadata(live(&home).join(file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    /// A relaunch keeps the session's own copy; a rest that changed since —
    /// the account captured again — is written in around the session's own
    /// values, not the account's, so nothing it has not reviewed is lost or
    /// shown as its own; and a copy that no longer parses is written again
    /// from its last review, saying so.
    #[test]
    fn a_relaunch_keeps_the_sessions_values_around_a_new_rest() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(
            account.join("preferences/.claude.json"),
            r#"{"copyOnSelect":false}"#,
        )
        .unwrap();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/claude"));
        let rest = |who: &str| {
            BTreeMap::from([(
                ".claude.json".to_string(),
                format!(r#"{{"oauthAccount":{{"emailAddress":"{who}"}}}}"#),
            )])
        };
        let seed = |rests: &BTreeMap<String, String>| {
            check(&claude(), &account)
                .unwrap()
                .unwrap()
                .seed(&claude(), &home, rests, false)
                .unwrap()
        };
        seed(&rest("a"));
        let copy = live(&home).join(".claude.json");
        std::fs::write(
            &copy,
            r#"{"oauthAccount":{"emailAddress":"a"},"copyOnSelect":true,"numStartups":3}"#,
        )
        .unwrap();
        // Another session kept a value meanwhile: not this session's.
        std::fs::write(
            account.join("preferences/.claude.json"),
            r#"{"copyOnSelect":false,"respectGitignore":false}"#,
        )
        .unwrap();

        seed(&rest("a"));
        assert_eq!(
            read_json(&copy),
            json(r#"{"oauthAccount":{"emailAddress":"a"},"copyOnSelect":true,"numStartups":3}"#)
        );

        seed(&rest("b"));
        assert_eq!(
            read_json(&copy),
            json(r#"{"oauthAccount":{"emailAddress":"b"},"copyOnSelect":true}"#)
        );

        std::fs::write(&copy, "{ not json").unwrap();
        let notes = seed(&rest("b")).join("\n");
        assert!(
            notes.contains(
                ".claude.json could not be read, and is written again from its last review"
            ),
            "{notes}"
        );
        assert_eq!(
            read_json(&copy),
            json(r#"{"oauthAccount":{"emailAddress":"b"},"copyOnSelect":false}"#)
        );
    }

    /// A launch that makes the session finds an earlier session's preferences
    /// under the same id: refused while a change there is still to review,
    /// naming it; seeded afresh once there is none — never continued.
    #[test]
    fn a_new_session_over_an_earlier_ones_preferences_starts_afresh_or_is_refused() {
        let (root, work, _) = account();
        std::fs::create_dir_all(work.join("preferences")).unwrap();
        std::fs::write(work.join("preferences/config.toml"), "model = \"w\"\n").unwrap();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/codex"));
        let at_live = live(&home).join("config.toml");
        let seed = |created: bool| {
            let checked = check(&codex(), &work).unwrap().unwrap();
            checked.apply(&none_running).unwrap();
            checked.seed(&codex(), &home, &BTreeMap::new(), created)
        };
        seed(false).unwrap();
        std::fs::write(&at_live, "model = \"mine\"\n").unwrap();

        let err = format!("{:#}", seed(true).unwrap_err());
        assert!(
            err.contains(
                "an earlier session under this id left codex preferences still to review — model"
            ),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"mine\"\n"
        );

        let sitting = review::Sitting {
            id: "s01".into(),
            home: Home::Kept(home.path().to_path_buf()),
        };
        review::review(&codex(), &[sitting], review::Answers::DropAll).unwrap();
        seed(true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&at_live).unwrap(),
            "model = \"w\"\n"
        );
    }

    /// A seed reads what the account keeps again, under its lock: a value
    /// kept between the launch's check and its seed is the one the session
    /// starts from, never the one checked.
    #[test]
    fn a_seed_starts_from_what_the_account_keeps_when_it_seeds() {
        let (root, work, _) = account();
        std::fs::create_dir_all(work.join("preferences")).unwrap();
        std::fs::write(work.join("preferences/config.toml"), "model = \"a\"\n").unwrap();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/codex"));
        let checked = check(&codex(), &work).unwrap().unwrap();
        checked.apply(&none_running).unwrap();
        std::fs::write(work.join("preferences/config.toml"), "model = \"b\"\n").unwrap();
        checked
            .seed(&codex(), &home, &BTreeMap::new(), false)
            .unwrap();
        for part in ["base", "reviewed", "live"] {
            assert_eq!(
                std::fs::read_to_string(home.path().join(part).join("config.toml")).unwrap(),
                "model = \"b\"\n",
                "{part}"
            );
        }
    }

    /// What the account keeps is its declared keys alone, whatever its store
    /// holds beside them: `omh doctor`'s line names those, and nothing else.
    #[test]
    fn what_an_account_keeps_is_named_by_its_declared_keys_alone() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/claude/work");
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(
            account.join("preferences/settings.json"),
            r#"{"model":"m","hooks":{"SessionStart":[]}}"#,
        )
        .unwrap();
        std::fs::write(account.join("preferences/.claude.json"), "{}").unwrap();
        let summary = check(&claude(), &account).unwrap().unwrap().summary();
        assert_eq!(
            summary,
            "claude's preferences are per session: .claude.json — the account keeps nothing; \
             settings.json — the account keeps model"
        );
    }
}
