//! The review: what a session changed among its preferences, put to the user
//! before any of it reaches the account (#197 §4).
//!
//! Per declared key, three readings of a session — `reviewed`, what its last
//! decision left; `live`, what the harness has now; `base`, what the account
//! kept when the session last synced — and the account's value now:
//!
//! | `reviewed` → `live` | account now vs `base` | shown as |
//! |---|---|---|
//! | equal | — | not shown |
//! | changed (absent ↔ present too) | equal | keep / drop |
//! | changed | differs: another session kept something | a conflict: mine / theirs |
//!
//! Keeping takes `live`'s value into the account, `base` and `reviewed`;
//! dropping leaves the account as it is, takes its value into `base` and
//! `live`'s into `reviewed`, so the same change is not asked about again.
//! A quiet key is kept without asking. Nothing is assumed: an answer nobody
//! gave leaves the change to review, at the next `down` or `rm`.

use super::{dotted, get, place, read_doc, set, undeclared, Doc, Value};
use super::{KEPT, LOCK, RECORD, REST};
use crate::adapter::{Adapter, Declared, KeyPath, PrefMount};
use crate::home::Home;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The account a session's preferences were seeded from, or `None` for one
/// never seeded.
pub fn account_of(home: &Home) -> Result<Option<PathBuf>> {
    let at = home.path().join(RECORD);
    match std::fs::read_to_string(&at) {
        Ok(a) => Ok(Some(PathBuf::from(a))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", at.display())),
    }
}

/// The declared keys a session changed that no decision has answered yet,
/// dotted — quiet ones aside, which are kept without asking. Nothing for a
/// session never seeded. `live` is read as the sandbox's, and one omh cannot
/// read is an error, never "nothing to review".
pub fn unreviewed(adapter: &Adapter, home: &Home) -> Result<Vec<String>> {
    let Some(prefs) = adapter.preferences.as_ref() else {
        return Ok(Vec::new());
    };
    if account_of(home)?.is_none() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for d in prefs.declared() {
        let side = Side::read(home, d)?;
        out.extend(side.changed().map(dotted));
    }
    Ok(out)
}

/// One preferences file of a session, as the review reads it: `base` and
/// `reviewed` are omh's own; `live` the sandbox's, read once.
struct Side {
    declared: Declared,
    base: Doc,
    reviewed: Doc,
    live: Doc,
    /// The file is a copy of an account file, whose other keys are the
    /// harness's bookkeeping and never the session's to keep or name.
    copy: bool,
}

impl Side {
    fn read(home: &Home, declared: Declared) -> Result<Side> {
        let root = home.path();
        let name = OsStr::new(&declared.name);
        let format = declared.format();
        let own = |part: &str| -> Result<Doc> {
            let dir = crate::held::Dir::open(&root.join(part))
                .with_context(|| format!("opening {}", root.join(part).display()))?;
            Ok(read_doc(&dir, name, format)?.unwrap_or_default())
        };
        let live = crate::held::Dir::open(&super::live(home))
            .map_err(anyhow::Error::from)
            .and_then(|dir| read_doc(&dir, name, format))
            .with_context(|| {
                format!(
                    "could not tell what this session changed in its {}",
                    declared.name
                )
            })?
            .unwrap_or_default();
        Ok(Side {
            base: own("base")?,
            reviewed: own("reviewed")?,
            live,
            copy: root.join(REST).join(name).is_file(),
            declared,
        })
    }

    /// The asked-about keys whose `live` value is not the reviewed one.
    fn changed(&self) -> impl Iterator<Item = &KeyPath> {
        self.declared
            .keys
            .iter()
            .filter(|k| get(&self.reviewed, k) != get(&self.live, k))
    }

    /// Write `base` and `reviewed` back.
    fn write(&self, home: &Home, mount: PrefMount) -> Result<()> {
        let name = OsStr::new(&self.declared.name);
        let format = self.declared.format();
        for (part, doc) in [("base", &self.base), ("reviewed", &self.reviewed)] {
            let dir = crate::held::Dir::open(&home.path().join(part))?;
            place(&dir, name, &format.render(doc)?, mount)?;
        }
        Ok(())
    }
}

/// A session under review.
pub struct Sitting {
    pub id: String,
    pub home: Home,
}

/// How the review is answered.
pub enum Answers<'a> {
    /// Somebody at a terminal is asked.
    Ask {
        input: &'a mut dyn BufRead,
        out: &'a mut dyn Write,
    },
    /// `rm --yes`, said in advance: every change dropped, and named.
    DropAll,
}

