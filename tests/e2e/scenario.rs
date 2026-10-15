//! What omh promises a session, each as one scenario run on every harness.
//!
//! A scenario either sees the feature work — in the worktree, or in what the
//! model was sent — or, where the harness has no way to express it, sees omh
//! say so by name. Never neither: a feature that silently did nothing is the
//! failure this suite exists to catch.

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::Value;

use crate::driver::Driver;
use crate::sandbox::{Session, Turn};

struct Scenario {
    name: &'static str,
    run: fn(&Session),
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "rules reach the model",
        run: rules,
    },
    Scenario {
        name: "tdd-guard refuses an edit whose test is untouched",
        run: tdd_refuses,
    },
    Scenario {
        name: "tdd-guard lets the edit through once the test changed",
        run: tdd_allows,
    },
    Scenario {
        name: "tdd-guard refuses in a subdirectory",
        run: tdd_refuses_nested,
    },
    Scenario {
        name: "tdd-guard lets a subdirectory's edit through once its test changed",
        run: tdd_allows_nested,
    },
    Scenario {
        name: "tdd-guard refuses a whole-file write too",
        run: tdd_refuses_a_write,
    },
    Scenario {
        name: "config-guard refuses an edit to omh's own config",
        run: config_refuses,
    },
];

/// The repo every session starts from: one source and test pair per guard
/// scenario, so no scenario's edits can decide another's.
const SEED: &[(&str, &str)] = &[
    ("refused.go", "package refused\n"),
    ("refused_test.go", "package refused\n"),
    ("allowed.go", "package allowed\n"),
    ("allowed_test.go", "package allowed\n"),
    ("pkg/nested.go", "package pkg\n"),
    ("pkg/nested_test.go", "package pkg\n"),
    ("pkg/eased.go", "package pkg\n"),
    ("pkg/eased_test.go", "package pkg\n"),
    ("written.go", "package written\n"),
    ("written_test.go", "package written\n"),
];

const HOOKS: &[&str] = &["tdd-guard", "config-guard"];

/// Every scenario, in one session of `harness`. One session, because each
/// holds a network from Docker's finite address pool; every scenario, even
/// after one fails, so a run reports all of what broke rather than the first.
pub fn all(harness: &'static Driver) {
    let session = Session::start(harness, SEED, HOOKS);
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

/// Whether the model was told `reason` after a call: a refusal reaches the
/// model as the call's result, in the next request it is sent.
fn told(turn: &Turn, reason: &str) -> bool {
    turn.asks()
        .iter()
        .skip(1)
        .any(|r| Turn::seen(r).contains(reason))
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

const TDD: &str = "test-first: nothing has changed in the test file for";

/// Read, then edit: Claude Code refuses an edit to a file it has not read, and
/// a model reads first anyway.
fn edit_script(h: &Driver, file: &str, old: &str, new: &str) -> Vec<Value> {
    vec![(h.read)(file), (h.edit)(file, old, new)]
}

fn refuses(s: &Session, file: &str, old: &str) {
    let before = s.read(file);
    let turn = s.turn(
        "Edit the file.",
        &edit_script(s.harness, file, old, "// edited"),
    );
    ran(&turn);
    assert!(
        told(&turn, TDD),
        "the model was never told tdd-guard's reason for {file}: {}",
        turn.printed()
    );
    assert_eq!(
        s.read(file),
        before,
        "{file} changed although the guard refused it"
    );
}

fn allows(s: &Session, file: &str, test: &str, old: &str) {
    s.write(test, &format!("{old}\n// a new test\n"));
    let turn = s.turn(
        "Edit the file.",
        &edit_script(s.harness, file, old, "// edited"),
    );
    ran(&turn);
    assert!(
        !told(&turn, TDD),
        "tdd-guard refused {file} although {test} had changed: {}",
        turn.printed()
    );
    if s.harness.edits_land {
        assert!(
            s.read(file).is_some_and(|c| c.contains("// edited")),
            "{file} did not change: {}",
            turn.printed()
        );
    }
}

fn tdd_refuses(s: &Session) {
    refuses(s, "refused.go", "package refused");
}

fn tdd_allows(s: &Session) {
    allows(s, "allowed.go", "allowed_test.go", "package allowed");
}

fn tdd_refuses_nested(s: &Session) {
    refuses(s, "pkg/nested.go", "package pkg");
}

fn tdd_allows_nested(s: &Session) {
    allows(s, "pkg/eased.go", "pkg/eased_test.go", "package pkg");
}

/// A write is an edit of the whole file, and a guard on edits that a write
/// walks past guards nothing.
fn tdd_refuses_a_write(s: &Session) {
    let before = s.read("written.go");
    let turn = s.turn(
        "Rewrite the file.",
        &[
            (s.harness.read)("written.go"),
            (s.harness.write)("written.go", "package written\n// rewritten\n"),
        ],
    );
    ran(&turn);
    assert!(
        told(&turn, TDD),
        "the model was never told tdd-guard's reason for a write to written.go: {}",
        turn.printed()
    );
    assert_eq!(
        s.read("written.go"),
        before,
        "written.go changed although the guard refused it"
    );
}

/// `.omh/` decides what the next session gets; a session editing it is
/// approving its own work.
fn config_refuses(s: &Session) {
    let file = ".omh/settings.toml";
    let before = s.read(file);
    assert!(before.is_some(), "the seed has no {file} to guard");
    let turn = s.turn(
        "Change the settings.",
        &[
            (s.harness.read)(file),
            (s.harness.write)(file, "# rewritten by the session\n"),
        ],
    );
    ran(&turn);
    assert!(
        told(&turn, "decides what the next session gets"),
        "the model was never told config-guard's reason for {file}: {}",
        turn.printed()
    );
    assert_eq!(
        s.read(file),
        before,
        "{file} changed although the guard refused it"
    );
}
