//! What omh promises a session, each as one scenario run on every harness.
//!
//! A scenario either sees the feature work — in the worktree, or in what the
//! model was sent — or, where the harness has no way to express it, sees omh
//! say so by name. Never neither: a feature that silently did nothing is the
//! failure this suite exists to catch.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::driver::Driver;
use crate::sandbox::{Session, Turn};

struct Scenario {
    name: &'static str,
    run: fn(&Session),
}

const SCENARIOS: &[Scenario] = &[Scenario {
    name: "rules reach the model",
    run: rules,
}];

/// Every scenario, in one session of `harness`. One session, because each
/// holds a network from Docker's finite address pool; every scenario, even
/// after one fails, so a run reports all of what broke rather than the first.
pub fn all(harness: &'static Driver) {
    let session = Session::start(harness);
    let failed: Vec<String> = SCENARIOS
        .iter()
        .filter_map(|s| {
            catch_unwind(AssertUnwindSafe(|| (s.run)(&session)))
                .err()
                .map(|e| format!("{}: {}", s.name, message(&e)))
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{} of {} scenarios failed on {}:\n\n{}",
        failed.len(),
        SCENARIOS.len(),
        harness.name,
        failed.join("\n\n")
    );
}

fn message(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic with no message".into())
}

/// A turn that ran, and asked the model at least once.
fn ran(turn: &Turn) {
    assert!(
        turn.out.status.success(),
        "the turn failed: {}",
        turn.printed()
    );
    assert!(
        !turn.asks().is_empty(),
        "the harness never asked the model for a next step: {} requests, {}",
        turn.requests.len(),
        turn.printed()
    );
}

/// omh's rules — composed per session, mounted where the harness reads its
/// project instructions — are in what the model is sent, line for line.
///
/// Except omh's provenance markers, `<!-- omh: base:graph-rules -->`: they
/// are for `omh why`, not the model, and Claude Code strips HTML comments from
/// the instructions it sends.
fn rules(s: &Session) {
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    let seen = Turn::seen(turn.asks()[0]);
    let rules = s.rendered("rules.md");
    let missing: Vec<&str> = rules
        .lines()
        .map(str::trim)
        .filter(|l| l.len() >= 20 && !(l.starts_with("<!--") && l.ends_with("-->")))
        .filter(|l| !seen.contains(l))
        .collect();
    assert!(
        missing.is_empty(),
        "{} of omh's rules lines never reached the model:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