/// What a review did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// What was kept, dropped, and not offered, one line each.
    pub notes: Vec<String>,
    /// The changes still to review, as `<session> <key>`.
    pub left: Vec<String>,
}

/// One change, offered.
struct Item {
    sitting: usize,
    file: usize,
    key: KeyPath,
    /// The account's value had already moved from `base` when the review
    /// began: shown, and kept by "keep all" as this session's.
    conflict: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Keep,
    Drop,
    Leave,
}

/// Review `sittings` — sessions of `adapter` on the same account — under the
/// account's lock, held from the first read to the last write: a launch
/// seeding from the account, or another review, waits. One prompt for them
/// all; a conflict this review itself makes between two of them is asked
/// about in turn.
pub fn review(adapter: &Adapter, sittings: &[Sitting], mut answers: Answers) -> Result<Outcome> {
    let Some(prefs) = adapter.preferences.as_ref() else {
        return Ok(Outcome::default());
    };
    let mut outcome = Outcome::default();
    // The account first, from each session's record — then its lock, under
    // which every reading below is taken, and held to the last write: two
    // reviews of one account never decide over the same reading.
    let mut account = None;
    let mut on: Vec<usize> = Vec::new();
    for (i, s) in sittings.iter().enumerate() {
        let Some(this) = account_of(&s.home)? else {
            continue;
        };
        anyhow::ensure!(
            account.as_ref().is_none_or(|a| *a == this),
            "{} launched on another account than the sessions reviewed beside it",
            s.id
        );
        account = Some(this);
        on.push(i);
    }
    let Some(account) = account else {
        return Ok(outcome);
    };
    let shown_account = account
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (kept_dir, _) = super::make_parent(&account, &format!("{KEPT}/{LOCK}"))?;
    let _lock = kept_dir
        .lock(OsStr::new(LOCK))
        .with_context(|| format!("locking {}", account.join(KEPT).display()))?;
    let mut read: Vec<(usize, Vec<Side>)> = Vec::new();
    for i in on {
        let s = &sittings[i];
        let sides: Result<Vec<Side>> = prefs
            .declared()
            .into_iter()
            .map(|d| Side::read(&s.home, d))
            .collect();
        match sides {
            Ok(sides) => read.push((i, sides)),
            Err(e) => {
                outcome.notes.push(crate::out::untrusted(&format!(
                    "{}: {e:#} — left to review",
                    s.id
                )));
                outcome.left.push(format!("{} (could not tell)", s.id));
            }
        }
    }
    let declared = prefs.declared();
    let mut kept: Vec<Doc> = Vec::new();
    for d in &declared {
        let doc = read_doc(&kept_dir, OsStr::new(&d.name), d.format())?.unwrap_or_default();
        kept.push(super::pick(&doc, d.all()));
    }
    let before = kept.clone();

    // Quiet keys, kept without asking: last writer wins.
    for (_, sides) in &mut read {
        for (f, side) in sides.iter_mut().enumerate() {
            for key in side.declared.quiet.clone() {
                let now = get(&side.live, &key).cloned();
                if get(&side.reviewed, &key) != now.as_ref() {
                    keep(side, &mut kept[f], &key, now);
                }
            }
        }
    }

    let mut items = Vec::new();
    for (r, (_, sides)) in read.iter().enumerate() {
        for (f, side) in sides.iter().enumerate() {
            for key in side.changed() {
                items.push(Item {
                    sitting: r,
                    file: f,
                    key: key.clone(),
                    conflict: get(&kept[f], key) != get(&side.base, key),
                });
            }
        }
    }

    let ids: Vec<String> = read.iter().map(|(i, _)| sittings[*i].id.clone()).collect();
    let id = |r: usize| ids[r].as_str();
    let many = read.len() > 1;
    let line = |item: &Item, sides: &[Side], kept: &[Doc]| -> String {
        let side = &sides[item.file];
        let mut s = format!(
            "{}{}  {}  →  {}",
            if many {
                format!("{}  ", id(item.sitting))
            } else {
                String::new()
            },
            dotted(&item.key),
            shown(get(&kept[item.file], &item.key)),
            shown(get(&side.live, &item.key)),
        );
        if get(&kept[item.file], &item.key) != get(&side.base, &item.key) {
            s.push_str(&format!(
                "   (another session kept {} since this one started)",
                shown(get(&kept[item.file], &item.key))
            ));
        }
        crate::out::untrusted(&s)
    };

    // The answer for them all: said in advance for `rm --yes`, asked once
    // otherwise — and nothing, for Enter or a closed input.
    let all = match &mut answers {
        _ if items.is_empty() => None,
        Answers::DropAll => Some('d'),
        Answers::Ask { input, out } => {
            let sessions: Vec<&str> = (0..read.len()).map(id).collect();
            writeln!(
                out,
                "{} changed {} {} preference{} on account `{}`:",
                crate::out::untrusted(&sessions.join(" and ")),
                items.len(),
                adapter.name,
                if items.len() == 1 { "" } else { "s" },
                crate::out::untrusted(&shown_account)
            )?;
            for item in &items {
                writeln!(out, "  {}", line(item, &read[item.sitting].1, &kept))?;
            }
            for (r, (_, sides)) in read.iter().enumerate() {
                let left = not_preferences(sides);
                if !left.is_empty() {
                    writeln!(
                        out,
                        "{}not kept, not preferences: {}",
                        if many {
                            format!("{}: ", id(r))
                        } else {
                            String::new()
                        },
                        crate::out::untrusted(&left.join(", "))
                    )?;
                }
            }
            ask(
                &mut **input,
                &mut **out,
                "[k]eep all  [r]eview each  [d]rop all  — Enter leaves them to review",
                &['k', 'r', 'd'],
            )?
        }
    };

    // Each change decided, and applied as it is, so the next one's conflict
    // is read against what this one kept.
    let mut decided = vec![Decision::Leave; items.len()];
    for (n, item) in items.iter().enumerate() {
        let (_, sides) = &read[item.sitting];
        let side = &sides[item.file];
        let conflict = get(&kept[item.file], &item.key) != get(&side.base, &item.key);
        // "Keep all" keeps this session's value over a conflict shown above;
        // one this review made, by keeping another session's value a moment
        // ago, was not shown, so it is asked.
        let asked = all == Some('r') || (all == Some('k') && conflict && !item.conflict);
        decided[n] = match (all, &mut answers) {
            (Some('d'), _) => Decision::Drop,
            (Some('k') | Some('r'), Answers::Ask { input, out }) if asked => {
                let (choices, keep_key) = match conflict {
                    true => ("[m]ine  [t]heirs", 'm'),
                    false => ("[k]eep  [d]rop", 'k'),
                };
                let question = format!("{}   {choices}", line(item, sides, &kept));
                let drop_key = if conflict { 't' } else { 'd' };
                match ask(&mut **input, &mut **out, &question, &[keep_key, drop_key])? {
                    Some(c) if c == keep_key => Decision::Keep,
                    Some(_) => Decision::Drop,
                    None => Decision::Leave,
                }
            }
            (Some('k') | Some('r'), _) => Decision::Keep,
            _ => Decision::Leave,
        };
        apply(&mut read, &mut kept, item, decided[n]);
    }

    for (f, d) in declared.iter().enumerate() {
        if kept[f] != before[f] {
            kept_dir.replace_private(OsStr::new(&d.name), &d.format().render(&kept[f])?)?;
        }
    }
    for (i, sides) in &read {
        for side in sides {
            side.write(&sittings[*i].home, prefs.mount)?;
        }
    }

    for (what, which) in [("kept", Decision::Keep), ("dropped", Decision::Drop)] {
        let keys: Vec<String> = items
            .iter()
            .zip(&decided)
            .filter(|(_, d)| **d == which)
            .map(|(item, _)| format!("{} {}", id(item.sitting), dotted(&item.key)))
            .collect();
        if !keys.is_empty() {
            outcome.notes.push(crate::out::untrusted(&format!(
                "{what} {} {} preference{}{}: {}",
                keys.len(),
                adapter.name,
                if keys.len() == 1 { "" } else { "s" },
                match which {
                    Decision::Keep => format!(" on account `{shown_account}`"),
                    _ => String::new(),
                },
                keys.join(", ")
            )));
        }
    }
    if matches!(answers, Answers::DropAll) {
        for (r, (_, sides)) in read.iter().enumerate() {
            let left = not_preferences(sides);
            if !left.is_empty() {
                outcome.notes.push(crate::out::untrusted(&format!(
                    "{}: not kept, not preferences: {}",
                    id(r),
                    left.join(", ")
                )));
            }
        }
    }
    outcome.left.extend(
        items
            .iter()
            .zip(&decided)
            .filter(|(_, d)| **d == Decision::Leave)
            .map(|(item, _)| format!("{} {}", id(item.sitting), dotted(&item.key))),
    );
    Ok(outcome)
}

