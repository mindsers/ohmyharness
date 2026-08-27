//! The note store. A knowledge graph of linked Markdown files, scoped to a
//! repo, that the agent queries during a task and writes to when something
//! surprises it.
//!
//! Everything here is pure given a temp filesystem, which is deliberate: the
//! places where a mistake is invisible are the places that test cheaply.

use crate::profile::Paths;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod deliver;
pub mod expiry;
pub mod index;
pub mod ingest;
pub mod promote;
pub mod recall;
pub mod tools;

// ── layers ──────────────────────────────────────────────────────────────────

/// `team` (committed) or `local` (gitignored).
///
/// Deliberately not `config::Layer`. Notes have no personal layer, so a type
/// that cannot represent `~/.omh/profile` is a type through which `remember`
/// cannot reach it — the exhaustive match *is* invariant 3's enforcement.
///
/// The two layers also do not merge, and never shadow one another: a setting
/// has one value, but a note is a claim, and two claims about one topic are
/// two facts. So the layer is part of a note's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Team,
    Local,
}

/// Where the local store is mounted inside the sandbox.
///
/// Deliberately not under `/work`: the code graph would index notes as source,
/// `git status` would show them, and an agent running `git add -A` would
/// commit local notes onto its session branch — which is the one thing §9.1
/// forbids. The committed half needs no mount at all; it is tracked, so it
/// arrives inside the worktree by itself.
pub const GUEST_LOCAL_NOTES: &str = "/omh/notes/local";

impl Layer {
    pub const ALL: [Layer; 2] = [Self::Team, Self::Local];

    /// Where `remember` writes, and the only place it may. An unattended
    /// writer that could reach the committed layer would push wrong facts to
    /// teammates through git, where they arrive with the authority of a
    /// reviewed change.
    pub const AGENT_WRITE: Layer = Self::Local;

    /// The two layers live apart because their lifecycles differ.
    ///
    /// `team` is committed, so it is *tracked* — which is what makes it
    /// retrievable in a fresh clone, and what puts it inside every session
    /// worktree for free.
    ///
    /// `local` is outside the checkout entirely, keyed by repo exactly as the
    /// graph cache is. A worktree holds tracked files only, and `omh s rm`
    /// runs `git worktree remove --force`, so a local store inside the repo
    /// would be invisible to the sandbox and destroyed by session removal —
    /// which is the opposite of what makes this memory rather than context.
    pub fn dir(&self, paths: &Paths) -> PathBuf {
        match self {
            Self::Team => paths.repo.join(".omh").join("notes"),
            Self::Local => paths.notes().join("local"),
        }
    }

    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Team)
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Team => "team",
            Self::Local => "local",
        })
    }
}

impl std::str::FromStr for Layer {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "team" => Ok(Self::Team),
            "local" => Ok(Self::Local),
            other => anyhow::bail!("unknown layer `{other}` (team, local)"),
        }
    }
}

// ── notes ───────────────────────────────────────────────────────────────────

/// `type:` in the frontmatter. Rust reserves `type`, so the field is `kind`
/// and the renderer writes `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Surprise,
    Topic,
    Stub,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Self::Surprise, Self::Topic, Self::Stub];

    /// Sections a note of this kind must carry, by heading. Data rather than
    /// code so the schema's refusal and the lint's message cannot drift apart.
    ///
    /// A surprise's first three are exactly what `remember` asks for: the
    /// signature is the discipline, and a section it cannot fill is a section
    /// no writer can supply. `Answers` is derived rather than asked for.
    pub fn required_sections(&self) -> &'static [&'static str] {
        match self {
            // `Answers` is what a future question is matched against. Measured:
            // an index of question-shaped lines scored 95.9% P@1 on
            // question-shaped queries where the full text scored 56%. A note
            // nobody can find is a note nobody wrote.
            Self::Surprise => &["Expected", "Observed", "Evidence", "Answers"],
            // A topic is one subject richly filled; what fills it is the
            // subject's business, not the schema's.
            Self::Topic => &[],
            Self::Stub => &["Answers"],
        }
    }

    /// Sections that hold a list and nothing else. This is the structural rule
    /// standing in for a length budget: a budget needs a number calibrated
    /// against a store that does not exist yet, and *bullets only, no prose
    /// block* needs none.
    pub fn list_sections(&self) -> &'static [&'static str] {
        match self {
            Self::Surprise => &["Related", "Answers"],
            Self::Topic => &["Related"],
            Self::Stub => &["Answers"],
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Surprise => "surprise",
            Self::Topic => "topic",
            Self::Stub => "stub",
        })
    }
}

impl std::str::FromStr for Kind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Kind::ALL
            .into_iter()
            .find(|k| k.to_string() == s)
            // Never a default arm: a typo that parsed into a kind would take
            // the wrong required sections with it and stop being validated.
            .ok_or_else(|| {
                let known: Vec<String> = Kind::ALL.iter().map(|k| k.to_string()).collect();
                anyhow!("unknown note type `{s}` ({})", known.join(", "))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub key: String,
    pub kind: Kind,
    pub source: String,
    /// The literal `YYYY-MM-DD`, validated on parse and never re-derived.
    /// There is no date type in this dependency set, and inventing one invites
    /// inventing a date.
    pub recorded: String,
    pub invalidated_by: Option<String>,
    pub body: String,
    /// Stamped from the directory the note was read from, never from
    /// frontmatter. A note that could declare its own layer could claim to
    /// have been reviewed.
    pub layer: Layer,
    pub path: PathBuf,
}

/// Split `---\n<scalars>\n---\n<body>`.
///
/// Hand-rolled on purpose: the frontmatter is a handful of known scalars, and
/// a YAML library would accept a superset omh cannot round-trip — which makes
/// the schema's job ambiguous, and the schema is the only guard that refuses.
fn split_frontmatter<'a>(raw: &'a str, path: &Path) -> Result<(&'a str, &'a str)> {
    let rest = raw
        .strip_prefix("---\n")
        .with_context(|| format!("{}: no frontmatter", path.display()))?;
    let end = rest
        .find("\n---")
        .with_context(|| format!("{}: frontmatter is never closed", path.display()))?;
    Ok((&rest[..end], rest[end + 4..].trim_start_matches('\n')))
}

/// Present *and* saying something. `contains_key` is the weak version, and it
/// is the shape behind both a hook whose `command` defaulted to `""` and a
/// date guard that only checked a date was there.
fn required<'a>(fields: &BTreeMap<&str, &'a str>, name: &str, path: &Path) -> Result<&'a str> {
    match fields.get(name) {
        None => bail!("{}: missing `{name}`", path.display()),
        Some(&"") => bail!("{}: `{name}` is present but empty", path.display()),
        Some(v) => Ok(v),
    }
}

pub fn parse(raw: &str, layer: Layer, path: &Path) -> Result<Note> {
    let (head, body) = split_frontmatter(raw, path)?;

    let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
    for line in head.lines().filter(|l| !l.trim().is_empty()) {
        // Unknown keys are ignored rather than refused: a note is a human's
        // document as much as an agent's, and the inverse of the adapter rule
        // applies — an adapter is our data, a note is theirs.
        let (k, v) = line
            .split_once(':')
            .with_context(|| format!("{}: `{line}` is not `key: value`", path.display()))?;
        fields.insert(k.trim(), v.trim());
    }

    let recorded = required(&fields, "recorded", path)?;
    if !is_calendar_date(recorded) {
        bail!(
            "{}: `recorded` must be a real calendar date, got `{recorded}`",
            path.display()
        );
    }

    // §8's closed set is checked by `check` and refused at the write path, not
    // here. `invalidated_by` was free text through M1–M3, so a legacy value is
    // a file already sitting on somebody's disk — and refusing it at *load*
    // made one hand-edited line take down `ls`, `recall`, `stale` and the MCP
    // read path together, then blocked every subsequent write.

    let kind: Kind = required(&fields, "type", path)?
        .parse()
        // Flattened rather than wrapped: `to_string()` on a wrapped error
        // shows only the outermost message, which would hide the name of the
        // type that was not understood.
        .map_err(|e| anyhow!("{}: {e}", path.display()))?;

    Ok(Note {
        key: required(&fields, "key", path)?.to_string(),
        kind,
        source: required(&fields, "source", path)?.to_string(),
        recorded: recorded.to_string(),
        invalidated_by: fields
            .get("invalidated_by")
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string()),
        body: body.to_string(),
        layer,
        path: path.to_path_buf(),
    })
}

/// The bytes a note is stored as. Held to `parse(render(n)) == n`, the same
/// round-trip invariant every renderer in `render.rs` is held to.
pub fn render(note: &Note) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("key: {}\n", note.key));
    out.push_str(&format!("type: {}\n", note.kind));
    out.push_str(&format!("source: {}\n", note.source));
    out.push_str(&format!("recorded: {}\n", note.recorded));
    if let Some(trigger) = &note.invalidated_by {
        out.push_str(&format!("invalidated_by: {trigger}\n"));
    }
    out.push_str("---\n\n");
    out.push_str(&note.body);
    out
}

/// `\d{4}-\d{2}-\d{2}` is the rule §5 states, and it is the weak version: it
/// accepts `2026-13-45`. A note's date is the only thing an unverified claim
/// can be judged by, so it has to be a date that exists.
pub(crate) fn is_calendar_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let (year, month, day) = (
        s[0..4].parse::<u32>().unwrap_or(0),
        s[5..7].parse::<u32>().unwrap_or(0),
        s[8..10].parse::<u32>().unwrap_or(0),
    );
    (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

// ── keys ────────────────────────────────────────────────────────────────────

/// Canonicalise free text into one key component.
///
/// One input, one key (invariant 6). Every run of non-alphanumerics collapses
/// to a single `-`, which is what makes two spellings of one event agree —
/// and, incidentally, what makes a separator unable to survive into a key.
/// *Both keys being unique is not the same as the identity being unique.*
pub fn slug(text: &str) -> Result<String> {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug = out.trim_matches('-');
    if slug.is_empty() {
        bail!("`{text}` has nothing a key can be made from");
    }
    Ok(slug.to_string())
}

/// What `{{slug}}` binds to for a surprise: the first sentence of what was
/// observed.
///
/// A sentence boundary rather than a word cap, deliberately. A cap is a number
/// nobody calibrated sitting in the identity path — reword one early word and
/// the cut moves, and the same event mints a second key.
pub fn slug_of_observation(observed: &str) -> Result<String> {
    let first = observed
        .find(['.', '!', '?'])
        .map(|i| &observed[..i])
        .unwrap_or(observed);
    slug(first)
}

/// Substitute `{{name}}` placeholders, and nothing else.
///
/// Imitates `adapter::expand`: the vocabulary is closed, so a template engine
/// would only buy the ability to put an expression in an identity. A
/// placeholder nothing binds is an error naming it, because the alternative is
/// a literal `{{slugg}}` inside a key that parses, round-trips, and is wrong
/// forever.
///
/// The result is validated here, at the mint, rather than by the caller: this
/// is the only place a key comes into existence, so a guard here is one every
/// future caller inherits instead of one each has to remember.
pub fn expand_key(template: &str, vars: &[(&str, &str)]) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .with_context(|| format!("`{template}`: a placeholder is never closed"))?;
        let name = after[..end].trim();
        let (_, value) = vars
            .iter()
            .find(|(k, _)| *k == name)
            .with_context(|| format!("`{template}` uses `{{{{{name}}}}}`, which nothing binds"))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    validate_key(&out)?;
    Ok(out)
}

// ── the clock ───────────────────────────────────────────────────────────────

/// A key is slash-separated slugs, and a key is also a path under the store.
///
/// `slug` guards what the *variable* can hold, but the template around it is
/// `.omh/memory.toml` — a committed file, so a clone supplies it — and
/// `expand_key` copies its literal text through untouched. Since `remember`
/// creates the key's parent directories, an unchecked template is an
/// arbitrary write. `auth::validate_name` and `session::validate_id` are the
/// precedent: validate identity where it is minted, not where it is used.
pub(crate) fn validate_key(key: &str) -> Result<()> {
    // An empty component covers more than it looks: `""` splits to one empty
    // part, `/abs` to a leading one, `a//b` and `a/` to an interior and a
    // trailing one. Spelling those out separately reads as thorough and is
    // untestable — no input can distinguish the extra arms from this one.
    let escapes = key.contains('\\')
        || key
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'));
    if escapes {
        bail!("`{key}` is not a key: a key is slash-separated slugs, never a path");
    }
    Ok(())
}

/// `YYYY-MM-DD` for today, UTC.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil(secs / 86_400)
}

/// Days since the epoch to a civil date. Hand-rolled because there is no date
/// crate here, and pure so the calendar can be tested without a clock —
/// `recorded` is the field the base-set post-mortem was written about.
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

// ── schema and lint ─────────────────────────────────────────────────────────

/// What can be wrong with a note.
///
/// Never a bool: `lint` prints these and `remember` refuses on them, so the
/// message is the whole product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// A file in the store that omh cannot read back as a note.
    ///
    /// A rule rather than a hard error, because `lint` is where you find out
    /// what is wrong with the store: aborting on the first bad file hid every
    /// other violation and reported nothing, on exactly the store that needed
    /// reporting most.
    Unreadable,
    UnclosedFence,
    MissingSection,
    ProseInListSection,
    KeyDisagreesWithPath,
    DuplicateKey,
    DanglingLink,
    /// A committed note links somewhere a fresh clone cannot follow.
    CrossLayerLink,
    /// `invalidated_by` names an expiry omh cannot evaluate, so the note
    /// advertises a freshness guarantee nothing will ever check.
    UnevaluatableTrigger,
    Orphan,
}

/// Schemas refuse; hygiene warns. Getting this backwards means an agent's
/// write fails because of somebody else's note — and agents negotiate with
/// warnings but cannot negotiate with a refused write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Refused,
    Warning,
}

impl Rule {
    /// Exhaustive on purpose, and with no `_` arm: a rule added without a
    /// severity fails to compile, which is a stronger guard than any test that
    /// iterates a `Rule::ALL` somebody has to remember to extend.
    pub fn severity(&self) -> Severity {
        match self {
            Self::Unreadable
            | Self::UnclosedFence
            | Self::MissingSection
            | Self::ProseInListSection
            | Self::KeyDisagreesWithPath
            | Self::UnevaluatableTrigger => Severity::Refused,
            Self::DuplicateKey | Self::DanglingLink | Self::CrossLayerLink | Self::Orphan => {
                Severity::Warning
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub key: String,
    pub layer: Layer,
    pub rule: Rule,
    pub detail: String,
}

/// A fence opener or closer: which character, and how many of it.
///
/// Three or more, backtick or tilde. Anything shorter is inline code.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == ch).count();
    (run >= 3).then_some((ch, run))
}

/// Every line, paired with whether a fenced block encloses it, plus whether
/// a fence was still open at the end.
///
/// One scanner for both readers, because `sections` and `links` disagreeing
/// about what is quoted is how a heading gets refused while the link beside
/// it is trusted. CommonMark's two rules that matter here: a fence closes
/// only on **the same character**, and only on a run **at least as long** —
/// without the second, a ````-wrapped example closes on the ``` it exists to
/// contain, which is the whole reason for a fourth backtick.
struct Fenced<'a> {
    lines: Vec<(&'a str, bool)>,
    unclosed: bool,
}

fn scan_fences(body: &str) -> Fenced<'_> {
    let mut open: Option<(char, usize)> = None;
    let mut lines = Vec::new();

    for line in body.lines() {
        let inside = match (open, fence_marker(line)) {
            (None, Some(marker)) => {
                open = Some(marker);
                true
            }
            (Some((och, orun)), Some((cch, crun))) if cch == och && crun >= orun => {
                open = None;
                true
            }
            (Some(_), _) => true,
            (None, _) => false,
        };
        lines.push((line, inside));
    }

    Fenced {
        lines,
        unclosed: open.is_some(),
    }
}

/// `## Name` and the lines beneath it, to the next heading of the same level.
///
/// Headings rather than substrings, because `body.contains("Expected")` is
/// satisfied by the word appearing in a sentence — the failure this repo
/// already shipped once as a staleness guard.
pub(crate) fn sections(body: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current: Option<&str> = None;
    // Nothing a fence encloses is a heading: the staged rules hand the agent
    // a fenced block containing these exact headings, so a schema that reads
    // inside one is satisfied by pasting back the example it was shown.
    for (line, quoted) in scan_fences(body).lines {
        let heading = if quoted {
            None
        } else {
            line.strip_prefix("## ")
        };
        // Quoted lines still count as the section's *content* — an `##
        // Evidence` holding a fenced command is full, not empty.
        if let Some(name) = heading {
            current = Some(name.trim());
            out.entry(name.trim()).or_default();
        } else if let Some(name) = current {
            out.entry(name).or_default().push(line);
        }
    }
    out
}

/// One note, in isolation. Pure, so every rule is a table test — and scoped to
/// one note, so a store-wide problem can never refuse somebody's write.
pub fn check(note: &Note) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut fire = |rule: Rule, detail: String| {
        found.push(Violation {
            key: note.key.clone(),
            layer: note.layer,
            rule,
            detail,
        })
    };

