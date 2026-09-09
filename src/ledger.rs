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
/// Claude's `{when} || exit 0` cannot draw this distinction from a plain exit
/// status and never emits it; that is a limit of that renderer, not of this
/// enum.
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
/// bytes to interpolate into a shell or JS literal, and building the object
/// here — instead of in three renderers separately — is what keeps
/// `Observation::parse`'s two keys, `hook` and `decision`, from drifting out
/// of step with what gets written.
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
    pub fn parse_all(text: &str) -> Self {
        let mut entries = Vec::new();
        let mut unreadable = 0;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match Observation::parse(line) {
                Some(o) => entries.push(o),
                None => unreadable += 1,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Every hook with at least one observation, in name order.
    pub activity: BTreeMap<String, Activity>,
    /// Rendered by this launch and never once observed. Derivable only from
    /// having `Ledger` and `Observations` as separate types — see the module
    /// doc.
    pub dormant: Vec<String>,
    /// Lines `Observations::parse_all` could not read as an observation.
    pub unreadable: usize,
}

impl Summary {
    pub fn of(ledger: &Ledger, observed: &Observations) -> Self {
        let mut activity: BTreeMap<String, Activity> = BTreeMap::new();
        for o in &observed.entries {
            let a = activity.entry(o.hook.clone()).or_default();
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
        // The renderers never build this JSON by hand — `ledger::line` is the
        // one place it is assembled, and this is what keeps that assembly
        // from drifting out of step with `Observation::parse`.
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