/// A decision's effect on the account and on the session's readings.
fn apply(read: &mut [(usize, Vec<Side>)], kept: &mut [Doc], item: &Item, d: Decision) {
    let side = &mut read[item.sitting].1[item.file];
    let now = get(&side.live, &item.key).cloned();
    match d {
        Decision::Keep => keep(side, &mut kept[item.file], &item.key, now),
        Decision::Drop => {
            set(
                &mut side.base,
                &item.key,
                get(&kept[item.file], &item.key).cloned(),
            );
            set(&mut side.reviewed, &item.key, now);
        }
        Decision::Leave => {}
    }
}

/// Keep `value` for `key`: the account, `base` and `reviewed` take it.
fn keep(side: &mut Side, kept: &mut Doc, key: &KeyPath, value: Option<Value>) {
    set(kept, key, value.clone());
    set(&mut side.base, key, value.clone());
    set(&mut side.reviewed, key, value);
}

/// What a session's own files hold that is not a preference, by key — never
/// a copy's, whose other keys are the harness's own.
fn not_preferences(sides: &[Side]) -> Vec<String> {
    sides
        .iter()
        .filter(|s| !s.copy)
        .flat_map(|s| {
            let keys: Vec<&KeyPath> = s.declared.all().collect();
            undeclared(&s.live, &keys)
        })
        .collect()
}

