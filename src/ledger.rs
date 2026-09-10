//! The decision ledger: what a session's hooks did, read back after a launch.
//!
//! Two kinds of fact, and they get two types for the reason `shadow.rs` gives
//! at its own top: **never trust what the sandbox asserts.** [`Ledger`] is
//! what omh itself rendered for a launch — host-written, at plan time, never
//! touched by the container. [`Observations`] is what the sandbox's own hooks
//! reported about themselves, written into the same mounted, agent-writable
//! gitdir `shadow.rs` already documents as one the agent can rewrite. Folding
//! both into one list would mean a hook's own `run` could forge the row that
//! says it fired.
//!
//! [`Summary::of`] is the only place the two meet, and it is what makes a
//! *dormant* hook expressible at all: a hook that never fired is `Ledger`
//! minus observed, which needs both sides as separate sets to compute.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one hook did against one tool call, in omh's words.
///
/// `Unevaluated` is not `Silent`: a `when` that returned false and a `when`
/// that could not be run both leave a call unblocked, but only the second is
/// omp's own `!p.ran` branch (`render.rs`'s `omp_one_hook`) — a refusal whose
/// predicate never ran, so the call is blocked rather than allowed unchecked.
/// Claude's `{when} || { <log silent>; exit 0; }` cannot draw this
/// distinction from a plain exit status and never emits it; that is a limit
/// of that renderer, not of this enum. (The bare `{when} || exit 0` this
/// once named is what `hook::render` emits only when nothing is logging at
/// all — every launch that records a ledger gets the wrapper.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Silent,
    Fired,
    Refused,
    Unevaluated,
}

impl Decision {
    /// The word this decision writes to the wire — used by the renderers to
    /// build the literal they hand to a shell or JS log call, so that word
    /// and `Observation::parse`'s reader agree by construction rather than by
    /// two call sites happening to spell the same string.
    pub fn wire(self) -> &'static str {
        match self {
            Decision::Silent => "silent",
            Decision::Fired => "fired",
            Decision::Refused => "refused",
            Decision::Unevaluated => "unevaluated",
        }
    }
}

/// One JSON-line statement a rendered hook runs to record a decision, in the
/// vocabulary shared by every renderer that logs at all.
///
/// A plain `String` rather than a `Value`: what a renderer needs is exact
/// bytes to interpolate into a shell literal, and this is what keeps
/// `hook::log_statement` (claude, the only renderer that actually calls this
/// function) from hand-rolling the object and drifting out of step with
/// `Observation::parse`'s two keys.
///
/// The two JS renderers do **not** call this — `render.rs`'s `LOG_BRIDGE`
/// hand-rolls `JSON.stringify({ hook, decision })` in generated JavaScript,
/// which cannot call a Rust function. Those two field names are kept in step
/// with `Wire`'s by hand, not by construction; `render::tests::
/// every_harness_that_has_hooks_logs_them` is what would actually catch a
/// drift there, by driving real `node` output back through
/// `Observation::parse`.
pub fn line(name: &str, decision: Decision) -> String {
    serde_json::json!({ "hook": name, "decision": decision.wire() }).to_string()
}

/// One line of what the sandbox reported about one hook.
///
/// Agent-writable, so nothing here is taken on trust — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub hook: String,
    pub decision: Decision,
}

/// The wire shape, kept separate from [`Observation`] so nothing outside this
/// module can hold a value that skipped `parse`.
#[derive(Debug, Deserialize)]
struct Wire {
    hook: String,
    decision: Decision,
}

impl Observation {
    /// The only door in — mirrors `hook::Hook::parse`. Unlike that one, this
    /// returns `None` rather than an `Err`: a torn or malformed line is not a
    /// reason to abort a read of the whole file, it is one line for
    /// `Observations::parse_all` to count as unreadable and move past.
    pub fn parse(line: &str) -> Option<Self> {
        let wire: Wire = serde_json::from_str(line.trim()).ok()?;
        Some(Self {
            hook: wire.hook,
            decision: wire.decision,
        })
    }
}

/// Every observation from one session's events file, plus what could not be
/// read as one.
///
/// A blank or absent file is zero of both — not the same state as lines that
/// existed and parsed as nothing, which is why unreadable lines are counted
/// rather than silently dropped (`omh_log` itself never writes a line that
/// would fail this parse; a line failing here means something else wrote to,
/// or mangled, the file).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observations {
    pub entries: Vec<Observation>,
    pub unreadable: usize,
}