    // §8's closed set, asked here rather than at load: a note that already
    // exists must still be readable, and this is what makes the bad one
    // visible instead of fatal.
    if let Some(raw) = note.invalidated_by.as_deref().filter(|v| !v.is_empty()) {
        if let Err(e) = expiry::Trigger::parse(raw) {
            fire(Rule::UnevaluatableTrigger, format!("{e}"));
        }
    }

    // A fence left open swallows every heading and every link after it, so
    // the section checks below would report sections the writer can plainly
    // see, and `links` would report a neighbourhood that is not the note's.
    // Naming the fence is the only message here that is both true and
    // actionable, so it is the only one worth printing.
    if scan_fences(&note.body).unclosed {
        fire(
            Rule::UnclosedFence,
            "a code fence is never closed, so everything after it is quoted \
             — including the sections and links below it"
                .to_string(),
        );
        return found;
    }

    let sections = sections(&note.body);
    for name in note.kind.required_sections() {
        match sections.get(name) {
            None => fire(
                Rule::MissingSection,
                format!("a `{}` note needs a `## {name}` section", note.kind),
            ),
            // A heading with nothing under it is the obvious evasion, and it
            // costs nothing to close.
            Some(lines) if lines.iter().all(|l| l.trim().is_empty()) => {
                fire(Rule::MissingSection, format!("`## {name}` is empty"))
            }
            Some(_) => {}
        }
    }

    for name in note.kind.list_sections() {
        let Some(lines) = sections.get(name) else {
            continue;
        };
        // A continuation line belongs to the bullet above it. Requiring every
        // line to start with `- ` flags ordinary wrapped bullets, which trains
        // people to ignore the lint.
        if lines.iter().any(|l| {
            !l.trim().is_empty() && !l.starts_with('-') && !l.starts_with(char::is_whitespace)
        }) {
            fire(
                Rule::ProseInListSection,
                format!("`## {name}` holds bullets and nothing else"),
            );
        }
    }

    let stem = note
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let leaf = note.key.rsplit('/').next().unwrap_or(&note.key);
    if !stem.is_empty() && stem != leaf {
        fire(
            Rule::KeyDisagreesWithPath,
            format!("key `{}` is stored as `{stem}`", note.key),
        );
    }

    found
}

/// `[[wiki-link]]` targets in a body, in order of appearance.
///
/// Hand-rolled for the same reason the frontmatter parser is: the grammar is
/// one bracket pair, and a Markdown library would bring opinions about the
/// other ninety.
pub fn links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (line, quoted) in scan_fences(body).lines {
        // A link in a quoted command is a string, not a claim about the
        // graph — without this, `lint` reports a shell snippet as dangling.
        // A fence left open therefore hides every link after it, which is
        // why `check` refuses such a note outright rather than warning.
        if quoted {
            continue;
        }
        // Scanned per line, because a target does not span one. Scanning the
        // whole body let an unclosed `[[` — which `## Evidence` exists to
        // hold — run on to the *next* link's `]]` and swallow it, so `rm`
        // reported an empty neighbourhood for a note others pointed at.
        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else { break };
            let target = after[..end].trim();
            if !target.is_empty() && !target.contains("[[") {
                out.push(target.to_string());
            }
            rest = &after[end + 2..];
        }
    }
    out
}