/// A value as the review shows it: JSON, so a newline or a quote is escaped
/// rather than drawn — and through `out::untrusted` by the caller, for what
/// JSON leaves as it is.
fn shown(v: Option<&Value>) -> String {
    match v {
        Some(v) => v.to_string(),
        None => "(unset)".to_string(),
    }
}

/// Ask until one of `choices` is answered; `None` for Enter and for a closed
/// input, neither of which is an answer.
fn ask(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    question: &str,
    choices: &[char],
) -> Result<Option<char>> {
    loop {
        write!(out, "{question}\n  > ")?;
        out.flush()?;
        let mut said = String::new();
        if input.read_line(&mut said)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        let said = said.trim().to_lowercase();
        let Some(first) = said.chars().next() else {
            return Ok(None);
        };
        if choices.contains(&first) {
            return Ok(Some(first));
        }
    }
}

/// The preferences homes of session `id` under `run`, one per harness it
/// ran, by harness name.
pub fn homes_of(run: &Path) -> Result<Vec<(String, Home)>> {
    let dir = run.join(".preferences");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        out.push((
            entry.file_name().to_string_lossy().into_owned(),
            Home::Kept(entry.path()),
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn claude() -> Adapter {
        Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            "claude",
        )
        .unwrap()
    }

    fn json(raw: &str) -> Doc {
        crate::adapter::PrefFormat::Json
            .parse(raw.as_bytes())
            .unwrap()
    }

    fn read_json(path: &Path) -> Doc {
        json(&std::fs::read_to_string(path).unwrap_or_default())
    }

    /// An account `work` keeping `settings`, and sessions on it.
    struct Fx {
        root: tempfile::TempDir,
        account: PathBuf,
    }

    impl Fx {
        fn new(settings: &str) -> Fx {
            let root = tempfile::tempdir().unwrap();
            let account = root.path().join("creds/claude/work");
            std::fs::create_dir_all(account.join("preferences")).unwrap();
            std::fs::write(account.join("preferences/settings.json"), settings).unwrap();
            std::fs::write(account.join("preferences/.claude.json"), "{}").unwrap();
            Fx { root, account }
        }

        /// Session `id`, seeded from the account as it is now.
        fn session(&self, id: &str) -> Sitting {
            let home = Home::Kept(
                self.root
                    .path()
                    .join("run/repo")
                    .join(id)
                    .join(".preferences/claude"),
            );
            super::super::check(&claude(), &self.account)
                .unwrap()
                .unwrap()
                .seed(&claude(), &home, &BTreeMap::new(), false)
                .unwrap();
            Sitting {
                id: id.to_string(),
                home,
            }
        }

        fn kept(&self) -> Doc {
            read_json(&self.account.join("preferences/settings.json"))
        }

        fn keep_on_account(&self, raw: &str) {
            std::fs::write(self.account.join("preferences/settings.json"), raw).unwrap();
        }
    }

    fn write_live(s: &Sitting, raw: &str) {
        std::fs::write(super::super::live(&s.home).join("settings.json"), raw).unwrap();
    }

    fn part(s: &Sitting, part: &str) -> Doc {
        read_json(&s.home.path().join(part).join("settings.json"))
    }

    /// The review answered with `typed`, and what it printed.
    fn answered(sittings: &[Sitting], typed: &str) -> (Outcome, String) {
        let mut out = Vec::new();
        let outcome = review(
            &claude(),
            sittings,
            Answers::Ask {
                input: &mut typed.as_bytes(),
                out: &mut out,
            },
        )
        .unwrap();
        (outcome, String::from_utf8(out).unwrap())
    }

    /// The table's first two rows: a key whose `live` is its reviewed value
    /// is not shown; one changed, the account where `base` left it, is
    /// offered — and kept, it is the account's, `base`'s and `reviewed`'s.
    #[test]
    fn a_change_is_offered_and_keeping_it_reaches_the_account() {
        let fx = Fx::new(r#"{"model":"a","theme":"dark"}"#);
        let s = fx.session("s01");
        write_live(&s, r#"{"model":"b","theme":"dark","hooks":{"x":1}}"#);
        assert_eq!(unreviewed(&claude(), &s.home).unwrap(), vec!["model"]);

        let (outcome, said) = answered(std::slice::from_ref(&s), "k\n");
        assert!(
            said.contains("s01 changed 1 claude preference on account `work`:")
                && said.contains(r#"model  "a"  →  "b""#)
                && !said.contains("theme")
                && said.contains("not kept, not preferences: hooks"),
            "{said}"
        );
        assert_eq!(fx.kept(), json(r#"{"model":"b","theme":"dark"}"#));
        assert_eq!(part(&s, "base"), json(r#"{"model":"b","theme":"dark"}"#));
        assert_eq!(
            part(&s, "reviewed"),
            json(r#"{"model":"b","theme":"dark"}"#)
        );
        assert!(outcome.left.is_empty(), "{:?}", outcome.left);
        assert!(
            outcome
                .notes
                .contains(&"kept 1 claude preference on account `work`: s01 model".to_string()),
            "{:?}",
            outcome.notes
        );
        assert!(unreviewed(&claude(), &s.home).unwrap().is_empty());
    }

    /// Dropped, the account is untouched and the change is not asked about
    /// again — until the session changes the key once more.
    #[test]
    fn a_dropped_change_is_not_asked_again() {
        let fx = Fx::new(r#"{"model":"a"}"#);
        let s = fx.session("s01");
        write_live(&s, r#"{"model":"b"}"#);
        let (outcome, _) = answered(std::slice::from_ref(&s), "d\n");
        assert_eq!(fx.kept(), json(r#"{"model":"a"}"#));
        assert_eq!(part(&s, "base"), json(r#"{"model":"a"}"#));
        assert_eq!(part(&s, "reviewed"), json(r#"{"model":"b"}"#));
        assert!(
            outcome
                .notes
                .contains(&"dropped 1 claude preference: s01 model".to_string()),
            "{:?}",
            outcome.notes
        );
        assert!(unreviewed(&claude(), &s.home).unwrap().is_empty());
        let (_, said) = answered(std::slice::from_ref(&s), "");
        assert!(said.is_empty(), "asked again: {said}");
        write_live(&s, r#"{"model":"c"}"#);
        assert_eq!(unreviewed(&claude(), &s.home).unwrap(), vec!["model"]);
    }

    /// *Absent* is a value: a key the session removed is a change, and
    /// keeping it removes it from the account.
    #[test]
    fn keeping_a_removal_removes_the_key_from_the_account() {
        let fx = Fx::new(r#"{"model":"a","verbose":true}"#);
        let s = fx.session("s01");
        write_live(&s, r#"{"verbose":true}"#);
        let (_, said) = answered(std::slice::from_ref(&s), "k\n");
        assert!(said.contains(r#"model  "a"  →  (unset)"#), "{said}");
        assert_eq!(fx.kept(), json(r#"{"verbose":true}"#));
    }

    /// The third row: another session kept the key since this one started.
    /// Both values are shown; theirs leaves the account and takes its value
    /// into `base`, so it is not a conflict again; mine is a keep.
    #[test]
    fn a_conflict_shows_both_values_and_theirs_or_mine_decides() {
        for (typed, account, base) in [
            ("r\nt\n", r#"{"model":"other"}"#, r#"{"model":"other"}"#),
            ("r\nm\n", r#"{"model":"mine"}"#, r#"{"model":"mine"}"#),
            ("k\n", r#"{"model":"mine"}"#, r#"{"model":"mine"}"#),
        ] {
            let fx = Fx::new(r#"{"model":"a"}"#);
            let s = fx.session("s01");
            write_live(&s, r#"{"model":"mine"}"#);
            fx.keep_on_account(r#"{"model":"other"}"#);
            let (outcome, said) = answered(std::slice::from_ref(&s), typed);
            assert!(
                said.contains(r#"model  "other"  →  "mine""#)
                    && said.contains(r#"(another session kept "other" since this one started)"#),
                "{typed:?}: {said}"
            );
            assert_eq!(fx.kept(), json(account), "{typed:?}");
            assert_eq!(part(&s, "base"), json(base), "{typed:?}");
            assert_eq!(part(&s, "reviewed"), json(r#"{"model":"mine"}"#));
            assert!(outcome.left.is_empty(), "{typed:?}");
        }
    }

    /// Nothing is assumed: Enter, and a closed input, decide nothing — and
    /// a walk through each change leaves the one not answered. A quiet key
    /// would be kept regardless; claude declares none, so the left-over check
    /// is the account untouched.
    #[test]
    fn no_answer_decides_nothing() {
        for typed in ["", "\n", "r\n", "r\n\nk\n"] {
            let fx = Fx::new(r#"{"model":"a","theme":"dark"}"#);
            let s = fx.session("s01");
            write_live(&s, r#"{"model":"b","theme":"light"}"#);
            let (outcome, _) = answered(std::slice::from_ref(&s), typed);
            let kept = fx.kept();
            match typed {
                "r\n\nk\n" => {
                    assert_eq!(kept, json(r#"{"model":"a","theme":"light"}"#));
                    assert_eq!(outcome.left, vec!["s01 model"]);
                }
                _ => {
                    assert_eq!(kept, json(r#"{"model":"a","theme":"dark"}"#), "{typed:?}");
                    assert_eq!(outcome.left, vec!["s01 model", "s01 theme"], "{typed:?}");
                    assert_eq!(
                        part(&s, "reviewed"),
                        json(r#"{"model":"a","theme":"dark"}"#)
                    );
                }
            }
        }
    }

    /// `rm --yes`: every change dropped and named, and what the session wrote
    /// that is not a preference named too — never kept.
    #[test]
    fn drop_all_names_each_change_and_keeps_nothing() {
        let fx = Fx::new(r#"{"model":"a"}"#);
        let s = fx.session("s01");
        write_live(
            &s,
            r#"{"model":"b","editorMode":"vim","enabledPlugins":{"p":true}}"#,
        );
        let outcome = review(&claude(), std::slice::from_ref(&s), Answers::DropAll).unwrap();
        assert_eq!(fx.kept(), json(r#"{"model":"a"}"#));
        assert_eq!(
            outcome.notes,
            vec![
                "dropped 2 claude preferences: s01 model, s01 editorMode".to_string(),
                "s01: not kept, not preferences: enabledPlugins".to_string(),
            ]
        );
        assert!(outcome.left.is_empty());
    }

    /// Two sessions on one account are one prompt. Keeping both, the second's
    /// change to the key the first just kept is a conflict this review made:
    /// not shown in the summary, so asked in turn.
    #[test]
    fn two_sessions_are_one_prompt_and_a_conflict_between_them_is_asked() {
        let fx = Fx::new(r#"{"model":"a"}"#);
        let (one, two) = (fx.session("s01"), fx.session("s02"));
        write_live(&one, r#"{"model":"b"}"#);
        write_live(&two, r#"{"model":"c","verbose":true}"#);
        let (outcome, said) = answered(&[one, two], "k\nt\n");
        assert!(
            said.contains("s01 and s02 changed 3 claude preferences on account `work`:")
                && said.contains(r#"s01  model  "a"  →  "b""#)
                && said.contains(r#"s02  model  "a"  →  "c""#)
                && said.contains(r#"s02  model  "b"  →  "c"   (another session kept "b" since this one started)   [m]ine  [t]heirs"#),
            "{said}"
        );
        assert_eq!(fx.kept(), json(r#"{"model":"b","verbose":true}"#));
        assert!(outcome.left.is_empty(), "{:?}", outcome.left);
    }

    /// A value is shown as JSON — a newline or a quote escaped, never drawn
    /// — and what JSON leaves, an escape character, is replaced.
    #[test]
    fn a_value_is_shown_as_json_and_never_drawn() {
        let fx = Fx::new("{}");
        let s = fx.session("s01");
        write_live(&s, "{\"theme\":\"a\\nb\\\"c\u{202e}\"}");
        let (_, said) = answered(std::slice::from_ref(&s), "");
        assert!(said.contains(r#"theme  (unset)  →  "a\nb\"c"#), "{said}");
        assert!(!said.contains('\u{202e}'), "{said:?}");
    }

    /// A copy the sandbox turned into a link is "could not tell" — never
    /// "nothing to review" — and the review leaves it, keeping nothing.
    #[test]
    fn a_live_copy_omh_cannot_read_is_could_not_tell() {
        let fx = Fx::new(r#"{"model":"a"}"#);
        let s = fx.session("s01");
        let at = super::super::live(&s.home).join("settings.json");
        std::fs::remove_file(&at).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", &at).unwrap();
        let err = format!("{:#}", unreviewed(&claude(), &s.home).unwrap_err());
        assert!(
            err.contains("could not tell what this session changed"),
            "{err}"
        );
        let (outcome, _) = answered(std::slice::from_ref(&s), "k\n");
        assert_eq!(outcome.left, vec!["s01 (could not tell)"]);
        assert_eq!(fx.kept(), json(r#"{"model":"a"}"#));
    }

    /// A quiet key — bookkeeping the harness writes on its own — is kept
    /// without asking, even when nothing else is answered.
    #[test]
    fn a_quiet_key_is_kept_without_asking() {
        let adapters = Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters");
        let codex = Adapter::find(&adapters, "codex").unwrap();
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("creds/codex/work");
        std::fs::create_dir_all(account.join("preferences")).unwrap();
        std::fs::write(account.join("preferences/config.toml"), "model = \"a\"\n").unwrap();
        let home = Home::Kept(root.path().join("run/repo/s01/.preferences/codex"));
        let checked = super::super::check(&codex, &account).unwrap().unwrap();
        checked.apply(&|| Ok(Vec::new())).unwrap();
        checked
            .seed(&codex, &home, &BTreeMap::new(), false)
            .unwrap();
        std::fs::write(
            super::super::live(&home).join("config.toml"),
            "model = \"b\"\n[tui]\nmodel_availability_nux = 3\n",
        )
        .unwrap();
        assert_eq!(unreviewed(&codex, &home).unwrap(), vec!["model"]);
        let sitting = Sitting {
            id: "s01".into(),
            home,
        };
        let mut out = Vec::new();
        let outcome = review(
            &codex,
            std::slice::from_ref(&sitting),
            Answers::Ask {
                input: &mut "".as_bytes(),
                out: &mut out,
            },
        )
        .unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(!said.contains("model_availability_nux"), "{said}");
        assert_eq!(outcome.left, vec!["s01 model"]);
        assert_eq!(
            std::fs::read_to_string(account.join("preferences/config.toml")).unwrap(),
            "model = \"a\"\n\n[tui]\nmodel_availability_nux = 3\n"
        );
    }

    /// A copy of an account file holds the harness's own bookkeeping beside
    /// its preferences; none of it is named as what the session wrote.
    #[test]
    fn a_copys_other_keys_are_never_named() {
        let fx = Fx::new("{}");
        let home = Home::Kept(fx.root.path().join("run/repo/s01/.preferences/claude"));
        let rests = BTreeMap::from([(
            ".claude.json".to_string(),
            r#"{"oauthAccount":{"emailAddress":"a@b"}}"#.to_string(),
        )]);
        super::super::check(&claude(), &fx.account)
            .unwrap()
            .unwrap()
            .seed(&claude(), &home, &rests, false)
            .unwrap();
        let copy = super::super::live(&home).join(".claude.json");
        std::fs::write(
            &copy,
            r#"{"oauthAccount":{"emailAddress":"a@b"},"numStartups":2,"copyOnSelect":true}"#,
        )
        .unwrap();
        write_live(
            &Sitting {
                id: "s01".into(),
                home: Home::Kept(home.path().to_path_buf()),
            },
            r#"{"hooks":{}}"#,
        );
        let sitting = Sitting {
            id: "s01".into(),
            home,
        };
        let outcome = review(&claude(), std::slice::from_ref(&sitting), Answers::DropAll).unwrap();
        assert_eq!(
            outcome.notes,
            vec![
                "dropped 1 claude preference: s01 copyOnSelect".to_string(),
                "s01: not kept, not preferences: hooks".to_string(),
            ]
        );
    }
}