impl Observations {
    /// Every line of an events file, from a `&str` or from raw bytes.
    ///
    /// Bytes, not text, is the honest input: the file is agent-writable, so
    /// it can hold anything. Splitting on `\n` *before* decoding is what
    /// makes "one poisoned byte costs one line" true rather than nearly
    /// true. Decoding the whole file with `from_utf8_lossy` first turns a
    /// bad byte into U+FFFD, and U+FFFD inside a JSON string value leaves
    /// the line perfectly valid — so a mangled `graph-<?>read` parsed
    /// cleanly, missed the ledger it could never match, and was reported as
    /// a hook nobody rendered. One corrupt byte manufactured a forgery
    /// warning. A line that is not valid UTF-8 is damage, and damage is
    /// counted.
    ///
    /// `\n` cannot appear inside a multi-byte UTF-8 sequence, so splitting
    /// first can never cut a character in half.
    pub fn parse_all(input: impl AsRef<[u8]>) -> Self {
        let mut entries = Vec::new();
        let mut unreadable = 0;
        for line in input.as_ref().split(|b| *b == b'\n') {
            match std::str::from_utf8(line) {
                Ok(text) if text.trim().is_empty() => continue,
                Ok(text) => match Observation::parse(text) {
                    Some(o) => entries.push(o),
                    None => unreadable += 1,
                },
                Err(_) => unreadable += 1,
            }
        }
        Self {
            entries,
            unreadable,
        }
    }
}

/// What omh rendered for one launch — host-written, at plan time, trusted.
///
/// Just the names a hook could have logged under: enough to tell a hook that
/// rendered and never fired apart from one that was never part of the launch
/// at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Ledger {
    pub hooks: Vec<String>,
}

/// Tally for one hook, over every observation naming it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    pub fired: usize,
    pub silent: usize,
    pub refused: usize,
    pub unevaluated: usize,
}

/// A launch's hooks, read back against what actually happened.
///
/// `activity` and `unlisted` are the same trust split the module doc opens
/// with, applied to the observations themselves rather than only to
/// `dormant`: an observation naming a hook this launch's `Ledger` actually
/// rendered goes in `activity`, and one naming anything else is counted,
/// never dropped, but kept out so it cannot read as a genuine row in a hook
/// this launch never rendered.
///
/// What that buys is bounded, and worth stating precisely, because this is
/// the module whose thesis is never trusting what the sandbox asserts. The
/// split keeps *unrendered* names out of `activity`. It does not make the
/// rows inside `activity` trustworthy: the events file is a fixed path in a
/// mount the agent can write, and the guest is handed every rendered hook
/// name in its own settings document, so a `run` can append a line under any
/// of those names and it will be counted as that hook's own doing. Within
/// the launch's own names, an observation remains the sandbox's claim about
/// itself. `shadow.rs`'s doc on `events_ledger` states the same boundary
/// from the other side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Every hook the ledger named, with at least one observation, in name
    /// order.
    pub activity: BTreeMap<String, Activity>,
    /// Rendered by this launch and never once observed. Derivable only from
    /// having `Ledger` and `Observations` as separate types — see the module
    /// doc.
    pub dormant: Vec<String>,
    /// Observations naming a hook absent from the ledger — see the struct
    /// doc. Never *non*-empty by accident, now that a launch starts its own
    /// events file empty: a name here was either invented by something
    /// running in the sandbox, or rendered by a renderer that forgot to
    /// record it. Both are worth showing rather than dropping, and the
    /// second is the one a reader is likelier to be looking at, which is why
    /// `render::tests::every_renderer_records_what_it_rendered` checks every
    /// hooks-capable renderer's `rendered_hooks` and not just claude's.
    pub unlisted: BTreeMap<String, Activity>,
    /// Lines `Observations::parse_all` could not read as an observation.
    pub unreadable: usize,
}