/// Which layers a `[[key]]` written *in a note of `from`* actually reaches.
///
/// A set, never a winner — that is the whole of §4. Two claims about one topic
/// are two facts, and picking one would hide a teammate's note behind yours.
///
/// The asymmetry is invariant 2: from `local` a key reaches whatever holds it,
/// but from `team` it reaches only `team`, because a committed note is read in
/// a clone where no local layer exists.
///
/// It is one place to be right, not a type that refuses to be wrong — the
/// return is `Vec<Layer>` whichever way it is asked, so the rule lives in the
/// filter below rather than in the signature. Worth saying because the
/// stronger claim invites trusting a `Vec<Layer>` obtained "from team" to be
/// safe by construction, and `recall`'s neighbourhood expansion already
/// resolves links without asking here.
pub fn resolve(notes: &[Note], key: &str, from: Layer) -> Vec<Layer> {
    let mut found: Vec<Layer> = notes
        .iter()
        .filter(|n| n.key == key)
        .map(|n| n.layer)
        .filter(|layer| !from.is_committed() || layer.is_committed())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The links a note carries that exist here but would not exist in a fresh
/// clone. A link to a key nobody wrote is the lint's `DanglingLink`, not this:
/// it is already broken everywhere, and counting it here would refuse a
/// promotion for a reason promotion cannot fix.
///
/// The one predicate both `lint` and `promote` call. Two implementations of
/// "what would dangle for a teammate" is the shape that once had two
/// subsystems telling two stories about one file.
///
/// `also_committed` is everything the caller *asked* to promote, not the
/// subset that will succeed, so a plan can be checked against its own closure
/// — otherwise two notes that point at each other are unpromotable in either
/// order, with an error that reads like a bug. That it is the request rather
/// than the outcome is safe only because one blocker aborts the whole batch:
/// a partial promotion would let a key that was itself blocked go on vouching
/// for its neighbours.
///
/// Takes the note, not its key. Identity is `(layer, key)` and `DuplicateKey`
/// is only a warning, so a key can name more than one file: looking one up
/// here judged every claimant by the first match's body, which hid the
/// offender's links behind a clean namesake and reported the clean one twice.
/// Both callers already hold the note.
pub fn uncommitted_links(notes: &[Note], note: &Note, also_committed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = links(&note.body)
        .into_iter()
        .filter(|target| {
            !also_committed.contains(target)
                && resolve(notes, target, Layer::Team).is_empty()
                // A link to a key nobody wrote is dangling, which the lint
                // already reports. Counting it here too would refuse a
                // promotion for a reason `promote` cannot fix.
                && !resolve(notes, target, Layer::Local).is_empty()
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Store-wide checks: links that point nowhere, notes nothing points at.
///
/// Takes the whole store because that is the only thing that can answer it,
/// and warns rather than refuses because the note at fault is often not the
/// note being written.
pub fn hygiene(notes: &[Note]) -> Vec<Violation> {
    // Resolution ignores the layer: §4 says a key present in either layer
    // retrieves, so a link across layers is not dangling.
    let known: std::collections::BTreeSet<&str> = notes.iter().map(|n| n.key.as_str()).collect();
    let mut pointed_at: std::collections::BTreeSet<String> = Default::default();
    let mut found = Vec::new();

    for note in notes {
        // Invariant 2, checked here and again at `promote`. Warns rather than
        // refuses: the note at fault is committed, and an agent writing right
        // now cannot fix somebody else's.
        //
        // The layer decides whether to ask, not what to do with the answer:
        // asking first and discarding the result for local notes computed the
        // whole predicate for every note in the store to throw most of it away.
        if note.layer.is_committed() {
            for target in uncommitted_links(notes, note, &[]) {
                found.push(Violation {
                    key: note.key.clone(),
                    layer: note.layer,
                    rule: Rule::CrossLayerLink,
                    detail: format!(
                        "`{}` is committed but links to `{target}`, which is not — \
                         a fresh clone would not have it",
                        note.key
                    ),
                });
            }
        }
        for target in links(&note.body) {
            if known.contains(target.as_str()) {
                pointed_at.insert(target);
            } else {
                found.push(Violation {
                    key: note.key.clone(),
                    layer: note.layer,
                    rule: Rule::DanglingLink,
                    detail: format!(
                        "`{}` links to `{target}`, which is not in the store",
                        note.key
                    ),
                });
            }
        }
    }

    // §6 makes a key a primary key, and `remember` refuses to break that —
    // but hand-written notes are the only writer M1 gives the agent, so the
    // store can already hold two. Per layer, because §4 makes `team/deploy`
    // and `local/deploy` two notes on purpose.
    let mut by_key: BTreeMap<(Layer, &str), Vec<&Path>> = BTreeMap::new();
    for note in notes {
        by_key
            .entry((note.layer, note.key.as_str()))
            .or_default()
            .push(&note.path);
    }
    for ((layer, key), mut paths) in by_key {
        if paths.len() < 2 {
            continue;
        }
        paths.sort();
        found.push(Violation {
            key: key.to_string(),
            layer,
            rule: Rule::DuplicateKey,
            detail: format!(
                "`{key}` is claimed by {} files: {}",
                paths.len(),
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }

    for note in notes {
        // An orphan is a note *nothing links to*. Inverting this flags every
        // leaf, which is the opposite of what the count is for.
        //
        // **The entry point is expected to be one, and that is deliberate.**
        // `omh init` seeds a hub, nothing links to a hub by construction, and
        // `a_freshly_ingested_store_is_not_entirely_orphans` asserts exactly
        // that shape. A review found the resulting warning on a fresh repo
        // unwelcome — omh naming a note omh had just written — and the fix
        // tried here was to exempt a note that links outward. It contradicts
        // two named invariants, so it was reverted rather than argued with in
        // a commit message. If the warning is worth removing, it is the
        // report's presentation of the entry point that should change, not
        // what the word means.
        if !pointed_at.contains(&note.key) {
            found.push(Violation {
                key: note.key.clone(),
                layer: note.layer,
                rule: Rule::Orphan,
                detail: format!("nothing in the store links to `{}`", note.key),
            });
        }
    }

    found
}

/// Every `.md` under `dir`, including inside namespaces — a key may carry one,
/// so the store has directories in it.
fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // An absent store is empty; anything else is a real failure and must
        // not be mistaken for one.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        // `file_type` rather than `is_dir`, because `is_dir` follows links:
        // one symlinked namespace and the store answers with files that are
        // not in it, and `rm` deletes one of them. A link is not a note, so
        // it is not followed and not collected — and `contained` refuses the
        // write that would put one there.
        let kind = entry
            .file_type()
            .with_context(|| format!("reading {}", entry.path().display()))?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            markdown_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Refuse a path that resolves outside the layer it claims to be in.
///
/// `validate_key` reads a key's spelling; this reads the filesystem. The
/// local store is bind-mounted writable into the sandbox, so the agent can
/// put a symlink in it, and `{key}.md` beneath one resolves anywhere —
/// exit 0, reporting a path it did not write to. Checked against the deepest
/// ancestor that exists, because the note itself does not yet.
fn contained(root: &Path, path: &Path) -> Result<()> {
    let anchor = root
        .canonicalize()
        .with_context(|| format!("resolving {}", root.display()))?;

    let mut existing = path.to_path_buf();
    while !existing.exists() && existing.pop() {}
    let resolved = existing
        .canonicalize()
        .with_context(|| format!("resolving {}", existing.display()))?;

    if !resolved.starts_with(&anchor) {
        bail!(
            "{} resolves outside the store, so omh will not write there",
            path.display()
        );
    }
    Ok(())
}

/// Every note in one directory, stamped with the layer that directory *is*.
///
/// Takes a directory rather than `Paths` because the MCP server runs inside
/// the sandbox, where there is no git repo to discover and the two stores
/// arrive as mount points. `Paths` is the host's way of naming them, not the
/// only way.
pub fn notes_in(root: &Path, layer: Layer) -> Result<Vec<Note>> {
    let mut files = Vec::new();
    markdown_files(root, &mut files)?;
    files.sort();

    files
        .iter()
        .map(|path| {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            // Never `filter_map(..ok())`: a store that silently drops a note
            // answers from a subset and says nothing about the gap.
            parse(&raw, layer, path)
        })
        .collect()
}

pub fn load_layer(paths: &Paths, layer: Layer) -> Result<Vec<Note>> {
    notes_in(&layer.dir(paths), layer)
}

/// What a layer holds: the keys it already claims, and the files it could
/// not be asked about.
///
/// The second field is the point. Skipping an unparseable note answers "is
/// this key free?" with "yes" to a question whose true answer may be "no",
/// and the caller acts on that by *writing* — so the gap has to travel with
/// the answer instead of being swallowed by a `filter_map(..ok())`.
struct LayerRead {
    notes: Vec<Note>,
    /// Files that exist but cannot be read back as notes, so whatever key
    /// they hold is unknown.
    opaque: Vec<PathBuf>,
}

/// Takes a directory rather than `Paths` for the same reason `notes_in` does:
/// `remember_in` runs against a mount point inside the sandbox, where there is
/// no repo to derive a layer directory from — and it is the caller that most
/// needs `opaque`, because it is about to write.
fn read_layer(root: &Path, layer: Layer) -> Result<LayerRead> {
    let mut files = Vec::new();
    // Traversal errors stay fatal: a directory omh cannot list may hold the
    // key a write is about to take, and guessing is the failure this whole
    // function exists to avoid.
    markdown_files(root, &mut files)?;
    files.sort();

    let mut notes = Vec::new();
    let mut opaque = Vec::new();
    for path in files {
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|raw| parse(&raw, layer, &path))
        {
            Ok(note) => notes.push(note),
            Err(_) => opaque.push(path),
        }
    }
    Ok(LayerRead { notes, opaque })
}

/// Both layers. Never merged and never deduped: `team/deploy` and
/// `local/deploy` are two notes, and both retrieve.
pub fn load(paths: &Paths) -> Result<Vec<Note>> {
    let mut all = Vec::new();
    for layer in Layer::ALL {
        all.extend(load_layer(paths, layer)?);
    }
    Ok(all)
}

// ── key templates ───────────────────────────────────────────────────────────

/// Seeded by `init` at `<repo>/.omh/memory.toml`, with `write_if_absent` and
/// never refreshed: a shipped template that changed under an existing store
/// would silently re-key every note in it.
/// Where the repo's memory configuration lives.
///
/// It was `keys.toml`, which named one table as though it were the whole
/// subsystem — key templates are one part of how the note store is configured,
/// and expiry has settings of its own coming. `settings.toml` /
/// `settings.local.toml` say what the file *is* rather than what one of its
/// tables holds, and this follows them.
pub const TEMPLATES: &str = "memory.toml";

pub const SHIPPED_KEYS: &str = "\
# How a note's key is derived. Identity, not a title — the agent never picks
# one, so the same observation cannot be recorded twice under two spellings.
[keys]
surprise = \"surprise/{{slug}}\"
topic    = \"{{slug}}\"
stub     = \"{{path}}\"
";

/// What omh ships, parsed. Infallible by construction — a shipped constant
/// that does not parse is a build-time defect, and the test that says so is
/// `the_shipped_key_templates_cover_every_note_type`.
pub fn shipped_templates() -> BTreeMap<Kind, String> {
    parse_templates(SHIPPED_KEYS).expect("the shipped key templates must parse")
}

fn parse_templates(raw: &str) -> Result<BTreeMap<Kind, String>> {
    #[derive(serde::Deserialize)]
    struct File {
        keys: BTreeMap<String, String>,
    }
    let file: File = toml::from_str(raw).context("reading key templates")?;
    file.keys
        .into_iter()
        .map(|(name, template)| Ok((name.parse::<Kind>()?, template)))
        .collect()
}

/// The repo's key templates, falling back to what omh ships.
///
/// A missing file is the shipped set rather than an error: `remember` has to
/// work in a repo where `init` has not run, and inventing a key on the spot is
/// the one thing §6 forbids.
pub fn templates(paths: &Paths) -> Result<BTreeMap<Kind, String>> {
    let path = paths.repo.join(".omh").join(TEMPLATES);

    // Checked before the read, not only when the new file is absent. `init`
    // writes `memory.toml` whether or not a `keys.toml` is beside it, so "both
    // present" is the *likely* state after a half-finished move — and it was
    // the one state this said nothing about, while the message it would have
    // printed says "rather than leaving both".
    //
    // A check rather than a fallback, because the fallback is the disaster:
    // `templates` reads a missing file as "use the shipped defaults", so an
    // edited `keys.toml` would silently re-key every note written from then on
    // while every existing key stopped being derivable from anything.
    let stale = paths.repo.join(".omh").join("keys.toml");
    if stale.exists() {
        // Built line by line rather than as one `\`-continued literal.
        // `cargo fmt` re-indents a continued string and the leading
        // whitespace survives into the message, so the user reads a
        // wall of spaces mid-sentence. It has happened twice here.
        let mut msg = format!("{} is where key templates used to live.\n", stale.display());
        msg.push_str(&format!("They are read from {} now.\n", path.display()));
        msg.push_str("Rename it rather than leaving both: the shipped defaults ");
        msg.push_str("would silently re-key every note written from here on, ");
        msg.push_str("and every existing key would stop being derivable.");
        anyhow::bail!(msg);
    }

    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_templates(&raw).with_context(|| format!("{}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parse_templates(SHIPPED_KEYS),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// `omh memory lint` — schema and hygiene, over both layers.
///
/// Also the store-quality meter: violation counts are a write-time proxy for
/// store quality, available with no questions asked and no model pass.
pub fn lint(paths: &Paths) -> Result<Vec<Violation>> {
    // Read leniently *here specifically*, and turn what could not be read
    // into violations. `load` stays strict because a query answered from a
    // subset is a wrong answer; `lint` is the one caller whose whole job is
    // to describe the store including its damage, so giving up on the first
    // bad file is the one thing it must not do.
    let mut notes = Vec::new();
    let mut found = Vec::new();
    for layer in Layer::ALL {
        let read = read_layer(&layer.dir(paths), layer)?;
        for path in read.opaque {
            found.push(Violation {
                key: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                layer,
                rule: Rule::Unreadable,
                detail: format!("{} is in the store but omh cannot read it", path.display()),
            });
        }
        notes.extend(read.notes);
    }

    found.extend(notes.iter().flat_map(check));
    found.extend(hygiene(&notes));
    found.sort_by(|a, b| (a.rule, &a.key).cmp(&(b.rule, &b.key)));
    Ok(found)
}

/// How many of these the schema refuses.
///
/// §14 makes `lint` M1's stand-in for the refused write the agent does not
/// get yet, and a command that always exits 0 cannot stand in for anything —
/// no hook, no CI step and no `&&` can read it. Only refusals decide: hygiene
/// warns about the store as a whole, and `Orphan` fires on every note nothing
/// links to, so gating on warnings would gate on the store's shape.
pub fn refused(violations: &[Violation]) -> usize {
    violations
        .iter()
        .filter(|v| v.rule.severity() == Severity::Refused)
        .count()
}

pub fn tally(violations: &[Violation]) -> BTreeMap<Rule, usize> {
    let mut counts = BTreeMap::new();
    for v in violations {
        *counts.entry(v.rule).or_insert(0) += 1;
    }
    counts
}

// ── remember ────────────────────────────────────────────────────────────────

/// `remember`'s parameters (§9.1) plus the two omh supplies.
///
/// A struct rather than seven arguments because the signature *is* the
/// discipline: adding a field is a visible change to what a writer must
/// supply, and provenance becomes a parameter that cannot be omitted rather
/// than a rule that can be violated.
#[derive(Debug, Clone, Default)]
pub struct Remembered {
    pub expected: String,
    pub observed: String,
    pub evidence: String,
    /// Questions this note answers, in the writer's own words.
    ///
    /// The one thing an algorithm cannot supply and the agent can: it is the
    /// only party that knows what it was trying to find out. Matching a
    /// question against a question is what survives a paraphrase; matching a
    /// question against prose is what does not.
    pub answers: Vec<String>,
    /// Keys, not titles — a key is computable before its target exists.
    pub relates_to: Vec<String>,
    pub invalidated_by: Option<String>,
    /// `session s03, claude`. omh's to supply, never the agent's.
    pub source: String,
    /// Passed in, so the writer is testable without a clock.
    pub recorded: String,
}

/// Retry policy as an argument. Skip-if-exists is an explicit mode, not a
/// fallback — as a fallback it makes every genuine conflict vanish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IfExists {
    #[default]
    Error,
    Skip,
    Suffix,
    Override,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wrote {
    Created(PathBuf),
    /// An `--if-exists override` that destroyed a note to make room. Its own
    /// variant because reporting it as a creation is how a destructive write
    /// passes for a harmless one, and §6 only permits it when it was named.
    Replaced(PathBuf),
    Skipped(String),
}

/// Parse a trigger, and rewrite a `file:` path relative to the repo.
///
/// Parsing here as well as at load is what makes §8's set closed at the only
/// door an agent uses.
/// The write path's share of §8: fold what only makes sense here, and refuse
/// what omh could never evaluate.
///
/// Spelled out rather than ending in a binding catch-all. This function exists
/// because a value that means one thing where it was written means another
/// where it is read, and `other => other.render()` silently opts each new kind
/// out of exactly that — writing it through un-normalised, which is the bug
/// this was added to prevent.
fn normalise_trigger(raw: &str, repo: &Path) -> Result<String> {
    Ok(match expiry::Trigger::parse(raw)? {
        // Recorded from inside the sandbox, read from outside it.
        expiry::Trigger::File { path, hash } => expiry::Trigger::File {
            path: expiry::normalise_path(&path, repo),
            hash,
        }
        .render(),
        // `current` is a request, not a value: resolve it while omh is the one
        // holding the recipe, so what lands on disk is a digest a later `stale`
        // can compare against.
        expiry::Trigger::Image { digest } if digest == expiry::IMAGE_NOW => {
            let now = crate::image::recipe_digest(&crate::image::base_dockerfile())
                .context("recording what the image recipe is right now")?;
            expiry::Trigger::Image { digest: now }.render()
        }
        t @ (expiry::Trigger::Image { .. }
        | expiry::Trigger::Base { .. }
        | expiry::Trigger::Symbol { .. }) => t.render(),
    })
}

fn non_blank(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("`{name}` is empty; there is nothing here worth recording");
    }
    Ok(())
}

fn body_of(input: &Remembered) -> String {
    let mut body = format!(
        "# {}\n\n## Expected\n{}\n\n## Observed\n{}\n\n## Evidence\n{}\n",
        input.observed.trim(),
        input.expected.trim(),
        input.observed.trim(),
        input.evidence.trim(),
    );
    body.push_str("\n## Answers\n\n");
    for question in &input.answers {
        body.push_str(&format!("- {}\n", question.trim()));
    }
    if !input.relates_to.is_empty() {
        // Sorted and deduped: the same neighbours in a different order are the
        // same note, and an unpinned order churns the file on every re-record.
        let mut related = input.relates_to.clone();
        related.sort();
        related.dedup();
        body.push_str("\n## Related\n\n");
        for key in related {
            body.push_str(&format!("- [[{key}]]\n"));
        }
    }
    body
}

/// Derive the key, build the note, check the schema, *then* create the file.
///
/// That order is the whole of `a_refused_write_leaves_nothing_on_disk`:
/// validating after writing leaves a refused note on disk, where `lint` blames
/// a writer that believes it never created one.
pub fn remember(paths: &Paths, input: &Remembered, if_exists: IfExists) -> Result<Wrote> {
    remember_in(
        &Layer::AGENT_WRITE.dir(paths),
        &paths.repo,
        &templates(paths)?,
        input,
        if_exists,
    )
}

/// `remember`, against an explicit directory.
///
/// `root` is where the note lands, and it is always the writable layer's
/// directory — the caller cannot pass the committed one, because no caller is
/// given the choice. In the sandbox this is a mount point rather than a path
/// derived from a repo.
pub fn remember_in(
    root: &Path,
    repo: &Path,
    templates: &BTreeMap<Kind, String>,
    input: &Remembered,
    if_exists: IfExists,
) -> Result<Wrote> {
    non_blank(&input.expected, "expected")?;
    non_blank(&input.observed, "observed")?;
    non_blank(&input.evidence, "evidence")?;
    non_blank(&input.source, "source")?;
    if input.answers.iter().all(|q| q.trim().is_empty()) {
        bail!("`answers` is empty; a note nobody can find is a note nobody wrote");
    }

    let template = templates
        .get(&Kind::Surprise)
        .context("no key template for `surprise`")?;
    let key = expand_key(
        template,
        &[("slug", &slug_of_observation(&input.observed)?)],
    )?;

    // Always the write layer, never a parameter. An unattended writer that
    // could reach the committed layer would push wrong facts to teammates
    // through git, where they arrive with the authority of a reviewed change.
    let layer = Layer::AGENT_WRITE;

    // §6 makes the key the primary key, so the conflict is on the *key*, not
    // on `{key}.md`. Those differ whenever a note sits somewhere other than
    // its own key — which nothing prevents, because `KeyDisagreesWithPath`
    // compares only the leaf. Checking the path let a second note land under
    // a key that was already taken, and `rm` could then separate neither.
    let taken = read_layer(root, layer)?;
    // A note omh cannot read may hold the key this write wants. Refusing is
    // the recoverable half of that choice — the other half is a second note
    // under a key that already existed, which is unrecoverable through the
    // CLI and invisible until somebody tries to `rm` one of them.
    if let Some(opaque) = taken.opaque.first() {
        bail!(
            "{} is in the store but omh cannot read it, so it cannot tell whether `{key}` is \
             free — fix or remove that note first",
            opaque.display()
        );
    }
    let held_at = |k: &str| {
        taken
            .notes
            .iter()
            .find(|note| note.key == k)
            .map(|note| note.path.clone())
    };

    // `replacing` is the note this write destroys, if any — carried this far
    // so the return value can say so instead of calling it a creation.
    let (key, path, replacing) = match if_exists {
        IfExists::Suffix => {
            let mut candidate = key.clone();
            let mut n = 1;
            while held_at(&candidate).is_some() || root.join(format!("{candidate}.md")).exists() {
                n += 1;
                candidate = format!("{key}-{n}");
            }
            let path = root.join(format!("{candidate}.md"));
            (candidate, path, None)
        }
        _ => match held_at(&key) {
            Some(existing) => match if_exists {
                IfExists::Skip => return Ok(Wrote::Skipped(key)),
                // The replacement lands at the key's own path and the old
                // file goes, so overriding *restores* invariant 5 rather than
                // preserving a mislocated note. Writing to `existing` instead
                // made the key disagree with its filename, and `check` then
                // refused the very write it had been told to force.
                IfExists::Override => {
                    let path = root.join(format!("{key}.md"));
                    (key, path, Some(existing))
                }
                IfExists::Error => {
                    bail!("`{key}` is already recorded; update that note instead")
                }
                IfExists::Suffix => unreachable!("handled by the arm above"),
            },
            None => {
                let path = root.join(format!("{key}.md"));
                // Nothing holds the key, but something holds its file: a note
                // whose own key disagrees with where it is stored. Refusing
                // beats overwriting a note this write does not own.
                if path.exists() {
                    bail!(
                        "{} already holds a note that does not claim `{key}` — `omh memory lint` says which",
                        path.display()
                    );
                }
                (key, path, None)
            }
        },
    };

    let note = Note {
        key,
        kind: Kind::Surprise,
        source: input.source.trim().to_string(),
        recorded: input.recorded.clone(),
        // Stored repo-relative. The agent records what it sees, which inside
        // the sandbox is `/work/...`; host-side that file does not exist, and
        // every `file:` trigger would report stale on day one.
        invalidated_by: input
            .invalidated_by
            .as_deref()
            .map(|raw| normalise_trigger(raw, repo))
            .transpose()?,
        body: body_of(input),
        layer,
        path: path.clone(),
    };

    // Round-tripped through the parser before it is trusted: a note omh cannot
    // read back is a note the store cannot serve.
    let rendered = render(&note);
    let note = parse(&rendered, layer, &path)?;

    // Only the schema refuses. Hygiene is store-wide, and a store-wide problem
    // must never fail somebody else's write.
    let refused = check(&note);
    if let Some(first) = refused.first() {
        bail!("`{}` was not written: {}", note.key, first.detail);
    }

    // The root has to exist to be resolved, and it is the one directory that
    // is trivially inside itself. Everything below it is checked before it is
    // created, so a symlinked namespace cannot be walked into on the way.
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    contained(root, &path)?;
    std::fs::create_dir_all(path.parent().unwrap())
        .with_context(|| format!("creating {}", root.display()))?;
    std::fs::write(&path, &rendered).with_context(|| format!("writing {}", path.display()))?;

    match replacing {
        Some(stale) => {
            // Unlinked after the replacement is safely on disk, and only when
            // it is a different file — one key must end up with one note.
            if stale != path {
                std::fs::remove_file(&stale)
                    .with_context(|| format!("removing {}", stale.display()))?;
            }
            Ok(Wrote::Replaced(path))
        }
        None => Ok(Wrote::Created(path)),
    }
}

// ── rm ──────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub struct Removed {
    pub path: PathBuf,
    /// The layer it came out of. Removing a committed note changes what
    /// teammates get, and that is worth saying out loud.
    pub layer: Layer,
    /// Keys that pointed at what was just removed, from either layer.
    pub inbound: Vec<String>,
}

/// Which of several notes under one key the caller meant.
///
/// `--layer` answers this only when the layers differ. Two notes under one
/// key in one layer used to produce "`k` is in local and local", and
/// `--layer local` produced it again — the note could not be reached through
/// omh at all, in a store deliberately kept outside the checkout.
fn disambiguate<'a>(
    paths: &Paths,
    many: &[&'a Note],
    key: &str,
    at: Option<&str>,
) -> Result<&'a Note> {
    // Relative to the layer's root, which is what `--at` takes: the store's
    // absolute path is noise the caller did not type.
    let shown = |note: &Note| {
        note.path
            .strip_prefix(note.layer.dir(paths))
            .unwrap_or(&note.path)
            .display()
            .to_string()
    };

    if let Some(at) = at {
        let picked: Vec<&&Note> = many.iter().filter(|n| n.path.ends_with(at)).collect();
        let spans_layers = |notes: &[&&Note]| {
            notes
                .iter()
                .map(|n| n.layer)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1
        };
        return match picked.as_slice() {
            [one] => Ok(**one),
            [] => bail!(
                "no note `{key}` at `{at}` — it is in {}",
                many.iter().map(|n| shown(n)).collect::<Vec<_>>().join(", ")
            ),
            // Two layers can hold the same relative path, and then `shown`
            // renders both identically — so "give more of the path" asks for
            // something the caller does not have. The layer is the only
            // thing that separates them.
            rest if spans_layers(rest) => bail!(
                "`{at}` matches {} notes, in {} — name one with --layer",
                rest.len(),
                rest.iter()
                    .map(|n| n.layer.to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            rest => bail!(
                "`{at}` matches {} of them — give more of the path",
                rest.len()
            ),
        };
    }

    let layers: Vec<String> = many.iter().map(|n| n.layer.to_string()).collect();
    if layers
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1
    {
        bail!(
            "`{key}` is in {} — name one with --layer",
            layers.join(" and ")
        );
    }
    bail!(
        "`{key}` is one key over {} files in {} — name one with --at: {}",
        many.len(),
        layers[0],
        many.iter().map(|n| shown(n)).collect::<Vec<_>>().join(", ")
    )
}

/// One note. Never a neighbour, never a link rewrite.
///
/// A dangling link is visible and the lint already finds it; a silently pruned
/// neighbourhood is neither. Fail toward the recoverable mistake.
/// `dry_run` resolves the note and counts what pointed at it, then leaves the
/// file alone. Everything that decides *which* note — the layer filter, `--at`,
/// the ambiguity refusal — runs either way, because a preview that skipped them
/// would be previewing a different deletion.
pub fn remove(
    paths: &Paths,
    layer: Option<Layer>,
    key: &str,
    at: Option<&str>,
    dry_run: bool,
) -> Result<Removed> {
    let notes = load(paths)?;
    let matching: Vec<&Note> = notes
        .iter()
        .filter(|n| n.key == key && layer.is_none_or(|l| n.layer == l))
        .collect();
    // `--at` is applied whenever it is given, not only when the key is
    // ambiguous. Consulting it only in the `many` arm meant naming a file
    // that does not hold the key deleted a *different* note and reported
    // success — the caller reached for `--at` to be careful, and it was the
    // one input that could not be wrong.
    let note = match (matching.as_slice(), at) {
        ([], _) => bail!("no note `{key}`"),
        ([one], None) => *one,
        (many, _) => disambiguate(paths, many, key, at)?,
    };

    let mut inbound: Vec<String> = notes
        .iter()
        .filter(|n| n.key != key && links(&n.body).iter().any(|t| t == key))
        .map(|n| n.key.clone())
        .collect();
    inbound.sort();
    inbound.dedup();

    if !dry_run {
        std::fs::remove_file(&note.path)
            .with_context(|| format!("removing {}", note.path.display()))?;
    }

    Ok(Removed {
        path: note.path.clone(),
        layer: note.layer,
        inbound,
    })
}

// ── listing and the review moment ───────────────────────────────────────────

/// `omh memory`'s output. Pure, and every line carries the note's own date and
/// its own layer: a note presented without age and origin cannot be judged.
pub fn render_list(notes: &[Note]) -> String {
    let mut sorted: Vec<&Note> = notes.iter().collect();
    sorted.sort_by(|a, b| (a.layer, &a.key).cmp(&(b.layer, &b.key)));

    let width = sorted.iter().map(|n| n.key.len()).max().unwrap_or(0);
    let mut out = String::new();
    for note in sorted {
        let refs = notes
            .iter()
            .filter(|n| links(&n.body).contains(&note.key))
            .count();
        out.push_str(&format!(
            "{:width$}  {:<5}  {}  {} ref{}\n",
            note.key,
            note.layer.to_string(),
            note.recorded,
            refs,
            if refs == 1 { "" } else { "s" },
        ));
    }
    out
}

/// The session an `omh`-written note came from, if its provenance says so.
///
/// Parsed rather than searched: `source.contains(id)` makes `s1` match `s10`,
/// which is the same canonicalisation failure as two spellings of one key.
fn session_of(source: &str) -> Option<&str> {
    let rest = source.strip_prefix("session ")?;
    Some(rest.split(',').next()?.trim())
}

pub fn from_session<'a>(notes: &'a [Note], session: &str) -> Vec<&'a Note> {
    notes
        .iter()
        .filter(|n| session_of(&n.source) == Some(session))
        .collect()
}

/// §12's review moment, riding on something already happening. `None` when the
/// session recorded nothing — an unconditional `0 notes` line is noise, and it
/// trains people to stop reading the removal report.
pub fn session_nudge(notes: &[Note], session: &str) -> Option<String> {
    let n = from_session(notes, session).len();
    (n > 0).then(|| {
        format!(
            "{n} note{} recorded during this session — `omh memory` to review",
            if n == 1 { "" } else { "s" }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        (dir, paths)
    }

    /// The rules every session is given, as omh generates them. The note
    /// protocol is one of them, and these guards are about what the agent is
    /// actually handed rather than about any file on disk.
    fn shipped_rules() -> String {
        crate::base::sections()
            .into_iter()
            .map(|s| s.body)
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ── layers ──────────────────────────────────────────────────────────────

    /// Invariant 3, as a constant rather than a rule. The risk this guards is
    /// someone "generalising" `remember` with a layer parameter, or flipping
    /// the constant while every other test stays green.
    #[test]
    fn the_layer_remember_writes_to_is_never_committed() {
        assert_eq!(Layer::AGENT_WRITE, Layer::Local);
        assert!(
            !Layer::AGENT_WRITE.is_committed(),
            "an unattended writer must not reach the committed layer"
        );
        // The negative half: without it, `is_committed` could be `=> false`
        // and this test would still pass — which this repo has shipped twice.
        assert!(Layer::Team.is_committed());
    }

    /// The local store must not live in the checkout, and the team store must.
    /// Asserted as a relationship to the repo root rather than as literal
    /// paths, so a refactor that moves `~/.omh` does not force an edit here.
    #[test]
    fn the_local_store_lives_outside_the_checkout_and_the_team_store_inside_it() {
        let (_d, paths) = fixture();

        assert!(
            Layer::Team.dir(&paths).starts_with(&paths.repo),
            "the team layer must be committable: {}",
            Layer::Team.dir(&paths).display()
        );
        assert!(
            !Layer::Local.dir(&paths).starts_with(&paths.repo),
            "a local note inside the checkout dies with the worktree: {}",
            Layer::Local.dir(&paths).display()
        );
        assert!(
            Layer::Local.dir(&paths).starts_with(&paths.root),
            "the local store belongs to omh, keyed by repo"
        );
    }

    /// Two repos must not share one local store. `repo_id` is what keys it,
    /// and a `notes()` that ignored the repo would silently pool every
    /// project's notes into one graph.
    #[test]
    fn two_repos_do_not_share_a_local_store() {
        let dir = tempfile::tempdir().unwrap();
        let a = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("alpha"),
        };
        let b = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("beta"),
        };
        assert_ne!(Layer::Local.dir(&a), Layer::Local.dir(&b));
    }

    // ── notes ───────────────────────────────────────────────────────────────

    /// A note with every field filled, as bytes. Built by hand rather than by
    /// `render`, so a round-trip test cannot pass by comparing a renderer with
    /// itself.
    const SURPRISE: &str = "\
---
key: surprise/mounting-a-credential-file-returns-ebusy
type: surprise
source: session s03, claude
recorded: 2026-08-07
invalidated_by: image:4f2a1c3b5d7e9f0a2b4c6d8e0f1a3b5c7d9e0f1a
---

# Mounting a credential file returns EBUSY

## Expected
A bind mount of the token file to persist the login.

## Observed
The harness rewrites in place; a file mount is one inode, so the write fails.

## Evidence
`EBUSY` from the mount syscall.

## Related

- [[credentials-are-a-named-volume]]
";

    fn parsed() -> Note {
        parse(SURPRISE, Layer::Local, std::path::Path::new("x.md")).unwrap()
    }

    /// Both directions, against bytes nobody generated. A renderer that emits
    /// `type: Surprise` while the parser expects `surprise`, or that writes an
    /// empty `invalidated_by:` line for `None`, passes every semantic
    /// assertion and fails this one.
    #[test]
    fn a_note_round_trips_through_its_own_parser() {
        let note = parsed();
        assert_eq!(
            parse(&render(&note), note.layer, &note.path).unwrap(),
            note,
            "render must produce bytes parse accepts"
        );

        let mut bare = note.clone();
        bare.invalidated_by = None;
        assert_eq!(parse(&render(&bare), bare.layer, &bare.path).unwrap(), bare);
        assert!(
            !render(&bare).contains("invalidated_by"),
            "an absent trigger must not render as an empty one:\n{}",
            render(&bare)
        );
    }

    /// Invariant 4. Each field dropped in turn, and the error must name the
    /// field that is missing — a parser that reads an absent key as
    /// `String::new()` produces a note with an empty identity instead.
    #[test]
    fn a_note_missing_a_required_field_is_refused_by_name() {
        for field in ["key", "type", "source", "recorded"] {
            let without: String = SURPRISE
                .lines()
                .filter(|l| !l.starts_with(&format!("{field}:")))
                .collect::<Vec<_>>()
                .join("\n");
            let err = parse(&without, Layer::Local, std::path::Path::new("x.md"))
                .unwrap_err()
                .to_string();
            assert!(err.contains(field), "dropping `{field}` gave: {err}");
        }
    }

    /// The weak version of the test above is `contains_key`, which is
    /// satisfied by a field that is present and says nothing. This repo has
    /// shipped that exact shape twice: a hook whose `command` defaulted to
    /// `""`, and a date guard that only checked a date was present.
    #[test]
    fn a_required_field_present_but_empty_is_refused_too() {
        for field in ["key", "type", "source", "recorded"] {
            let blank: String = SURPRISE
                .lines()
                .map(|l| {
                    if l.starts_with(&format!("{field}:")) {
                        format!("{field}:   ")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let err = parse(&blank, Layer::Local, std::path::Path::new("x.md"));
            assert!(
                err.is_err(),
                "a blank `{field}` must be refused, not read as empty"
            );
            assert!(err.unwrap_err().to_string().contains(field));
        }
    }

    /// §5 writes this rule as `\d{4}-\d{2}-\d{2}`, which is the weak version:
    /// it accepts `2026-13-45`. A note's date is the only thing an undated
    /// claim can be judged by, so it must be a date that exists.
    #[test]
    fn the_recorded_date_must_be_a_real_calendar_date() {
        let with = |d: &str| {
            let raw = SURPRISE.replace("recorded: 2026-08-07", &format!("recorded: {d}"));
            parse(&raw, Layer::Local, std::path::Path::new("x.md"))
        };
        for bad in [
            "2026-13-45",
            "2026-00-10",
            "2026-08-32",
            "2026-8-7",
            "26-08-07",
            "abcd-ef-gh",
            "2025-02-29",
            "2026-08-07x",
        ] {
            assert!(with(bad).is_err(), "`{bad}` is not a date");
        }
        for good in ["2026-08-07", "2024-02-29", "2000-02-29", "1970-01-01"] {
            assert!(with(good).is_ok(), "`{good}` is a date");
        }
    }

    /// A `_ => Kind::Topic` arm silently reclassifies a typo'd note, which
    /// then gets the wrong required sections and stops being validated at all
    /// — total degradation, reported as success.
    #[test]
    fn an_unknown_note_type_is_an_error_not_a_default() {
        let raw = SURPRISE.replace("type: surprise", "type: hunch");
        let err = parse(&raw, Layer::Local, std::path::Path::new("x.md"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("hunch"), "got: {err}");
        for kind in Kind::ALL {
            assert!(err.contains(&kind.to_string()), "must name `{kind}`: {err}");
        }
    }

    /// The layer is where a note lives, never what it says. A note that can
    /// declare its own layer can lie about having been reviewed, which makes
    /// the provenance the whole feature rests on decorative.
    #[test]
    fn a_notes_layer_comes_from_where_it_lives_not_from_what_it_says() {
        let lying = SURPRISE.replace("type: surprise", "layer: team\ntype: surprise");
        let note = parse(&lying, Layer::Local, std::path::Path::new("x.md")).unwrap();
        assert_eq!(note.layer, Layer::Local);
        assert!(
            !render(&note).contains("layer:"),
            "the layer is not a field a note carries"
        );
    }

    /// The sections a surprise must carry are exactly what `remember` asks
    /// for. Tying them together is what stops a parameter being added to the
    /// signature and silently dropped on the way to disk, or a section being
    /// required that no writer can supply.
    #[test]
    fn the_sections_a_surprise_requires_are_the_ones_remember_supplies() {
        // Tied to the tool's own required arguments, so a parameter cannot be
        // added to one and forgotten in the other: a section nothing supplies
        // refuses every write, and an argument no section holds is discarded
        // on the way to disk.
        let supplied: Vec<String> = crate::memory::tools::REQUIRED
            .iter()
            .map(|a| {
                let mut c = a.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(Kind::Surprise.required_sections(), supplied.as_slice());
    }

    /// A list-like section that is not also declared somewhere is a rule that
    /// can never fire. Every kind must answer both questions.
    #[test]
    fn every_note_type_declares_both_its_section_tables() {
        for kind in Kind::ALL {
            let required = kind.required_sections();
            let lists = kind.list_sections();
            assert!(
                !required.is_empty() || !lists.is_empty(),
                "`{kind}` is validated by nothing at all"
            );
            for name in lists {
                assert!(
                    !name.is_empty(),
                    "`{kind}` declares an unnamed list section"
                );
            }
        }
    }

    // ── keys ────────────────────────────────────────────────────────────────

    /// Invariant 6, and the whole of it. A key is a primary key, so two
    /// spellings of one event must not mint two of them.
    ///
    /// The naive `to_lowercase().replace(' ', "-")` passes none of these: a
    /// double space becomes `--`, a trailing full stop survives, and an em
    /// dash survives with its spaces. Each produces a second key for an event
    /// already recorded, and §6's conflict error never fires.
    #[test]
    fn every_spelling_of_one_observation_derives_the_same_key() {
        let want = "mounting-a-credential-file-returns-ebusy";
        for spelling in [
            "Mounting a credential FILE returns EBUSY",
            "mounting a  credential file returns ebusy",
            "Mounting a credential file returns EBUSY.",
            "Mounting a credential — file — returns EBUSY",
            "  Mounting a credential file returns EBUSY  ",
            "Mounting/a/credential/file/returns/EBUSY",
        ] {
            assert_eq!(slug(spelling).unwrap(), want, "from: {spelling:?}");
        }
    }

    /// A slug that passes a separator through is a key that is a path, and
    /// `remember` then writes wherever the agent's prose points. The same
    /// class as `auth::validate_name` and `session::validate_id`.
    #[test]
    fn a_key_never_contains_a_path_separator_or_a_leading_dot() {
        for hostile in [
            "../../.ssh/id_rsa",
            "a/b",
            "..",
            ".",
            "C:\\Windows",
            ".hidden",
        ] {
            let Ok(s) = slug(hostile) else { continue };
            assert!(!s.contains('/'), "{hostile:?} → {s:?}");
            assert!(!s.contains('\\'), "{hostile:?} → {s:?}");
            assert!(!s.starts_with('.'), "{hostile:?} → {s:?}");
            assert!(s != "." && s != "..", "{hostile:?} → {s:?}");
        }
    }

    /// An empty slug is a file called `.md` — a hidden file carrying the empty
    /// key, which then matches loosely everywhere downstream.
    #[test]
    fn an_empty_or_punctuation_only_input_is_an_error_not_an_empty_key() {
        for nothing in ["!!!", "   ", "", "---", "..."] {
            assert!(slug(nothing).is_err(), "{nothing:?} is not a key");
        }
    }

    /// A word cap is an uncalibrated number sitting in the identity path:
    /// reword one early word and the cut moves, minting a second key for one
    /// event. A sentence boundary has no number in it — §7 applied where it
    /// costs most.
    #[test]
    fn key_derivation_stops_at_a_sentence_boundary_not_a_word_count() {
        let key =
            slug_of_observation("The mount fails with EBUSY. It is one inode, so the write fails.")
                .unwrap();
        assert_eq!(key, "the-mount-fails-with-ebusy");
        assert!(
            !key.contains("inode"),
            "the second sentence is not identity"
        );

        // Rewording early must not move the cut, which is what a `take(n)`
        // word cap cannot promise.
        let reworded =
            slug_of_observation("The bind mount fails with EBUSY. Something else.").unwrap();
        assert_eq!(reworded, "the-bind-mount-fails-with-ebusy");

        // No terminator at all is still one sentence, not an error.
        assert_eq!(
            slug_of_observation("no full stop here").unwrap(),
            "no-full-stop-here"
        );
    }

    /// `template.replace("{{slug}}", s)` leaves a typo'd `{{slugg}}` sitting
    /// in the output, where it becomes part of a key — unique, permanent, and
    /// wrong. Identity is the one place a silent substitution cannot be
    /// tolerated.
    #[test]
    fn the_key_template_substitutes_only_the_placeholders_it_knows() {
        assert_eq!(
            expand_key("surprise/{{slug}}", &[("slug", "ebusy")]).unwrap(),
            "surprise/ebusy"
        );

        let err = expand_key("surprise/{{slugg}}", &[("slug", "ebusy")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("slugg"), "must name the placeholder: {err}");

        assert!(
            expand_key("surprise/{{slug", &[("slug", "ebusy")]).is_err(),
            "an unclosed placeholder is not a literal"
        );
    }

    /// A key may carry a namespace; a slug may not invent one. Keeping these
    /// separate is what lets `surprise/{{slug}}` mean a directory while the
    /// agent's prose can never reach one.
    #[test]
    fn a_template_may_introduce_a_namespace_where_a_slug_may_not() {
        let key = expand_key(
            "surprise/{{slug}}",
            &[("slug", &slug("a/b").unwrap() as &str)],
        )
        .unwrap();
        assert_eq!(key, "surprise/a-b");
        assert_eq!(key.matches('/').count(), 1);
    }

    /// The guard belongs at the mint, not at one call site. `remember` is the
    /// only caller today, but M2 adds a second (`stub = "docs/{{path}}"`,
    /// binding a repo path into an identity) and a guard it has to remember
    /// to call is a guard it will ship without.
    #[test]
    fn a_template_that_escapes_the_store_never_becomes_a_key() {
        let err = expand_key("../../escaped/{{slug}}", &[("slug", "x")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a key"), "got: {err}");

        assert!(
            expand_key("docs/{{path}}", &[("path", "../../etc/passwd")]).is_err(),
            "a bound value must not smuggle in what the template may not spell"
        );
    }

    // ── the clock ───────────────────────────────────────────────────────────

    /// `days / 365` misdates every note by a growing amount, and it is exactly
    /// the class `contributing.md` counted four shipped bugs in: pure,
    /// cheaply-testable code that nobody tested.
    #[test]
    fn todays_date_is_computed_from_the_calendar_not_from_averages() {
        for (days, want) in [
            (0, "1970-01-01"),
            (19_722, "2023-12-31"),
            (19_723, "2024-01-01"),
            (19_782, "2024-02-29"), // a leap day
            (11_016, "2000-02-29"), // a century that IS a leap year
            (365, "1971-01-01"),
        ] {
            assert_eq!(civil(days), want, "day {days}");
        }
    }

    /// The pure table above cannot see an epoch offset or seconds read as
    /// days. Only the system can.
    #[test]
    #[cfg(unix)]
    fn todays_date_agrees_with_the_system_clock() {
        let out = std::process::Command::new("date")
            .args(["-u", "+%F"])
            .output()
            .unwrap();
        let want = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(today(), want);
    }

    // ── schema ──────────────────────────────────────────────────────────────

    fn note_with(kind: Kind, key: &str, body: &str) -> Note {
        Note {
            key: key.to_string(),
            kind,
            source: "session s01, claude".into(),
            recorded: "2026-08-07".into(),
            invalidated_by: None,
            body: body.to_string(),
            layer: Layer::Local,
            path: PathBuf::from(format!("{}.md", key.rsplit('/').next().unwrap())),
        }
    }

    fn surprise_body() -> String {
        "# T\n\n## Expected\na\n\n## Observed\nb\n\n## Evidence\nc\n\n## Answers\n\n- what happens here\n"
            .to_string()
    }

    fn rules(violations: &[Violation]) -> Vec<Rule> {
        violations.iter().map(|v| v.rule).collect()
    }

    /// The weak version is `body.contains("Expected")`, which is satisfied by
    /// the word appearing in a sentence. That is the staleness guard's exact
    /// recorded failure, and this is the test in M1 most likely to be written
    /// that way.
    #[test]
    fn a_note_missing_a_required_section_for_its_type_is_refused() {
        assert!(check(&note_with(Kind::Surprise, "k", &surprise_body())).is_empty());

        for missing in Kind::Surprise.required_sections() {
            let body = surprise_body().replace(&format!("## {missing}"), "## Something");
            let found = check(&note_with(Kind::Surprise, "k", &body));
            assert!(
                rules(&found).contains(&Rule::MissingSection),
                "dropping `{missing}` must be refused"
            );
            assert!(
                found.iter().any(|v| v.detail.contains(missing)),
                "the violation must name `{missing}`: {found:?}"
            );
        }

        // The word, in prose, is not the section.
        let prose = "# T\n\nExpected a mount. Observed an error. Evidence below.\n";
        assert!(
            rules(&check(&note_with(Kind::Surprise, "k", prose))).contains(&Rule::MissingSection),
            "a section is a heading, not a word that appears somewhere"
        );

        // A heading with nothing under it is the obvious evasion.
        let hollow = "# T\n\n## Expected\n\n## Observed\nb\n\n## Evidence\nc\n";
        assert!(
            rules(&check(&note_with(Kind::Surprise, "k", hollow))).contains(&Rule::MissingSection),
            "an empty section is a section that was not filled in"
        );
    }

    /// The structural rule that stands in for a length budget. A budget needs
    /// a number calibrated against a store that does not exist yet; this needs
    /// none.
    #[test]
    fn a_list_section_holds_bullets_and_nothing_else() {
        let clean = format!("{}\n## Related\n\n- [[a]]\n- [[b]]\n", surprise_body());
        assert!(check(&note_with(Kind::Surprise, "k", &clean)).is_empty());

        let prose = format!(
            "{}\n## Related\n\nThis re-narrates the note above at length.\n\n- [[a]]\n",
            surprise_body()
        );
        assert!(
            rules(&check(&note_with(Kind::Surprise, "k", &prose)))
                .contains(&Rule::ProseInListSection),
            "a prose block in a list section is the failure this detects"
        );
    }

    /// Requiring every line to start with `- ` flags ordinary wrapped bullets,
    /// which trains people to ignore the lint — the same failure as a check
    /// that fires on everything.
    #[test]
    fn a_bullet_that_wraps_across_lines_is_still_a_bullet() {
        let wrapped = format!(
            "{}\n## Related\n\n- [[a]] which needed a longer sentence than fits\n  on one line at all\n- [[b]]\n",
            surprise_body()
        );
        assert!(
            check(&note_with(Kind::Surprise, "k", &wrapped)).is_empty(),
            "a continuation line is part of its bullet"
        );
    }

    /// Two sources of truth for identity means `rm <key>` can delete the wrong
    /// file.
    #[test]
    fn a_note_whose_key_disagrees_with_its_filename_is_a_violation() {
        let mut note = note_with(Kind::Surprise, "k", &surprise_body());
        note.path = PathBuf::from("something-else.md");
        let found = check(&note);
        assert!(rules(&found).contains(&Rule::KeyDisagreesWithPath));
        assert!(
            found
                .iter()
                .any(|v| v.detail.contains("something-else") && v.detail.contains('k')),
            "must name both: {found:?}"
        );
    }

    /// §7 gives the two layers different powers, and nothing else enforces
    /// that split. A schema rule that only warned would let a note omh cannot
    /// serve onto disk; a hygiene rule that refused would fail the agent's
    /// write because of somebody else's note.
    #[test]
    fn schema_rules_refuse_and_hygiene_rules_only_warn() {
        let broken = note_with(Kind::Surprise, "k", "# T\n\n## Related\n\nprose\n");
        let found = check(&broken);
        assert!(!found.is_empty(), "this note breaks the schema");
        for v in &found {
            assert_eq!(
                v.rule.severity(),
                Severity::Refused,
                "`{:?}` came from the schema and must refuse",
                v.rule
            );
        }
    }

    /// The staged rules in `detect.rs` hand the agent a fenced ```markdown
    /// block containing these three headings. A schema those satisfy is a
    /// schema satisfied by pasting back the example — `body.contains(…)`'s
    /// failure one fence further out.
    #[test]
    fn a_heading_inside_a_code_fence_does_not_satisfy_the_schema() {
        let fenced =
            "# T\n\n```markdown\n## Expected\nsample\n## Observed\nx\n## Evidence\ny\n```\n\n## Answers\n\n- what happens here\n";
        let note = note_with(Kind::Surprise, "fenced", fenced);

        assert_eq!(
            rules(&check(&note)),
            vec![
                Rule::MissingSection,
                Rule::MissingSection,
                Rule::MissingSection
            ],
            "a note whose every section is quoted has no sections"
        );
    }

    /// A fence closes what it opens, so real sections after one still count.
    /// Without this the fix trades a false pass for a false refusal.
    #[test]
    fn a_section_after_a_closed_fence_still_counts() {
        let body = "# T\n\n## Expected\n```markdown\n## Observed\nquoted\n```\n\n## Observed\nb\n\n## Evidence\nc\n\n## Answers\n\n- what happens here\n";
        assert!(
            check(&note_with(Kind::Surprise, "k", body)).is_empty(),
            "got: {:?}",
            check(&note_with(Kind::Surprise, "k", body))
        );
    }

    /// `## Evidence` exists to hold the command that surprised you, and a
    /// command can contain `[[`. Scanning to the next `]]` swallows the next
    /// real link, so `rm` reports an empty neighbourhood for a note two
    /// others point at — §8's one guarantee, inverted.
    #[test]
    fn an_unclosed_bracket_does_not_swallow_the_next_link() {
        let body = "## Evidence\nthe agent typed `[[` in a sample\n\n## Related\n\n- [[a-real-note]]\n- [[another]]\n";

        let found = links(body);
        assert!(
            found.contains(&"a-real-note".to_string()),
            "the link after the stray bracket vanished: {found:?}"
        );
        assert!(found.contains(&"another".to_string()), "got: {found:?}");
    }

    /// A tilde fence is CommonMark's other fence character, and matching only
    /// backticks left the whole bypass open under a different spelling.
    #[test]
    fn a_tilde_fence_hides_a_heading_just_like_a_backtick_one() {
        let fenced =
            "# T\n\n~~~markdown\n## Expected\nsample\n## Observed\nx\n## Evidence\ny\n~~~\n\n## Answers\n\n- what happens here\n";

        assert_eq!(
            rules(&check(&note_with(Kind::Surprise, "tilde", fenced))),
            vec![
                Rule::MissingSection,
                Rule::MissingSection,
                Rule::MissingSection
            ],
        );
    }

    /// A fourth backtick exists precisely to wrap a fenced example, which is
    /// the shape the staged rules teach. A blind toggle closed the outer
    /// fence on the inner one and read the rest of the block as prose.
    #[test]
    fn a_longer_fence_is_not_closed_by_a_shorter_one_inside_it() {
        let nested = "# T\n\n````markdown\n```\n## Expected\n## Observed\n## Evidence\n```\n````\n\n## Answers\n\n- what happens here\n";

        assert_eq!(
            rules(&check(&note_with(Kind::Surprise, "fourtick", nested))),
            vec![
                Rule::MissingSection,
                Rule::MissingSection,
                Rule::MissingSection
            ],
        );
    }

    /// Fences inside a list item are indented, and CommonMark allows up to
    /// three spaces before one. The heading inside is written flush, so this
    /// separates "the fence was recognised" from "the heading was indented
    /// out of recognition" — which a fence of indented headings cannot.
    #[test]
    fn an_indented_fence_still_opens_a_block() {
        let indented =
            "# T\n\n## Expected\na\n\n  ```markdown\n## Observed\nquoted\n  ```\n\n## Evidence\nc\n\n## Answers\n\n- what happens here\n";
        let found = check(&note_with(Kind::Surprise, "indented", indented));

        assert_eq!(
            rules(&found),
            vec![Rule::MissingSection],
            "`## Observed` sits inside an indented fence: {found:?}"
        );
        assert!(
            found[0].detail.contains("Observed"),
            "got: {}",
            found[0].detail
        );
    }

    /// `## Evidence` holds pasted terminal output, so a truncated paste that
    /// never closes its fence is the likely accident, not the exotic one.
    /// Everything after it stops being a heading and stops being a link — so
    /// the note must say *that*, rather than reporting sections it can see.
    #[test]
    fn an_unclosed_fence_is_refused_by_name_not_as_missing_sections() {
        let truncated =
            "# T\n\n## Expected\na\n\n## Evidence\n```sh\nomh run\n\n## Observed\nb\n\n## Related\n\n- [[somewhere]]\n";
        let found = check(&note_with(Kind::Surprise, "truncated", truncated));

        assert_eq!(
            rules(&found),
            vec![Rule::UnclosedFence],
            "the fence is the problem; the sections are right there"
        );
        assert_eq!(
            Rule::UnclosedFence.severity(),
            Severity::Refused,
            "a note whose links have silently vanished must not be written"
        );
        // The links really are gone — which is why this has to be refused
        // rather than warned about.
        assert!(links(truncated).is_empty());
    }

    /// Per-line scanning stops a stray `[[` reaching the *next* line's link,
    /// but not the rest of its own line: `[[a [[b]]` still closes. A target
    /// holding another opener is a malformed span, not a note called `a [[b`.
    #[test]
    fn a_target_that_swallows_an_opener_is_not_a_link() {
        assert_eq!(links("- [[a [[b]]\n- [[real]]\n"), vec!["real".to_string()]);
    }

    /// A link in a quoted command is not a claim about the graph. Without
    /// this, `lint` warns about a dangling link that is a shell snippet.
    #[test]
    fn a_wiki_link_inside_a_code_fence_is_not_a_link() {
        let body = "## Related\n\n```sh\ngrep '[[not-a-note]]' x\n```\n\n- [[real-note]]\n";
        assert_eq!(links(body), vec!["real-note".to_string()]);
    }

    // ── the store ───────────────────────────────────────────────────────────

    /// The note a key names, for the predicates that take one. Panics rather
    /// than returning an `Option`: a fixture that did not seed what the test
    /// asks about is a broken test, not a case to assert about.
    fn find<'a>(notes: &'a [Note], key: &str) -> &'a Note {
        notes
            .iter()
            .find(|n| n.key == key)
            .unwrap_or_else(|| panic!("no note `{key}` in the fixture"))
    }

    fn seed(paths: &Paths, layer: Layer, key: &str, body: &str) {
        let path = layer.dir(paths).join(format!("{key}.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let note = Note {
            path: path.clone(),
            ..note_with(Kind::Surprise, key, body)
        };
        std::fs::write(&path, render(&note)).unwrap();
    }

    fn keys(notes: &[Note]) -> Vec<String> {
        let mut out: Vec<String> = notes.iter().map(|n| n.key.clone()).collect();
        out.sort();
        out
    }

    #[test]
    fn links_are_read_in_order_and_a_bare_bracket_is_not_a_link() {
        assert_eq!(links("see [[b]] then [[a]]"), vec!["b", "a"]);
        assert_eq!(links("[not a link] and [[  spaced  ]]"), vec!["spaced"]);
        assert!(links("[[]]").is_empty(), "an empty target is not a link");
    }

    /// `config.rs:101-117` is this bug's own docstring. A store that loses a
    /// note answers from a subset and says nothing about it.
    #[test]
    fn a_note_file_that_does_not_parse_is_an_error_not_a_skipped_file() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "good", &surprise_body());
        std::fs::write(Layer::Local.dir(&paths).join("bad.md"), "no frontmatter\n").unwrap();

        let err = load_layer(&paths, Layer::Local).unwrap_err().to_string();
        assert!(
            err.contains("bad"),
            "must name the file it could not read: {err}"
        );
    }

    /// Every memory command would otherwise fail before `init` has run.
    #[test]
    fn an_absent_store_is_empty_not_an_error() {
        let (_d, paths) = fixture();
        assert!(load(&paths).unwrap().is_empty());
    }

    #[test]
    fn a_non_markdown_file_in_the_store_is_ignored() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "good", &surprise_body());
        std::fs::write(Layer::Local.dir(&paths).join(".DS_Store"), "junk").unwrap();
        assert_eq!(keys(&load(&paths).unwrap()), ["good"]);
    }

    /// A key may carry a namespace, so the store has directories in it. A
    /// loader that reads only the top level silently halves the store.
    #[test]
    fn a_note_in_a_namespace_is_still_in_the_store() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "surprise/ebusy", &surprise_body());
        assert_eq!(keys(&load(&paths).unwrap()), ["surprise/ebusy"]);
    }

    /// §4. Shadowing would hide a teammate's note behind yours and the reader
    /// would never learn it existed — so both load, and both carry their own
    /// layer.
    #[test]
    fn the_two_layers_do_not_shadow_each_other() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "deploy", &surprise_body());
        seed(&paths, Layer::Local, "deploy", &surprise_body());

        let all = load(&paths).unwrap();
        assert_eq!(all.len(), 2, "one key in two layers is two notes");
        let mut layers: Vec<Layer> = all.iter().map(|n| n.layer).collect();
        layers.sort();
        assert_eq!(layers, [Layer::Team, Layer::Local]);
    }

    /// A link into the other layer is not dangling — §4 says both retrieve —
    /// and treating it as one buries the real dangling links in noise.
    #[test]
    fn a_dangling_link_is_found_and_names_both_ends() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "target", &surprise_body());
        seed(
            &paths,
            Layer::Local,
            "source",
            &format!(
                "{}\n## Related\n\n- [[target]]\n- [[nope]]\n",
                surprise_body()
            ),
        );

        let found = hygiene(&load(&paths).unwrap());
        let dangling: Vec<&Violation> = found
            .iter()
            .filter(|v| v.rule == Rule::DanglingLink)
            .collect();
        assert_eq!(dangling.len(), 1, "only `nope` dangles: {found:?}");
        assert_eq!(dangling[0].key, "source");
        assert!(dangling[0].detail.contains("nope"), "{:?}", dangling[0]);
    }

    /// The commonest way to get this wrong is to invert it, which flags every
    /// leaf and makes the count worthless as a quality meter.
    #[test]
    fn an_orphan_is_a_note_nothing_links_to_not_a_note_with_no_links() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Local,
            "pointer",
            &format!("{}\n## Related\n\n- [[leaf]]\n", surprise_body()),
        );
        seed(&paths, Layer::Local, "leaf", &surprise_body());

        let orphans: Vec<String> = hygiene(&load(&paths).unwrap())
            .into_iter()
            .filter(|v| v.rule == Rule::Orphan)
            .map(|v| v.key)
            .collect();
        assert_eq!(
            orphans,
            ["pointer"],
            "`leaf` is pointed at; `pointer` is not"
        );
    }

    /// A hygiene rule must never carry the power to refuse.
    #[test]
    fn hygiene_only_ever_warns() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Local,
            "source",
            &format!("{}\n## Related\n\n- [[nope]]\n", surprise_body()),
        );
        let found = hygiene(&load(&paths).unwrap());
        assert!(!found.is_empty());
        for v in &found {
            assert_eq!(v.rule.severity(), Severity::Warning, "{v:?}");
        }
    }

    // ── remember ────────────────────────────────────────────────────────────

    fn observation() -> Remembered {
        Remembered {
            expected: "A bind mount of the token file to persist the login.".into(),
            observed: "The harness rewrites in place. A file mount is one inode.".into(),
            evidence: "`EBUSY` from the mount syscall.".into(),
            answers: vec!["why does my login not persist".into()],
            relates_to: Vec::new(),
            invalidated_by: None,
            source: "session s03, claude".into(),
            recorded: "2026-08-07".into(),
        }
    }

    /// The key `remember` would derive for `input`, without writing anything.
    fn derived_key(paths: &Paths, input: &Remembered) -> String {
        expand_key(
            templates(paths).unwrap().get(&Kind::Surprise).unwrap(),
            &[("slug", &slug_of_observation(&input.observed).unwrap())],
        )
        .unwrap()
    }

    fn files_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let bytes = std::fs::read(&p).unwrap();
                    out.insert(p, bytes);
                }
            }
        }
        out
    }

    /// Invariant 3, at the only call site that exists in M1. Asserted by
    /// walking the whole tree rather than by checking one path: that catches
    /// both a write to the committed layer and a key that escaped through
    /// `../`, and it survives a refactor that moves the store.
    #[test]
    fn remember_writes_nothing_outside_the_local_store() {
        let (dir, paths) = fixture();
        remember(&paths, &observation(), IfExists::Error).unwrap();

        let written = files_under(dir.path());
        assert!(!written.is_empty(), "something must have been written");
        for path in written.keys() {
            assert!(
                path.starts_with(Layer::Local.dir(&paths)),
                "wrote outside the local store: {}",
                path.display()
            );
        }
    }

    /// Every spelling of "not a key", in one table. The escape test below
    /// exercises exactly one of these through `remember`; the other four
    /// disjuncts were each individually removable with the suite green, and
    /// two of them are redundant with each other for absolute keys, so
    /// neither was named by anything.
    #[test]
    fn a_key_is_slash_separated_slugs_and_nothing_else() {
        for bad in [
            "",
            "/etc/passwd",
            "..",
            "../escaped",
            "a/../b",
            "a//b",
            "a/",
            "a\\b",
            ".ssh/authorized_keys",
            "surprise/.",
        ] {
            assert!(
                validate_key(bad).is_err(),
                "`{bad}` must not be usable as a key"
            );
        }
        for good in ["a", "ns/a", "surprise/the-mount-failed", "docs/a/b/c"] {
            assert!(
                validate_key(good).is_ok(),
                "`{good}` is a key and must stay one"
            );
        }
    }

    /// A key can be innocent and still land outside the store, because the
    /// store is a directory tree an agent can write to: one symlinked
    /// namespace and `{key}.md` resolves anywhere. `validate_key` reads the
    /// spelling, so only the resolved path can answer this.
    #[test]
    fn a_symlinked_namespace_cannot_carry_a_write_out_of_the_store() {
        let (dir, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("surprise")).unwrap();

        let err = remember(&paths, &observation(), IfExists::Error).unwrap_err();
        assert!(err.to_string().contains("outside the store"), "got: {err}");
        assert!(
            std::fs::read_dir(&outside).unwrap().next().is_none(),
            "a note landed outside the store through a symlink"
        );
    }

    /// The write guard and the read guard are separate: refusing to write
    /// through a symlink does nothing about one already in the store. A store
    /// that reads through it answers with notes that are not in it, and `rm`
    /// then deletes one of them — outside the directory it claims to own.
    #[test]
    fn the_store_does_not_read_through_a_symlink() {
        let (dir, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&root).unwrap();

        let mut stray = note_with(Kind::Surprise, "elsewhere", &surprise_body());
        stray.path = outside.join("elsewhere.md");
        std::fs::write(&stray.path, render(&stray)).unwrap();

        // Both shapes, because they are caught by different things: a linked
        // directory by reading the entry's own type instead of the target's,
        // a linked file by refusing links outright.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
            std::os::unix::fs::symlink(&stray.path, root.join("linked.md")).unwrap();
        }

        assert!(
            load_layer(&paths, Layer::Local).unwrap().is_empty(),
            "the store answered with a note that is not in it"
        );
    }

    /// A `keys.toml` still present is a loud error naming both paths, never a
    /// silent fallback.
    ///
    /// `templates` treats a missing file as "use the shipped defaults", which
    /// is right for a repo where `init` has not run and catastrophic for a
    /// rename: an edited `keys.toml` would fail to find `memory.toml`, revert
    /// to the shipped templates, and re-key every note written from then on
    /// while every existing key stopped being derivable. Nothing would say so.
    ///
    /// This is the whole of the migration, and it is a check rather than a move
    /// because moving a file in somebody's repo behind their back is the larger
    /// of the two surprises.
    #[test]
    fn a_leftover_keys_toml_is_an_error_naming_both_paths() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(paths.repo.join(".omh/keys.toml"), SHIPPED_KEYS).unwrap();

        let err = templates(&paths).unwrap_err().to_string();
        assert!(err.contains("keys.toml"), "must name the old path: {err}");
        assert!(err.contains("memory.toml"), "and the new one: {err}");

        // And with both present, which is the likelier half-migrated state and
        // the one the check used to miss entirely: `init` writes `memory.toml`
        // beside an untouched `keys.toml`, the new file wins, and the edited
        // templates are ignored without a word. The message says "rather than
        // leaving both" — so "both" has to be a state it detects.
        std::fs::write(paths.repo.join(".omh/memory.toml"), SHIPPED_KEYS).unwrap();
        let err = templates(&paths).unwrap_err().to_string();
        assert!(
            err.contains("keys.toml"),
            "both present is still an error: {err}"
        );
    }

    /// The other half: the new name is read where the old one was.
    #[test]
    fn key_templates_are_read_from_memory_toml() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(
            paths.repo.join(".omh/memory.toml"),
            "[keys]\nsurprise = \"mine/{{slug}}\"\ntopic = \"{{slug}}\"\nstub = \"docs/{{path}}\"\n",
        )
        .unwrap();

        assert_eq!(templates(&paths).unwrap()[&Kind::Surprise], "mine/{{slug}}");
    }

    /// §6 derives keys from a template in the repo, so the template is input
    /// omh does not control — a clone carries one. `remember` creates the
    /// key's parent directories, so a template that leaves the store is an
    /// arbitrary write, and `slug`'s separator rule guards only the variable.
    #[test]
    fn a_key_template_cannot_write_outside_the_store() {
        let (dir, paths) = fixture();
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(
            paths.repo.join(".omh/memory.toml"),
            "[keys]\nsurprise = \"../../escaped/{{slug}}\"\ntopic = \"{{slug}}\"\nstub = \"docs/{{path}}\"\n",
        )
        .unwrap();

        let err = remember(&paths, &observation(), IfExists::Error).unwrap_err();
        assert!(
            err.to_string().contains("not a key"),
            "the refusal must name the problem, got: {err}"
        );
        // `.md` only: the fixture's own `memory.toml` lives outside the store
        // by design, and it is notes that must not escape.
        for path in files_under(dir.path()).keys() {
            assert!(
                path.extension().is_none_or(|e| e != "md")
                    || path.starts_with(Layer::Local.dir(&paths)),
                "wrote outside the local store: {}",
                path.display()
            );
        }
    }

    /// Invariant 5 is about keys, not filenames. `path.exists()` enforces it
    /// only while every note sits at exactly its key — which nothing checks,
    /// because `KeyDisagreesWithPath` compares the leaf alone. The result is
    /// two notes under one key, which `rm` then cannot separate.
    #[test]
    fn a_key_already_in_the_layer_is_a_conflict_wherever_it_is_stored() {
        let (_d, paths) = fixture();
        let taken = derived_key(&paths, &observation());

        // The same key, stored somewhere else — what a hand-written note
        // produces, which in M1 is the only writer the agent has.
        let elsewhere = Layer::Local.dir(&paths).join("hand-written.md");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        let mut note = note_with(Kind::Surprise, &taken, &surprise_body());
        note.path = elsewhere.clone();
        std::fs::write(&elsewhere, render(&note)).unwrap();

        let err = remember(&paths, &observation(), IfExists::Error).unwrap_err();
        assert!(
            err.to_string().contains("already recorded"),
            "a taken key is a conflict wherever it lives, got: {err}"
        );
    }

    /// Skipping a note omh cannot parse answers "is this key free?" with
    /// "yes" to a question whose true answer may be "no" — and the
    /// consequence is a write, not a read. The result was two notes under one
    /// key, silently, through the sanctioned path.
    ///
    /// So the write is refused. A deliberate exception to §7's "a store-wide
    /// problem must never refuse somebody else's write": this is not an
    /// unrelated problem elsewhere in the store, it is omh being unable to
    /// verify the invariant *this* write depends on. A refused write is
    /// recoverable; a duplicated key is not.
    #[test]
    fn a_note_omh_cannot_read_stops_the_write_rather_than_risking_a_duplicate() {
        let (dir, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("unreadable.md"), "this has no frontmatter\n").unwrap();

        let err = remember(&paths, &observation(), IfExists::Error).unwrap_err();
        assert!(
            err.to_string().contains("unreadable.md"),
            "the refusal must name the file standing in the way: {err}"
        );

        let notes = files_under(dir.path())
            .into_keys()
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .count();
        assert_eq!(
            notes, 1,
            "nothing new may be written while the store is unverifiable"
        );
    }

    /// A mislocated note is exactly the case `--if-exists override` was
    /// pointed at, and it was the one case it could not do: writing to the
    /// old file made the key disagree with the new filename, so `check`
    /// refused the write it had just been told to force.
    ///
    /// Overriding restores the invariant rather than preserving the mistake:
    /// one note, at the key's own path.
    #[test]
    fn override_replaces_a_mislocated_note_and_leaves_one_behind() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        let key = derived_key(&paths, &observation());

        let stale = root.join("hand-written.md");
        std::fs::create_dir_all(root.join("surprise")).unwrap();
        let mut note = note_with(Kind::Surprise, &key, &surprise_body());
        note.path = stale.clone();
        std::fs::write(&stale, render(&note)).unwrap();

        let wrote = remember(&paths, &observation(), IfExists::Override).unwrap();

        assert_eq!(
            wrote,
            Wrote::Replaced(root.join(format!("{key}.md"))),
            "a write that destroyed a note must not report as a creation"
        );
        assert!(!stale.exists(), "the note it replaced is still there");
        assert_eq!(
            lint(&paths)
                .unwrap()
                .iter()
                .filter(|v| v.rule == Rule::DuplicateKey)
                .count(),
            0,
            "override must leave one note under the key, not two"
        );
    }

    /// `skip` is §6's mode for idempotent ingest, so it has to recognise the
    /// key as taken wherever the note holding it sits — otherwise a repeated
    /// ingest writes a second copy of what it was told to leave alone.
    #[test]
    fn skip_and_suffix_see_a_key_held_by_a_mislocated_note() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        let key = derived_key(&paths, &observation());
        let mut note = note_with(Kind::Surprise, &key, &surprise_body());
        note.path = root.join("hand-written.md");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&note.path, render(&note)).unwrap();

        assert_eq!(
            remember(&paths, &observation(), IfExists::Skip).unwrap(),
            Wrote::Skipped(key.clone()),
            "the key is taken, so there is nothing to add"
        );

        let Wrote::Created(path) = remember(&paths, &observation(), IfExists::Suffix).unwrap()
        else {
            panic!("suffix creates a new note");
        };
        assert!(
            path.ends_with(format!("{key}-2.md")),
            "suffix must step over the held key: {}",
            path.display()
        );
    }

    /// Every field the caller supplies has to arrive under its own heading.
    /// Asserting only that a file appeared lets `expected` and `evidence`
    /// swap places, or `invalidated_by` be dropped, with the suite green.
    #[test]
    fn every_input_to_remember_lands_in_its_own_place_on_disk() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.relates_to = vec!["credentials-are-a-named-volume".into()];
        input.invalidated_by = Some("image:4f2a1c3b5d7e9f0a2b4c6d8e0f1a3b5c7d9e0f1a".into());

        let Wrote::Created(path) = remember(&paths, &input, IfExists::Error).unwrap() else {
            panic!("a fresh key must be created");
        };
        let note = parse(
            &std::fs::read_to_string(&path).unwrap(),
            Layer::Local,
            &path,
        )
        .unwrap();

        assert_eq!(note.kind, Kind::Surprise);
        assert_eq!(note.source, input.source);
        assert_eq!(note.recorded, input.recorded);
        assert_eq!(note.invalidated_by, input.invalidated_by);

        let body = sections(&note.body);
        for (heading, supplied) in [
            ("Expected", &input.expected),
            ("Observed", &input.observed),
            ("Evidence", &input.evidence),
        ] {
            assert_eq!(
                body[heading].join("\n").trim(),
                supplied.trim(),
                "`## {heading}` must hold what was passed as {heading}"
            );
        }
        assert_eq!(links(&note.body), input.relates_to);
    }

    /// The key names what happened, not what was guessed. Deriving it from
    /// `expected` still round-trips, still lints clean, and is wrong forever.
    #[test]
    fn the_key_is_derived_from_what_was_observed() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.expected = "Zebras would persist the login.".into();
        input.observed = "Walruses returned EBUSY.".into();

        let Wrote::Created(path) = remember(&paths, &input, IfExists::Error).unwrap() else {
            panic!("a fresh key must be created");
        };
        let shown = path.to_string_lossy().to_string();
        assert!(shown.contains("walruses"), "got: {shown}");
        assert!(!shown.contains("zebras"), "keyed off the guess: {shown}");
    }

    /// §9.1 makes provenance a parameter that cannot be omitted rather than a
    /// rule that can be violated. Defaulting a blank one to `agent` turns it
    /// straight back into a rule.
    #[test]
    fn provenance_is_not_the_agents_to_supply() {
        let (_d, paths) = fixture();
        let mut blank = observation();
        blank.source = "   ".into();
        assert!(
            remember(&paths, &blank, IfExists::Error).is_err(),
            "a note with no provenance cannot be judged, so it is not written"
        );

        let note = &load(&paths).unwrap();
        assert!(note.is_empty(), "and nothing was written anyway");
    }

    /// The filter §9.1 says runs for free: an agent with nothing to put in
    /// `expected` has learned nothing worth recording.
    #[test]
    fn an_observation_with_nothing_expected_is_refused() {
        let (_d, paths) = fixture();
        for blank in ["expected", "observed", "evidence"] {
            let mut input = observation();
            match blank {
                "expected" => input.expected = "  ".into(),
                "observed" => input.observed = "  ".into(),
                _ => input.evidence = "  ".into(),
            }
            let err = remember(&paths, &input, IfExists::Error);
            assert!(err.is_err(), "a blank `{blank}` is not an observation");
            assert!(err.unwrap_err().to_string().contains(blank));
        }
    }

    /// §6: writing an existing key is a conflict whose message says *update
    /// instead*. The alternative is a truncating write that loses the earlier
    /// observation with no trace.
    #[test]
    fn writing_an_existing_key_is_an_error_that_says_update_instead() {
        let (_d, paths) = fixture();
        let first = remember(&paths, &observation(), IfExists::Error).unwrap();
        let Wrote::Created(path) = first else {
            panic!("expected a write")
        };
        let before = std::fs::read(&path).unwrap();

        let err = remember(&paths, &observation(), IfExists::Error)
            .unwrap_err()
            .to_string();
        assert!(err.contains("update"), "must say what to do instead: {err}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "and change nothing");
    }

    /// Validate before writing, or a refused note lands anyway and `lint`
    /// reports a violation its writer believes it never created.
    #[test]
    fn a_refused_write_leaves_nothing_on_disk() {
        let (dir, paths) = fixture();
        let mut bad = observation();
        bad.recorded = "2026-13-45".into();

        assert!(remember(&paths, &bad, IfExists::Error).is_err());
        assert!(
            files_under(dir.path()).is_empty(),
            "a refused write must not leave a file behind"
        );
    }

    /// §6. Making `Skip` the default for convenience makes every genuine
    /// conflict disappear silently.
    #[test]
    fn skip_if_exists_is_an_explicit_mode_not_a_fallback() {
        let (_d, paths) = fixture();
        assert!(
            matches!(IfExists::default(), IfExists::Error),
            "the default refuses"
        );

        let Wrote::Created(path) = remember(&paths, &observation(), IfExists::Error).unwrap()
        else {
            panic!()
        };
        let before = std::fs::read(&path).unwrap();

        let again = remember(&paths, &observation(), IfExists::Skip).unwrap();
        assert!(matches!(again, Wrote::Skipped(_)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn suffix_never_reuses_a_suffix_already_taken() {
        let (_d, paths) = fixture();
        let mut written = Vec::new();
        for _ in 0..3 {
            match remember(&paths, &observation(), IfExists::Suffix).unwrap() {
                Wrote::Created(p) => written.push(p),
                other => panic!("{other:?}"),
            }
        }
        written.sort();
        written.dedup();
        assert_eq!(written.len(), 3, "each write is its own file");
        assert_eq!(load(&paths).unwrap().len(), 3);
    }

    /// §4, at the write path. Keying the collision check on the bare key
    /// across layers means an agent's *contradicting* observation is refused
    /// because a teammate documented the topic — the inverse of the shadowing
    /// §4 forbids.
    #[test]
    fn a_teammates_note_on_the_same_topic_does_not_block_the_write() {
        let (_d, paths) = fixture();
        let key = expand_key(
            "surprise/{{slug}}",
            &[(
                "slug",
                &slug_of_observation(&observation().observed).unwrap(),
            )],
        )
        .unwrap();
        seed(&paths, Layer::Team, &key, &surprise_body());

        assert!(
            remember(&paths, &observation(), IfExists::Error).is_ok(),
            "the committed layer is a different note, not a collision"
        );
        assert_eq!(load(&paths).unwrap().len(), 2);
    }

    /// §7 gives the two guards different powers. `remember` calling `lint()`
    /// would fail the agent's write, unattended, because of somebody else's
    /// note — with no way for it to fix the problem.
    #[test]
    fn a_hygiene_violation_never_refuses_a_write() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Local,
            "broken",
            &format!("{}\n## Related\n\n- [[nowhere]]\n", surprise_body()),
        );
        assert!(
            !hygiene(&load(&paths).unwrap()).is_empty(),
            "the store is dirty"
        );
        assert!(
            remember(&paths, &observation(), IfExists::Error).is_ok(),
            "a clean note is written into a dirty store"
        );
    }

    /// §6: *both keys being unique is not the same as the identity being
    /// unique.* Preserving input order makes an idempotent re-record churn the
    /// file.
    #[test]
    fn a_multi_valued_key_component_is_pinned_to_one_order() {
        let (_d, paths) = fixture();
        let mut one = observation();
        one.relates_to = vec!["nate".into(), "joanna".into(), "nate".into()];
        let Wrote::Created(a) = remember(&paths, &one, IfExists::Error).unwrap() else {
            panic!()
        };
        let first = std::fs::read_to_string(&a).unwrap();

        let (_d2, other) = fixture();
        let mut two = observation();
        two.relates_to = vec!["joanna".into(), "nate".into()];
        let Wrote::Created(b) = remember(&other, &two, IfExists::Error).unwrap() else {
            panic!()
        };

        assert_eq!(
            first,
            std::fs::read_to_string(&b).unwrap(),
            "the same neighbours in a different order are the same note"
        );
    }

    // ── rm ──────────────────────────────────────────────────────────────────

    /// `--at` was consulted only when the key matched several notes, so
    /// naming a file that does not hold the key deleted a different one and
    /// reported success. Someone types `--at` precisely to be careful about
    /// which file dies; deletion is irreversible and the local store is
    /// outside the checkout, so git will not give it back.
    #[test]
    fn an_at_that_names_nothing_never_falls_through_to_another_note() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "solo", &surprise_body());

        let err = remove(&paths, None, "solo", Some("some-other-file.md"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("some-other-file.md"), "got: {err}");
        assert!(
            Layer::Local.dir(&paths).join("solo.md").exists(),
            "a note the caller did not name was removed"
        );
    }

    /// Both bail arms of `disambiguate` are one `Ok(first)` away from being
    /// silent data loss, so both are pinned: a `--at` that matches nothing,
    /// and one that matches more than it can separate.
    #[test]
    fn an_at_that_cannot_pick_one_note_removes_none_of_them() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        std::fs::create_dir_all(root.join("ns")).unwrap();
        for at in ["dup.md", "ns/dup.md"] {
            let mut note = note_with(Kind::Surprise, "dup", &surprise_body());
            note.path = root.join(at);
            std::fs::write(&note.path, render(&note)).unwrap();
        }

        let missed = remove(&paths, Some(Layer::Local), "dup", Some("absent.md"), false)
            .unwrap_err()
            .to_string();
        assert!(missed.contains("absent.md"), "got: {missed}");

        // `dup.md` is a component-suffix of both paths, so it names neither.
        let ambiguous = remove(&paths, Some(Layer::Local), "dup", Some("dup.md"), false)
            .unwrap_err()
            .to_string();
        assert!(
            ambiguous.contains("2"),
            "an ambiguous --at must say so: {ambiguous}"
        );

        assert_eq!(
            files_under(&root).len(),
            2,
            "neither refusal may remove anything"
        );
    }

    /// When the same relative path exists in both layers, `--at` cannot
    /// separate them however much of the path is given — the answer is
    /// `--layer`, and the message has to say so rather than asking for more
    /// of a path that is already identical.
    #[test]
    fn an_at_that_spans_layers_points_at_the_layer_flag() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "shared", &surprise_body());
        seed(&paths, Layer::Team, "shared", &surprise_body());

        let err = remove(&paths, None, "shared", Some("shared.md"), false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--layer"),
            "the only thing that separates these is the layer: {err}"
        );
    }

    /// `remove` assumed duplicates could only be cross-layer, so two notes
    /// under one key in one layer produced "`k` is in local and local" — and
    /// `--layer local` produced it again. There was no argument that reached
    /// either note, in a store deliberately kept outside the checkout.
    #[test]
    fn a_key_duplicated_inside_one_layer_is_still_removable() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        std::fs::create_dir_all(root.join("ns")).unwrap();
        for at in ["dup.md", "ns/dup.md"] {
            let mut note = note_with(Kind::Surprise, "dup", &surprise_body());
            note.path = root.join(at);
            std::fs::write(&note.path, render(&note)).unwrap();
        }

        let err = remove(&paths, Some(Layer::Local), "dup", None, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("dup.md") && err.contains("ns/dup.md"),
            "the error must name the files, since the layer cannot separate them: {err}"
        );
        assert!(
            !err.contains("local and local"),
            "duplicates in one layer are not a layer question: {err}"
        );

        // And the store must be repairable through omh itself.
        let removed = remove(&paths, Some(Layer::Local), "dup", Some("ns/dup.md"), false)
            .expect("a duplicated key must still be removable");
        assert!(
            removed.path.ends_with("ns/dup.md"),
            "got: {}",
            removed.path.display()
        );
        assert!(
            root.join("dup.md").exists(),
            "rm must take exactly one note"
        );
    }

    /// Invariant 7. Deleting a neighbour is one failure; *rewriting* one to
    /// strip the now-dangling link is the other, and only byte identity can
    /// see the second.
    #[test]
    fn rm_removes_one_note_and_leaves_every_neighbour_byte_identical() {
        let (dir, paths) = fixture();
        let pointing = format!("{}\n## Related\n\n- [[b]]\n", surprise_body());
        seed(&paths, Layer::Local, "a", &pointing);
        seed(&paths, Layer::Team, "c", &pointing);
        seed(&paths, Layer::Local, "b", &surprise_body());

        let before: BTreeMap<PathBuf, Vec<u8>> = files_under(dir.path())
            .into_iter()
            .filter(|(p, _)| !p.ends_with("b.md"))
            .collect();

        remove(&paths, None, "b", None, false).unwrap();

        assert_eq!(
            files_under(dir.path()),
            before,
            "every other note must be untouched, byte for byte"
        );
    }

    #[test]
    fn rm_reports_what_linked_to_the_note_it_removed() {
        let (_d, paths) = fixture();
        let pointing = format!("{}\n## Related\n\n- [[b]]\n", surprise_body());
        seed(&paths, Layer::Local, "a", &pointing);
        seed(&paths, Layer::Team, "c", &pointing);
        seed(&paths, Layer::Local, "b", &surprise_body());

        let removed = remove(&paths, None, "b", None, false).unwrap();
        assert_eq!(
            removed.inbound,
            ["a", "c"],
            "inbound links cross layers; scoping to one hides half of them"
        );
    }

    /// Picking `local` "because that is where writes go" leaves a pulled team
    /// note silently alive while the user believes it is gone.
    #[test]
    fn removing_a_key_present_in_both_layers_names_both_rather_than_picking_one() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "deploy", &surprise_body());
        seed(&paths, Layer::Local, "deploy", &surprise_body());

        let err = remove(&paths, None, "deploy", None, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("team") && err.contains("local"), "got: {err}");
        assert_eq!(load(&paths).unwrap().len(), 2, "and removed neither");

        remove(&paths, Some(Layer::Local), "deploy", None, false).unwrap();
        let left = load(&paths).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].layer, Layer::Team);
    }

    /// Removing a committed note deletes it here and nowhere else until the
    /// deletion is committed. `rm` has to report which layer it came out of,
    /// or a shared note reads as gone for everybody.
    #[test]
    fn rm_reports_which_layer_the_note_came_out_of() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "shared", &surprise_body());
        seed(&paths, Layer::Local, "mine", &surprise_body());

        assert!(remove(&paths, None, "shared", None, false)
            .unwrap()
            .layer
            .is_committed());
        assert!(!remove(&paths, None, "mine", None, false)
            .unwrap()
            .layer
            .is_committed());
    }

    #[test]
    fn rm_on_an_absent_key_says_so_rather_than_succeeding_quietly() {
        let (_d, paths) = fixture();
        let err = remove(&paths, None, "never-existed", None, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("never-existed"), "got: {err}");
    }

    // ── listing and the review moment ───────────────────────────────────────

    /// Every line carries its own date and its own layer. Two notes with
    /// *different* dates, because a single date is satisfied by a constant —
    /// which is how a date guard in this repo passed while checking nothing.
    ///
    /// This is `omh memory`'s render, not the retrieval proxy's. Invariant 1
    /// belongs to `recall` and this test does not discharge it.
    #[test]
    fn omh_memory_lists_every_note_with_its_date_and_its_layer() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "older", &surprise_body());
        seed(&paths, Layer::Local, "newer", &surprise_body());
        let mut notes = load(&paths).unwrap();
        for note in &mut notes {
            note.recorded = if note.key == "older" {
                "2026-06-12".into()
            } else {
                "2026-08-07".into()
            };
        }

        let out = render_list(&notes);
        for (key, layer, date) in [
            ("older", "team", "2026-06-12"),
            ("newer", "local", "2026-08-07"),
        ] {
            let line = out
                .lines()
                .find(|l| l.contains(key))
                .unwrap_or_else(|| panic!("`{key}` is missing from:\n{out}"));
            assert!(line.contains(layer), "`{key}` lost its layer: {line}");
            assert!(line.contains(date), "`{key}` lost its date: {line}");
        }
    }

    /// `source.contains(id)` makes `s1` match `s10`, so removing one session
    /// claims notes belonging to another that is still running.
    #[test]
    fn the_session_removal_nudge_counts_only_that_sessions_notes() {
        let mut notes = Vec::new();
        for session in ["s1", "s10", "s1x"] {
            let mut note = note_with(Kind::Surprise, session, &surprise_body());
            note.source = format!("session {session}, claude");
            notes.push(note);
        }
        assert_eq!(from_session(&notes, "s1").len(), 1);
        assert_eq!(from_session(&notes, "s10").len(), 1);
        assert!(from_session(&notes, "s2").is_empty());
    }

    /// Printing `0 notes recorded` on every removal is noise, and it trains
    /// people to stop reading the one report §12 relies on.
    #[test]
    fn the_nudge_is_silent_when_the_session_recorded_nothing() {
        assert!(session_nudge(&[], "s1").is_none());

        let mut note = note_with(Kind::Surprise, "k", &surprise_body());
        note.source = "session s1, claude".into();
        let line = session_nudge(&[note], "s1").expect("one note is worth a line");
        assert!(line.contains('1') && line.contains("omh memory"), "{line}");
    }

    #[test]
    fn the_shipped_key_templates_cover_every_note_type() {
        let shipped = parse_templates(SHIPPED_KEYS).unwrap();
        for kind in Kind::ALL {
            let template = shipped
                .get(&kind)
                .unwrap_or_else(|| panic!("no key template for `{kind}`"));
            let key = expand_key(template, &[("slug", "x"), ("path", "docs/x")]).unwrap();
            assert!(!key.is_empty() && !key.contains("{{"), "`{kind}` → {key}");
        }
    }

    // ── resolution across layers ────────────────────────────────────────────

    /// Identity is `(layer, key)`. A `BTreeMap<String, Note>` is the natural
    /// first implementation and it collapses `team/deploy` into
    /// `local/deploy`, which silently loses whichever a teammate wrote — the
    /// exact shadowing §4 forbids.
    #[test]
    fn the_layer_is_part_of_a_notes_identity() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "deploy", &surprise_body());
        seed(&paths, Layer::Local, "deploy", &surprise_body());

        let notes = load(&paths).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(resolve(&notes, "deploy", Layer::Local).len(), 2);
    }

    /// From the gitignored layer a key resolves into whatever holds it: §4 says
    /// both retrieve, so a local note may point at a committed one.
    #[test]
    fn a_local_link_resolves_into_either_layer() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "shared", &surprise_body());
        let notes = load(&paths).unwrap();
        assert_eq!(resolve(&notes, "shared", Layer::Local), vec![Layer::Team]);
    }

    /// **Invariant 2's whole mechanism.** A committed note is read in a clone
    /// where no local layer exists, so a link out of `team` may only reach
    /// `team`. Expressed as resolution rather than as a separate rule, because
    /// a rule can be forgotten at a second call site and a return type cannot.
    ///
    /// This is also the "fallback" somebody adds to silence the test above:
    /// one layer-blind `resolve` breaks the invariant everywhere at once.
    #[test]
    fn a_committed_note_never_resolves_a_link_into_the_gitignored_layer() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "mine", &surprise_body());
        let notes = load(&paths).unwrap();

        assert_eq!(resolve(&notes, "mine", Layer::Local), vec![Layer::Local]);
        assert!(
            resolve(&notes, "mine", Layer::Team).is_empty(),
            "a teammate cloning this repo has no local layer to reach"
        );
    }

    /// The one predicate the lint and `promote` both call. Two implementations
    /// of "which links would dangle in a clone" is the shape that let two
    /// subsystems tell two stories about one file in `config.rs`.
    #[test]
    fn uncommitted_links_names_what_a_clone_would_lose() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "committed", &surprise_body());
        seed(&paths, Layer::Local, "private", &surprise_body());
        seed(
            &paths,
            Layer::Local,
            "candidate",
            &format!(
                "{}\n## Related\n\n- [[committed]]\n- [[private]]\n",
                surprise_body()
            ),
        );
        let notes = load(&paths).unwrap();

        assert_eq!(
            uncommitted_links(&notes, find(&notes, "candidate"), &[]),
            vec!["private".to_string()],
            "only the link a clone could not follow"
        );
    }

    /// Two notes that point at each other are unpromotable in either order
    /// unless the check knows what else is being promoted alongside — and the
    /// error would read like a bug rather than a rule.
    #[test]
    fn a_pair_that_link_to_each_other_are_promotable_together() {
        let (_d, paths) = fixture();
        for (key, other) in [("a", "b"), ("b", "a")] {
            seed(
                &paths,
                Layer::Local,
                key,
                &format!("{}\n## Related\n\n- [[{other}]]\n", surprise_body()),
            );
        }
        let notes = load(&paths).unwrap();

        assert_eq!(
            uncommitted_links(&notes, find(&notes, "a"), &[]),
            vec!["b".to_string()]
        );
        assert!(
            uncommitted_links(&notes, find(&notes, "a"), &["b".to_string()]).is_empty(),
            "promoted together, neither dangles"
        );
    }

    /// **Invariant 2, the headline.** A lint that asks "does this key exist
    /// *somewhere*" is green on precisely the store that breaks in a fresh
    /// clone — the target is right there in the local layer, which the
    /// teammate will never receive.
    #[test]
    fn every_committed_note_links_only_to_committed_notes() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &surprise_body());
        seed(
            &paths,
            Layer::Team,
            "shared",
            &format!("{}\n## Related\n\n- [[private]]\n", surprise_body()),
        );

        let found = lint(&paths).unwrap();
        let crossing: Vec<&Violation> = found
            .iter()
            .filter(|v| v.rule == Rule::CrossLayerLink)
            .collect();
        assert_eq!(crossing.len(), 1, "got: {found:?}");
        assert_eq!(crossing[0].key, "shared");
        assert!(crossing[0].detail.contains("private"), "{:?}", crossing[0]);
    }

    /// **The store the lint exists for is the one it was blind to.** Identity
    /// is `(layer, key)` and `DuplicateKey` is a *warning*, so two committed
    /// files may legitimately claim one key. Looking a note up by key alone
    /// then judged every one of them by the first match's body: the offending
    /// file's links were never read, and the clean file was reported twice.
    ///
    /// Order-independent on purpose. The defect was invisible while
    /// `Layer::ALL` happened to list `Team` first, and a test that only holds
    /// for one ordering is a test that stops holding when somebody sorts.
    #[test]
    fn a_duplicate_key_never_hides_a_committed_notes_cross_layer_link() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &surprise_body());
        // Clean, and first by path — so a key-only lookup finds this one.
        seed(&paths, Layer::Team, "dup", &surprise_body());
        // The offender, claiming the same key from a different file.
        let other = Layer::Team.dir(&paths).join("ns/dup.md");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(
            &other,
            render(&Note {
                path: other.clone(),
                ..note_with(
                    Kind::Surprise,
                    "dup",
                    &format!("{}\n## Related\n\n- [[private]]\n", surprise_body()),
                )
            }),
        )
        .unwrap();

        let found = lint(&paths).unwrap();
        let crossing: Vec<&Violation> = found
            .iter()
            .filter(|v| v.rule == Rule::CrossLayerLink)
            .collect();
        assert_eq!(
            crossing.len(),
            1,
            "the file that links into the gitignored layer, exactly once: {found:?}"
        );
        assert!(crossing[0].detail.contains("private"), "{:?}", crossing[0]);
    }

    /// Without this the test above passes on a lint that complains about every
    /// committed note — which this repo has shipped before, as a check that
    /// could have been `=> true`.
    #[test]
    fn a_committed_note_pointing_at_a_committed_note_is_silent() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "target", &surprise_body());
        seed(
            &paths,
            Layer::Team,
            "source",
            &format!("{}\n## Related\n\n- [[target]]\n", surprise_body()),
        );
        assert!(
            !lint(&paths)
                .unwrap()
                .iter()
                .any(|v| v.rule == Rule::CrossLayerLink),
            "a committed link to a committed note is exactly what is wanted"
        );
    }

    /// Applied in both directions it would make the gitignored layer unusable:
    /// a local note is *supposed* to reach a committed one.
    #[test]
    fn a_local_note_may_point_wherever_it_likes() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "shared", &surprise_body());
        seed(&paths, Layer::Local, "other", &surprise_body());
        seed(
            &paths,
            Layer::Local,
            "mine",
            &format!(
                "{}\n## Related\n\n- [[shared]]\n- [[other]]\n",
                surprise_body()
            ),
        );
        assert!(!lint(&paths)
            .unwrap()
            .iter()
            .any(|v| v.rule == Rule::CrossLayerLink));
    }

    /// It warns rather than refuses. The note at fault is committed and the
    /// agent writing right now cannot fix it, so refusing would fail an
    /// unattended write over somebody else's mistake — §7's whole split.
    #[test]
    fn a_cross_layer_link_warns_rather_than_refusing() {
        assert_eq!(Rule::CrossLayerLink.severity(), Severity::Warning);
    }

    // ── lint ────────────────────────────────────────────────────────────────

    /// One note omh cannot parse aborted `lint` before it printed anything,
    /// so the command that exists to tell you what is wrong with the store
    /// went silent on precisely the store that needed it — and `remember`
    /// refuses while it is in that state, which made it unrecoverable
    /// through omh. Reporting it is what lets you fix it.
    #[test]
    fn lint_reports_a_note_it_cannot_read_instead_of_giving_up() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("broken.md"), "no frontmatter here\n").unwrap();
        // Judged and *found wanting*, so "the rest of the store is still
        // judged" rests on a schema finding rather than on orphanhood — which
        // is silent in a store this small, and which is not what this test is
        // about anyway.
        seed(&paths, Layer::Local, "fine", "# F\n\n## Expected\na\n");

        let found = lint(&paths).unwrap();
        let unreadable: Vec<_> = found
            .iter()
            .filter(|v| v.rule == Rule::Unreadable)
            .collect();

        assert_eq!(unreadable.len(), 1, "got: {found:?}");
        assert!(
            unreadable[0].detail.contains("broken.md"),
            "the report must name the file: {}",
            unreadable[0].detail
        );
        assert_eq!(
            Rule::Unreadable.severity(),
            Severity::Refused,
            "a note the store cannot read is not a style warning"
        );
        // And the rest of the store is still judged, which is the point.
        assert!(
            found.iter().any(|v| v.key == "fine"),
            "one bad file must not hide every other violation: {found:?}"
        );
    }

    /// `remember` now refuses to create one, but a store can already hold
    /// two notes under one key — hand-written notes are the only writer M1
    /// gives the agent. Nothing could report that state: `KeyDisagreesWithPath`
    /// compares only the leaf, so two files whose leaves agree were invisible
    /// and `lint` exited 0 on a violated §6.
    ///
    /// A warning, not a refusal, because it is a property of the store rather
    /// than of the note being written — §7's split, and the reason `remember`
    /// is where the refusal lives.
    #[test]
    fn two_notes_under_one_key_in_one_layer_are_reported() {
        let (_d, paths) = fixture();
        let root = Layer::Local.dir(&paths);
        std::fs::create_dir_all(root.join("ns")).unwrap();
        for at in ["dup.md", "ns/dup.md"] {
            let mut note = note_with(Kind::Surprise, "dup", &surprise_body());
            note.path = root.join(at);
            std::fs::write(&note.path, render(&note)).unwrap();
        }

        let found = lint(&paths).unwrap();
        let dupes: Vec<_> = found
            .iter()
            .filter(|v| v.rule == Rule::DuplicateKey)
            .collect();

        assert_eq!(dupes.len(), 1, "one key, one report: {found:?}");
        assert!(
            dupes[0].detail.contains("dup.md") && dupes[0].detail.contains("ns/dup.md"),
            "the report must name both files: {}",
            dupes[0].detail
        );
        assert_eq!(Rule::DuplicateKey.severity(), Severity::Warning);
    }

    /// §4 makes `team/deploy` and `local/deploy` two notes on purpose — both
    /// retrieve, and neither shadows the other. A duplicate check that
    /// ignored the layer would report the design as a defect.
    #[test]
    fn one_key_in_both_layers_is_not_a_duplicate() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "deploy", &surprise_body());
        seed(&paths, Layer::Team, "deploy", &surprise_body());

        assert!(
            !lint(&paths)
                .unwrap()
                .iter()
                .any(|v| v.rule == Rule::DuplicateKey),
            "a key in both layers is a disagreement, not a duplicate"
        );
    }

    /// What makes `omh memory lint` fail. Only refusals may: `Orphan` fires
    /// on every note nothing links to — which is every note `remember` writes
    /// without `--relates-to` — so gating on warnings gates on store shape
    /// and trains people to ignore the command.
    #[test]
    fn only_refusals_decide_whether_lint_fails() {
        let warning = Violation {
            key: "k".into(),
            layer: Layer::Local,
            rule: Rule::Orphan,
            detail: String::new(),
        };
        let refusal = Violation {
            rule: Rule::MissingSection,
            ..warning.clone()
        };

        assert_eq!(
            refused(std::slice::from_ref(&warning)),
            0,
            "a store full of warnings is still a passing store"
        );
        assert_eq!(refused(&[warning, refusal]), 1);
    }

    /// The two guards answer different questions, and a `lint` wired to only
    /// one of them reports a clean store while half the checks never ran.
    #[test]
    fn lint_reports_both_the_schema_and_the_links() {
        let (_d, paths) = fixture();
        seed(
            &paths,
            Layer::Local,
            "broken",
            "# T\n\n## Expected\na\n\n## Related\n\nprose, not bullets\n",
        );

        let found = lint(&paths).unwrap();
        let seen = tally(&found);
        assert!(
            seen.contains_key(&Rule::MissingSection),
            "the schema half must run: {found:?}"
        );
        assert!(
            seen.contains_key(&Rule::ProseInListSection),
            "the structural half must run: {found:?}"
        );
        assert!(
            seen.contains_key(&Rule::Orphan),
            "the hygiene half must run: {found:?}"
        );
    }

    /// §7's store-quality meter, tested rather than asserted in prose. A rule
    /// that cannot separate a store you believe in from one you don't is a
    /// rule that does not ship.
    #[test]
    fn the_violation_count_separates_a_good_store_from_a_bad_one() {
        let (_d, good) = fixture();
        seed(
            &good,
            Layer::Local,
            "a",
            &format!("{}\n## Related\n\n- [[b]]\n", surprise_body()),
        );
        seed(
            &good,
            Layer::Local,
            "b",
            &format!("{}\n## Related\n\n- [[a]]\n", surprise_body()),
        );

        let (_d2, bad) = fixture();
        seed(
            &bad,
            Layer::Local,
            "a",
            "# T\n\n## Related\n\nre-narration\n",
        );
        seed(&bad, Layer::Local, "b", "# T\n\n## Related\n\n- [[gone]]\n");

        let clean = lint(&good).unwrap();
        assert!(
            clean.is_empty(),
            "a store worth keeping lints clean: {clean:?}"
        );
        assert!(lint(&bad).unwrap().len() > clean.len());
    }

    // ── the staged rules ────────────────────────────────────────────────────

    /// M1 has no MCP surface, so the only thing that can teach the agent the
    /// note format is the rules file. A template that does not survive the
    /// parser teaches a shape the store then refuses — and the agent has no
    /// way to discover that, because nothing tells it the write failed.
    ///
    /// Filled in with plausible values and pushed through the real parser and
    /// the real schema, which is the only check that cannot drift from them.
    #[test]
    fn the_note_template_in_the_staged_rules_actually_parses() {
        let rules = shipped_rules();
        let start = rules
            .find("```markdown\n")
            .expect("the rules must show a note template");
        let block = &rules[start + "```markdown\n".len()..];
        let block = &block[..block.find("```").expect("unterminated code block")];

        let filled = block
            .replace("<the filename, without .md>", "an-observation")
            .replace(
                "session $OMH_SESSION, <this harness>",
                "session s01, claude",
            )
            .replace("<YYYY-MM-DD, the day it happened>", "2026-08-07")
            .replace("# One line naming the surprise", "# A mount failed")
            .replace("## Expected\n", "## Expected\nit would persist\n")
            .replace("## Observed\n", "## Observed\nit did not\n")
            .replace("## Evidence\n", "## Evidence\n`EBUSY`\n")
            .replace(
                "- <the question somebody would later ask to find this>",
                "- why does my login not persist",
            );

        let path = PathBuf::from("an-observation.md");
        let note = parse(&filled, Layer::Local, &path)
            .unwrap_or_else(|e| panic!("the documented shape does not parse: {e}\n\n{filled}"));
        assert_eq!(
            check(&note),
            vec![],
            "the documented shape must satisfy the schema that refuses writes"
        );
    }

    /// Two graphs reach the agent, and they answer different questions. The
    /// code graph knows what the code *is* — where a symbol lives, how one
    /// module reaches another — and is re-derived from the code every turn.
    /// Memory knows what nobody could re-derive: why it is that way, what was
    /// tried and failed, what surprised somebody at 2am.
    ///
    /// Without a rule for choosing, an agent asks the wrong one and concludes
    /// the answer does not exist. The rule cannot live in a tool description
    /// either — a description is attached to one tool and cannot say "prefer
    /// the other one", so this is the part that has to be in the rules file.
    #[test]
    fn the_rules_say_which_of_the_two_graphs_answers_which_question() {
        let rules = shipped_rules();
        let lower = rules.to_lowercase();

        assert!(
            lower.contains("search_graph"),
            "the code graph must be named"
        );
        assert!(lower.contains("recall"), "memory must be named");

        // Naming both is not a rule. There has to be a sentence that tells them
        // apart, and it has to survive somebody rewording the prose around it.
        let has_rule = lower.contains("what the code is") && lower.contains("why");
        assert!(
            has_rule,
            "the rules name both graphs but never say how to choose:\n{rules}"
        );
    }

    /// The trigger is the half a rules file carries badly — it decays as
    /// context grows — so it also rides on the call. A description that only
    /// says what the tool searches leaves the agent to guess when.
    #[test]
    fn recalls_description_says_when_to_reach_for_it_not_only_what_it_holds() {
        let text =
            crate::memory::index::describe(&crate::memory::index::Index::of(&[])).to_lowercase();
        assert!(
            text.contains("code"),
            "it has to distinguish itself from the code graph: {text}"
        );
    }

    /// The agent records what it sees, and inside the sandbox that is
    /// `/work/...`. Host-side that path does not exist, so every `file:`
    /// trigger would report stale the moment it was written.
    #[test]
    fn a_trigger_recorded_in_the_sandbox_is_stored_repo_relative() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.invalidated_by = Some(format!(
            "file:{}/src/main.rs@abc1230",
            crate::container_workdir()
        ));
        remember(&paths, &input, IfExists::Error).unwrap();

        let note = &load(&paths).unwrap()[0];
        assert_eq!(
            note.invalidated_by.as_deref(),
            Some("file:src/main.rs@abc1230"),
            "the sandbox prefix must not survive into the store"
        );
    }

    /// §8's set is closed so that `stale` can evaluate every member. A note
    /// carrying `vibes:soon` advertises an expiry that exists only in the
    /// reader's mind.
    ///
    /// The door is the schema, not the parser. Reading such a note has to keep
    /// working — it may already exist — so `check` is what refuses it, which is
    /// also what puts it in front of somebody in `lint`.
    #[test]
    fn an_invalidation_kind_omh_cannot_evaluate_is_refused_by_the_schema() {
        let raw = SURPRISE.replace(
            "invalidated_by: image:4f2a1c3b5d7e9f0a2b4c6d8e0f1a3b5c7d9e0f1a",
            "invalidated_by: vibes:soon",
        );
        let note = parse(&raw, Layer::Local, std::path::Path::new("x.md"))
            .expect("a note that already exists must still be readable");

        let found = check(&note);
        let bad: Vec<&Violation> = found
            .iter()
            .filter(|v| v.rule == Rule::UnevaluatableTrigger)
            .collect();
        assert_eq!(bad.len(), 1, "got: {found:?}");
        assert!(bad[0].detail.contains("vibes"), "{:?}", bad[0]);
        assert_eq!(
            bad[0].rule.severity(),
            Severity::Refused,
            "a warning would let it ship"
        );
    }

    /// And a refused trigger must leave nothing behind, like any other refused
    /// write.
    ///
    /// One assertion, not three joined by `||`: the original could pass because
    /// the directory happened to be unreadable, which is not the same news.
    #[test]
    fn a_note_with_an_unevaluatable_trigger_is_never_written() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.invalidated_by = Some("whenever:i-feel-like-it".into());
        assert!(remember(&paths, &input, IfExists::Error).is_err());
        assert!(load(&paths).unwrap().is_empty());
    }

    /// **One bad note must not take the store down with it.** `invalidated_by`
    /// was free text through M1–M3, so a legacy value is a file that already
    /// exists on somebody's disk. Refusing it at *load* made `ls`, `recall`,
    /// `stale` and the MCP read path all fail together, and `remember`'s own
    /// opaque check then refused every subsequent write: one hand-edited line,
    /// and the memory is gone.
    #[test]
    fn a_trigger_omh_cannot_evaluate_does_not_take_the_store_down() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "good", &surprise_body());
        let bad = Layer::Local.dir(&paths).join("legacy.md");
        std::fs::write(
            &bad,
            format!(
                "---\nkey: legacy\ntype: surprise\nsource: audit\n\
                 recorded: 2026-08-07\ninvalidated_by: whenever:i-feel-like-it\n---\n\n{}",
                surprise_body()
            ),
        )
        .unwrap();

        let notes = load(&paths).unwrap();
        assert_eq!(notes.len(), 2, "both notes still load");

        // And the lint is where it surfaces, rather than nowhere.
        let found = lint(&paths).unwrap();
        let bad_trigger: Vec<&Violation> = found
            .iter()
            .filter(|v| v.rule == Rule::UnevaluatableTrigger)
            .collect();
        assert_eq!(bad_trigger.len(), 1, "got: {found:?}");
        assert_eq!(bad_trigger[0].key, "legacy");
        assert_eq!(Rule::UnevaluatableTrigger.severity(), Severity::Refused);
    }

    /// **`image:` had no producer.** The digest is a hash of recipe text that
    /// only exists inside the binary, so no command could print it and every
    /// pin a writer invented was wrong from birth — the expiry-that-can-never-
    /// fire this module opens by forbidding. `current` is the name for "what
    /// omh would build now", resolved here so the store still holds a value.
    #[test]
    fn pinning_the_current_image_records_the_digest_omh_would_build() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.invalidated_by = Some(format!("image:{}", expiry::IMAGE_NOW));
        remember(&paths, &input, IfExists::Error).unwrap();

        let recorded = load(&paths).unwrap()[0].invalidated_by.clone().unwrap();
        let expected = crate::image::recipe_digest(&crate::image::base_dockerfile()).unwrap();
        assert_eq!(
            recorded,
            format!("image:{expected}"),
            "the sentinel must not reach the store"
        );
        assert!(
            expiry::Trigger::parse(&recorded).is_ok(),
            "and what lands is a pin omh can evaluate"
        );
    }

    /// **The shape of the vendor's bug, guarded on omh's own renderer.**
    ///
    /// Their remaining quality gap was not a prompt failure: the renderer
    /// rewrote link text as it wrote, so three successive prompt revisions
    /// looked randomly disobeyed. This does not guard *their* renderer — iwe is
    /// never invoked here — it guards `render`/`parse`, which is the one that
    /// could acquire the same habit.
    ///
    /// **Both halves are load-bearing, and the byte comparison is the weaker
    /// one.** It catches a rewrite that differs from pass to pass. It cannot
    /// catch an *idempotent* one — which is what the bug actually was — because
    /// both writes go through the same renderer and agree with each other. The
    /// `[[key]]` assertion below is what sees that, so the two are not
    /// belt-and-braces: they cover different failures.
    ///
    /// This belongs here whether or not hub pages ever ship.
    #[test]
    fn writing_a_note_never_rewrites_its_link_text() {
        let (_d, paths) = fixture();
        let mut input = observation();
        input.relates_to = vec![
            "credentials-are-a-named-volume".into(),
            "surprise/one-inode".into(),
        ];
        let Wrote::Created(path) = remember(&paths, &input, IfExists::Error).unwrap() else {
            panic!()
        };
        let first = std::fs::read(&path).unwrap();

        // Read it back and write it out again by the same route a rename, a
        // promotion or a lint pass would.
        let note = &load(&paths).unwrap()[0];
        std::fs::write(&path, render(note)).unwrap();

        assert_eq!(
            std::fs::read(&path).unwrap(),
            first,
            "a round trip must not touch a single byte, link text least of all"
        );
        let text = String::from_utf8(first).unwrap();
        for key in &input.relates_to {
            assert!(
                text.contains(&format!("[[{key}]]")),
                "the link is stored as written: {text}"
            );
        }
    }

    /// The agent writes into the sandbox, so the path it is given must be the
    /// one omh mounts. A rules file naming the host path sends every note to a
    /// directory that does not exist in the container.
    #[test]
    fn the_staged_rules_name_the_path_the_store_is_mounted_at() {
        assert!(
            shipped_rules().contains(GUEST_LOCAL_NOTES),
            "the rules must point at {GUEST_LOCAL_NOTES}"
        );
    }

    #[test]
    fn every_layer_round_trips_through_its_own_name() {
        for layer in Layer::ALL {
            assert_eq!(Layer::from_str(&layer.to_string()).unwrap(), layer);
        }
    }

    /// An unknown layer names what is accepted rather than defaulting. A
    /// `_ => Local` arm would silently write a typo'd `--layer team` into the
    /// gitignored store and report success.
    #[test]
    fn an_unknown_layer_is_an_error_that_names_the_known_ones() {
        let err = Layer::from_str("shared").unwrap_err().to_string();
        assert!(err.contains("shared"), "got: {err}");
        assert!(err.contains("team") && err.contains("local"), "got: {err}");
    }
}
