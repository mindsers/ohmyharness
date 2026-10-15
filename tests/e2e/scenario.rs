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
    Scenario {
        name: "graph-orient reaches the model at session start",
        run: graph_orient,
    },
    Scenario {
        name: "graph-read answers the read of a large file",
        run: graph_read,
    },
    Scenario {
        name: "graph-first is named dropped, with no search tool to narrow to",
        run: graph_first,
    },
    Scenario {
        name: "git-turn photographs the turn's tree",
        run: git_turn,
    },
    Scenario {
        name: "graph-refresh runs at the end of a turn",
        run: graph_refresh,
    },
    Scenario {
        name: "a hook runs, or injects, at every event",
        run: every_event,
    },
    Scenario {
        name: "the memory server keeps what the model remembers, for the next turn",
        run: memory_round_trip,
    },
    Scenario {
        name: "the code graph server answers the model",
        run: code_graph_answers,
    },
    Scenario {
        name: "omh reads the turn back from the harness's transcript, or says it cannot",
        run: transcript,
    },
    Scenario {
        name: "omh doctor passes in the session's repo",
        run: doctor,
    },
    Scenario {
        name: "the model is the only thing the harness reaches, but for what is named",
        run: nothing_else,
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
    // `run` and `inject` at every event. The tool moments are narrowed to one
    // command, so they cannot touch any other scenario's turn.
    (
        ".omh/hooks/e2e-start-run.json",
        r#"{"on":"session-start","run":"echo x >> /tmp/e2e-start-run"}"#,
    ),
    (
        ".omh/hooks/e2e-start-inject.json",
        r#"{"on":"session-start","inject":"E2E-START-INJECT"}"#,
    ),
    (
        ".omh/hooks/e2e-before-run.json",
        r#"{"on":"before-tool","tools":["shell"],"when":"[ \"$OMH_TOOL_COMMAND\" = 'echo e2e-hooks' ]","run":"echo x >> /tmp/e2e-before-run"}"#,
    ),
    (
        ".omh/hooks/e2e-before-inject.json",
        r#"{"on":"before-tool","tools":["shell"],"when":"[ \"$OMH_TOOL_COMMAND\" = 'echo e2e-hooks' ]","inject":"E2E-BEFORE-INJECT"}"#,
    ),
    (
        ".omh/hooks/e2e-after-run.json",
        r#"{"on":"after-tool","tools":["shell"],"when":"[ \"$OMH_TOOL_COMMAND\" = 'echo e2e-hooks' ]","run":"echo x >> /tmp/e2e-after-run"}"#,
    ),
    (
        ".omh/hooks/e2e-after-inject.json",
        r#"{"on":"after-tool","tools":["shell"],"when":"[ \"$OMH_TOOL_COMMAND\" = 'echo e2e-hooks' ]","inject":"E2E-AFTER-INJECT"}"#,
    ),
    (
        ".omh/hooks/e2e-end-run.json",
        r#"{"on":"turn-end","run":"echo x >> /tmp/e2e-end-run"}"#,
    ),
    (
        ".omh/hooks/e2e-end-inject.json",
        r#"{"on":"turn-end","inject":"E2E-END-INJECT"}"#,
    ),
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

/// A hook either did what it is for, or omh said it could not: the launch
/// line names every hook it dropped, and why.
fn worked_or_named(s: &Session, hook: &str, worked: bool) {
    assert!(
        worked || s.dropped(hook),
        "{hook} neither worked nor was named dropped on {}. omh new said:\n{}",
        s.harness.name,
        s.launched
    );
}

/// The lines in a file in the container, which a `run` hook appends to.
fn lines(s: &Session, file: &str) -> usize {
    let out = s.sh(&format!("cat {file} 2>/dev/null | wc -l"));
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn graph_orient(s: &Session) {
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    let seen = Turn::seen(turn.asks()[0]).contains("Code graph for project");
    worked_or_named(s, "graph-orient", seen);
}

/// Over the 8,000 bytes graph-read waits for. Written in the turn's own
/// worktree rather than seeded, since nothing needs it committed.
fn graph_read(s: &Session) {
    s.write(
        "big.go",
        &format!("package big\n{}", "// filler line\n".repeat(600)),
    );
    let turn = s.turn("Read it.", &[(s.harness.read)("big.go")]);
    ran(&turn);
    let told = told(&turn, "is large. For one symbol rather than the whole file");
    worked_or_named(s, "graph-read", told);
}

/// No harness omh supports has one tool for searching: Claude Code searches
/// through its shell, and the others have `grep` and `glob` apart. A hook
/// narrowed to `search` would match nothing, so it is dropped by name.
fn graph_first(s: &Session) {
    assert!(
        s.dropped("graph-first"),
        "graph-first was not named dropped on {}. omh new said:\n{}",
        s.harness.name,
        s.launched
    );
}

fn git_turn(s: &Session) {
    let turn = s.turn(
        "Take notes.",
        &[(s.harness.write)("notes.txt", "turn notes\n")],
    );
    ran(&turn);
    let shot = s.sh("git show refs/omh/turn:notes.txt");
    let photographed = String::from_utf8_lossy(&shot.stdout).contains("turn notes");
    worked_or_named(s, "git-turn", photographed);
}

fn graph_refresh(s: &Session) {
    let before = fired(s, "graph-refresh");
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    worked_or_named(s, "graph-refresh", fired(s, "graph-refresh") > before);
}

/// How many times omh's ledger records `hook` as fired. Parsed, not grepped:
/// the shell and plugin renderers write the same two keys in either order.
fn fired(s: &Session, hook: &str) -> usize {
    let out = s.sh("cat /omh/shadow/omh-events.jsonl 2>/dev/null");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["hook"] == hook && e["decision"] == "fired")
        .count()
}

/// `run` and `inject` at each of the four moments, from hooks in the repo's
/// `.omh/hooks`. A `run` is seen by what it appended during the turn; an
/// `inject` by its text in what the model was sent after it.
fn every_event(s: &Session) {
    let runs = ["start", "before", "after", "end"].map(|m| format!("/tmp/e2e-{m}-run"));
    let before: Vec<usize> = runs.iter().map(|f| lines(s, f)).collect();
    let turn = s.turn("Run it.", &[(s.harness.shell)("echo e2e-hooks")]);
    ran(&turn);
    let ran_during = |i: usize| lines(s, &runs[i]) > before[i];
    let first = Turn::seen(turn.asks()[0]);
    let anywhere = |text: &str| turn.requests.iter().any(|r| Turn::seen(r).contains(text));

    let mut silent = Vec::new();
    for (hook, worked) in [
        ("e2e-start-run", ran_during(0)),
        ("e2e-start-inject", first.contains("E2E-START-INJECT")),
        ("e2e-before-run", ran_during(1)),
        ("e2e-before-inject", told(&turn, "E2E-BEFORE-INJECT")),
        ("e2e-after-run", ran_during(2)),
        ("e2e-after-inject", told(&turn, "E2E-AFTER-INJECT")),
        ("e2e-end-run", ran_during(3)),
        ("e2e-end-inject", anywhere("E2E-END-INJECT")),
    ] {
        if !worked && !s.dropped(hook) {
            silent.push(hook);
        }
    }
    assert!(
        silent.is_empty(),
        "these hooks neither worked nor were named dropped on {}: {silent:?}\nomh new said:\n{}\n{}",
        s.harness.name,
        s.launched,
        turn.printed()
    );
}

/// Everything a harness tried to reach other than the model, during a turn,
/// beyond the hosts its driver names and says why.
///
/// Through the proxy only: a process that ignores `HTTPS_PROXY`, or is
/// started without it, goes out directly and is not seen here.
fn nothing_else(s: &Session) {
    let turn = s.turn("Run it.", &[(s.harness.shell)("true")]);
    ran(&turn);
    let unnamed: Vec<String> = turn
        .elsewhere()
        .into_iter()
        .filter(|h| !s.harness.elsewhere.contains(&h.as_str()))
        .collect();
    assert!(
        unnamed.is_empty(),
        "{} reached: {unnamed:?}",
        s.harness.name
    );
}

/// A note written through the memory server in one turn is found by a
/// search in the next. Two turns, so the second's conversation holds the
/// note only if the server handed it back.
fn memory_round_trip(s: &Session) {
    let mark = format!("E2E-NOTE-{}", s.harness.name.to_uppercase());
    let note = serde_json::json!({
        "expected": "the e2e suite expected nothing in particular",
        "observed": mark,
        "evidence": "image:current",
        "answers": ["what did the e2e suite record"],
    });
    let wrote = s.turn("Remember it.", &(s.harness.mcp)("memory", "remember", note));
    ran(&wrote);
    let recalled = s.turn(
        "Recall it.",
        &(s.harness.mcp)(
            "memory",
            "recall",
            serde_json::json!({ "question": "what did the e2e suite record" }),
        ),
    );
    ran(&recalled);
    // A recall lists notes by key, and the server derives a note's key from
    // what was observed, lower-cased: `surprise/e2e-note-claude`.
    let results = recalled.results();
    assert!(
        results
            .iter()
            .any(|r| r.to_lowercase().contains(&mark.to_lowercase())),
        "the recall did not return {mark}. What the calls returned:\n{results:?}\n{}",
        recalled.printed()
    );
}

/// The code graph server lists the session's project: the server started,
/// answered, and reads the index omh keeps for this worktree.
fn code_graph_answers(s: &Session) {
    let project = format!("{}-s01", s.id);
    let turn = s.turn(
        "List the projects.",
        &(s.harness.mcp)("codegraph", "list_projects", serde_json::json!({})),
    );
    ran(&turn);
    let results = turn.results();
    assert!(
        results.iter().any(|r| r.contains(&project)),
        "list_projects did not name {project}. What the calls returned:\n{results:?}\n{}",
        turn.printed()
    );
}

/// After a turn with one shell call, `omh s01` reads that call back from the
/// harness's transcript — or says, as itself, that this harness keeps none
/// omh can read. Never an empty history that looks like nothing happened.
fn transcript(s: &Session) {
    let activity = |s: &Session| -> Value {
        let out = s.omh(&["s01", "--json"]);
        serde_json::from_slice::<Value>(&out.stdout)
            .map(|v| v["focus"]["activity"].clone())
            .unwrap_or_else(|_| panic!("omh s01 --json: {}", crate::sandbox::text(&out)))
    };
    let shells = |a: &Value| -> u64 {
        a["tools"]
            .as_object()
            .map(|t| t.values().filter_map(Value::as_u64).sum())
            .unwrap_or(0)
    };
    let before = activity(s);
    let turn = s.turn("Run it.", &[(s.harness.shell)("echo transcript")]);
    ran(&turn);
    let after = activity(s);
    match after["state"].as_str() {
        Some("read") => assert!(
            shells(&after) > shells(&before),
            "the transcript was read and holds no new call: before {before}, after {after}"
        ),
        Some("not-recorded") => assert!(
            after["why"]
                .as_str()
                .is_some_and(|w| w.contains("does not record transcripts")),
            "not recorded, and not said why: {after}"
        ),
        _ => panic!("omh read no transcript and said nothing about why: {after}"),
    }
}

/// `omh doctor` for this harness, in the repo the session runs on: the same
/// probes a person runs, against the same catalogue and image.
fn doctor(s: &Session) {
    let out = s.omh(&["doctor", "--harness", s.harness.name, "--json"]);
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("omh doctor --json: {}", crate::sandbox::text(&out)));
    let failed: Vec<&Value> = report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["ok"] != true)
        .collect();
    assert!(
        report["ok"] == true && failed.is_empty(),
        "omh doctor failed on {}: {failed:#?}",
        s.harness.name
    );
}