impl Summary {
    pub fn of(ledger: &Ledger, observed: &Observations) -> Self {
        let rendered: std::collections::BTreeSet<&str> =
            ledger.hooks.iter().map(String::as_str).collect();
        let mut activity: BTreeMap<String, Activity> = BTreeMap::new();
        let mut unlisted: BTreeMap<String, Activity> = BTreeMap::new();
        for o in &observed.entries {
            let a = if rendered.contains(o.hook.as_str()) {
                activity.entry(o.hook.clone()).or_default()
            } else {
                unlisted.entry(o.hook.clone()).or_default()
            };
            match o.decision {
                Decision::Silent => a.silent += 1,
                Decision::Fired => a.fired += 1,
                Decision::Refused => a.refused += 1,
                Decision::Unevaluated => a.unevaluated += 1,
            }
        }
        let dormant = ledger
            .hooks
            .iter()
            .filter(|h| !activity.contains_key(h.as_str()))
            .cloned()
            .collect();
        Self {
            activity,
            dormant,
            unlisted,
            unreadable: observed.unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unparseable_line_is_counted_not_dropped() {
        let text = "{\"hook\":\"graph-read\",\"decision\":\"fired\"}\nnot json at all\n";
        let observed = Observations::parse_all(text);
        assert_eq!(observed.entries.len(), 1);
        assert_eq!(observed.unreadable, 1);
    }

    #[test]
    fn blank_lines_are_neither_entries_nor_unreadable() {
        let text = "\n\n  \n";
        let observed = Observations::parse_all(text);
        assert_eq!(observed.entries.len(), 0);
        assert_eq!(observed.unreadable, 0);
    }

    #[test]
    fn a_hook_that_never_fired_is_dormant_not_absent() {
        let ledger = Ledger {
            hooks: vec!["graph-orient".into(), "graph-first".into()],
        };
        let observed =
            Observations::parse_all("{\"hook\":\"graph-orient\",\"decision\":\"fired\"}\n");
        let summary = Summary::of(&ledger, &observed);
        assert!(summary.activity.contains_key("graph-orient"));
        assert_eq!(summary.dormant, vec!["graph-first".to_string()]);
    }

    #[test]
    fn a_hook_never_rendered_is_not_dormant() {
        // `Summary::of` must read dormancy off the ledger, not off the set of
        // hooks a stray observation happens to name — an observation for a
        // hook this launch never rendered (a stale file from an older
        // session, say) must not make some *other*, real hook look present.
        let ledger = Ledger {
            hooks: vec!["graph-orient".into()],
        };
        let observed = Observations::parse_all(
            "{\"hook\":\"a-hook-this-launch-never-rendered\",\"decision\":\"fired\"}\n",
        );
        let summary = Summary::of(&ledger, &observed);
        assert_eq!(summary.dormant, vec!["graph-orient".to_string()]);
    }

    /// The other half of the fixture above: that stray observation must not
    /// land in `activity` either. `activity` is what a reader takes as "this
    /// launch's own hook did this" — a hook's own `run` writing a line under
    /// any name it likes must not be able to forge one, and a name left over
    /// from an earlier launch of a resumed session (the events file outlives
    /// a launch, the ledger does not) must read as what it is rather than as
    /// this launch's activity.
    #[test]
    fn an_observation_for_a_hook_outside_the_ledger_is_unlisted_not_activity() {
        let ledger = Ledger {
            hooks: vec!["graph-orient".into()],
        };
        let observed = Observations::parse_all(
            "{\"hook\":\"tdd-guard\",\"decision\":\"fired\"}\n\
             {\"hook\":\"tdd-guard\",\"decision\":\"fired\"}\n",
        );
        let summary = Summary::of(&ledger, &observed);
        assert!(
            !summary.activity.contains_key("tdd-guard"),
            "a hook outside the ledger must not appear as activity: {:?}",
            summary.activity
        );
        assert_eq!(
            summary.unlisted["tdd-guard"].fired, 2,
            "but it is still counted, never dropped"
        );
    }

    #[test]
    fn counts_split_by_decision_not_totalled() {
        let ledger = Ledger {
            hooks: vec!["graph-read".into()],
        };
        let observed = Observations::parse_all(
            "{\"hook\":\"graph-read\",\"decision\":\"fired\"}\n\
             {\"hook\":\"graph-read\",\"decision\":\"silent\"}\n\
             {\"hook\":\"graph-read\",\"decision\":\"silent\"}\n",
        );
        let summary = Summary::of(&ledger, &observed);
        let a = summary.activity["graph-read"];
        assert_eq!((a.fired, a.silent, a.refused, a.unevaluated), (1, 2, 0, 0));
    }

    #[test]
    fn the_written_line_parses_back_to_the_same_observation() {
        // Claude's renderer never builds this JSON by hand — `line` is the
        // one place it assembles it — so this is what keeps `line` itself
        // from drifting out of step with `Observation::parse`. The two JS
        // renderers hand-roll their own JSON independently; see `line`'s
        // own doc for how that half is checked instead.
        let written = line("graph-read", Decision::Fired);
        let parsed = Observation::parse(&written).expect("line must parse back");
        assert_eq!(parsed.hook, "graph-read");
        assert_eq!(parsed.decision, Decision::Fired);
    }

    #[test]
    fn an_unevaluated_guard_is_its_own_decision_not_silent() {
        let observed =
            Observations::parse_all("{\"hook\":\"tdd-guard\",\"decision\":\"unevaluated\"}\n");
        assert_eq!(observed.entries[0].decision, Decision::Unevaluated);
    }
}
