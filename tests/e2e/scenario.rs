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

/// Run in this order, in one session. Each scenario before the session's
/// life works on files of its own, so those could run in any order; the
/// life cannot, and comes last.
const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "rules reach the model",
        run: rules,
    },
    Scenario {
        name: "omh drops exactly the hooks this harness cannot express",
        run: drops,
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
        name: "a server the session adds itself does not load",
        run: planted_server,
    },
    Scenario {
        name: "omh doctor passes in the session's repo",
        run: doctor,
    },
    Scenario {
        name: "through its proxy, the harness reaches the model and what is named",
        run: nothing_else,
    },
    // The session's life, in order and last: each leaves what the next needs,
    // and the last removes the session.
    Scenario {
        name: "the turn's work shows in omh s01 diff",
        run: lifecycle_diff,
    },
    Scenario {
        name: "the agent's own commit shows in omh s01 log",
        run: lifecycle_log,
    },
    Scenario {
        name: "omh s01 commit lands the work on the session's branch",
        run: lifecycle_commit,
    },
    Scenario {
        name: "omh s01 sync brings the base in, and the next turn is told",
        run: lifecycle_sync,
    },
    Scenario {
        name: "omh s01 down stops the session, and resume brings it back",
        run: lifecycle_down_resume,
    },
    Scenario {
        name: "omh s01 rm removes the session and keeps its branch",
        run: lifecycle_rm,
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

/// A turn that ran, asked the model, and played the whole script: a turn that
/// stopped early would pass every check that something did *not* happen.
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
    assert_eq!(
        turn.unreadable(),
        0,
        "the fake model could not parse a request: {}",
        turn.printed()
    );
    assert!(
        turn.played(),
        "the turn ended before the script did: {}",
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
    let checked: Vec<&str> = rules
        .lines()
        .map(str::trim)
        .filter(|l| l.len() >= 20 && !(l.starts_with("<!--") && l.ends_with("-->")))
        .collect();
    assert!(
        !checked.is_empty(),
        "omh composed no rules to look for:\n{rules}"
    );
    let missing: Vec<&str> = checked.into_iter().filter(|l| !seen.contains(l)).collect();
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
    assert!(before.is_some(), "the seed has no {file} to guard");
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
    assert!(
        s.read(file).is_some_and(|c| c.contains("// edited")),
        "{file} did not change: {}",
        turn.printed()
    );
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

/// A hook did what it is for, or — when the driver expects this harness to
/// have no way to express it — omh named it dropped. Both ways strict: a hook
/// that stopped working fails, and so does one omh newly drops.
fn worked_or_named(s: &Session, hook: &str, worked: bool) {
    if s.harness.drops.contains(&hook) {
        assert!(
            s.dropped(hook),
            "{hook} was expected dropped on {} and was not. omh new said:\n{}",
            s.harness.name,
            s.launched
        );
    } else {
        assert!(
            worked && !s.dropped(hook),
            "{hook} did not work on {} (dropped: {}). omh new said:\n{}",
            s.harness.name,
            s.dropped(hook),
            s.launched
        );
    }
}

/// omh drops exactly the hooks the driver expects for this harness, and no
/// others.
fn drops(s: &Session) {
    let want: std::collections::BTreeSet<String> =
        s.harness.drops.iter().map(|h| h.to_string()).collect();
    assert_eq!(
        s.dropped_all(),
        want,
        "omh dropped a different set on {}. omh new said:\n{}",
        s.harness.name,
        s.launched
    );
}

/// The lines in a file in the container, which a `run` hook appends to. A
/// file not there yet is none; one there and unreadable, or a container
/// that could not be asked, fails.
fn lines(s: &Session, file: &str) -> usize {
    let out = s.sh(&format!(
        "if [ -e {file} ]; then wc -l < {file}; else echo 0; fi"
    ));
    assert!(
        out.status.success(),
        "counting {file}: {}",
        crate::sandbox::text(&out)
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("counting {file}: {}", crate::sandbox::text(&out)))
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

/// No harness omh supports has one tool for searching: Claude Code and Codex
/// search through their shell, and omp and opencode have `grep` and `glob`
/// apart. A hook narrowed to `search` would match nothing, so it is dropped by
/// name.
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
    // Not there yet is no firings; there and unreadable fails.
    let out = s.sh("f=/omh/shadow/omh-events.jsonl; if [ -e $f ]; then cat $f; fi");
    assert!(
        out.status.success(),
        "reading the ledger: {}",
        crate::sandbox::text(&out)
    );
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

    let mut wrong = Vec::new();
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
        let expected_dropped = s.harness.drops.contains(&hook);
        if expected_dropped != s.dropped(hook) || (!expected_dropped && !worked) {
            wrong.push(format!(
                "{hook}: worked {worked}, dropped {}, expected dropped {expected_dropped}",
                s.dropped(hook)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "hooks that did not do what {} should: {wrong:#?}\nomh new said:\n{}\n{}",
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
    let wrote = s.turn(
        "Remember it.",
        &(s.harness.mcp)("omh-memory", "remember", note),
    );
    ran(&wrote);
    let recalled = s.turn(
        "Recall it.",
        &(s.harness.mcp)(
            "omh-memory",
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
        &(s.harness.mcp)("omh-codegraph", "list_projects", serde_json::json!({})),
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
    let want = if s.harness.transcripts {
        "read"
    } else {
        "not-recorded"
    };
    assert_eq!(
        after["state"].as_str(),
        Some(want),
        "omh s01 should report {want} on {}: {after}",
        s.harness.name
    );
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

/// A server the agent adds — with the harness's own command, or into the file
/// directly — lands in the account, which every later session on it reads. The policy omh fills from
/// the launch leaves it out, and keeps omh's own in. Same command as an omh
/// server would not help: the lock compares arguments too.
///
/// One session is enough: the policy is read at every start of the harness,
/// and `mcp list` is a start. Harnesses with no policy omh fills are skipped.
fn planted_server(s: &Session) {
    let (add, list, remove) = match s.harness.name {
        "codex" => (
            "codex mcp add e2e-planted -- /bin/true",
            "codex mcp list --json",
            "codex mcp remove e2e-planted",
        ),
        // Written into the file rather than through `claude mcp add`, which
        // refuses under the lock ("not allowed by enterprise policy") — a
        // session is not held to the CLI. `cat >` keeps the bind-mounted inode.
        "claude" => (
            r#"jq '.mcpServers["e2e-planted"] = {"command":"/bin/true","args":[]}' ~/.claude.json > /tmp/planted && cat /tmp/planted > ~/.claude.json"#,
            "claude mcp list",
            r#"jq 'del(.mcpServers["e2e-planted"])' ~/.claude.json > /tmp/planted && cat /tmp/planted > ~/.claude.json"#,
        ),
        _ => return,
    };
    ok(&s.sh(add), add);
    let listed = s.sh(list);
    let _ = s.sh(remove);
    let listed = ok(&listed, list);
    match s.harness.name {
        "codex" => {
            let servers: Vec<Value> = serde_json::from_str(&listed)
                .unwrap_or_else(|e| panic!("{list} is not JSON ({e}): {listed}"));
            let named = |name: &str| {
                servers
                    .iter()
                    .find(|v| v["name"] == name)
                    .unwrap_or_else(|| panic!("{name} is not listed: {listed}"))
            };
            let planted = named("e2e-planted");
            assert!(
                planted["enabled"] == false
                    && planted["disabled_reason"]
                        .as_str()
                        .is_some_and(|r| r.contains("requirements")),
                "the planted server loads: {planted}"
            );
            for own in ["omh-codegraph", "omh-memory"] {
                assert_eq!(named(own)["enabled"], true, "{own}: {listed}");
            }
        }
        _ => {
            assert!(
                !listed.contains("e2e-planted"),
                "the planted server loads: {listed}"
            );
            for own in ["omh-codegraph", "omh-memory"] {
                assert!(listed.contains(own), "{own} is not listed: {listed}");
            }
        }
    }
}

/// `omh doctor` for this harness, in the repo the session runs on: the same
/// probes a person runs, against the same catalogue and image.
fn doctor(s: &Session) {
    let out = s.omh(&["doctor", "--harness", s.harness.name, "--json"]);
    // Every document, not one: a failed run once printed two, and parsing
    // stdout as a single value then panicked on "trailing characters" with
    // the failed checks buried in the dump. The checks come first, then the
    // count, so a failure names what failed.
    let documents: Vec<Value> = serde_json::Deserializer::from_slice(&out.stdout)
        .into_iter::<Value>()
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| {
            panic!(
                "omh doctor --json is not JSON ({e}): {}",
                crate::sandbox::text(&out)
            )
        });
    let failed: Vec<&Value> = documents
        .iter()
        .flat_map(|d| d["checks"].as_array().into_iter().flatten())
        .filter(|c| c["ok"] != true)
        .collect();
    assert!(
        failed.is_empty() && documents.iter().all(|d| d["ok"] == true),
        "omh doctor failed on {}: {failed:#?}\n{}",
        s.harness.name,
        crate::sandbox::text(&out)
    );
    // Asked, not merely not failed: a check doctor skips is a row missing,
    // which the assertion above reads as a pass. The memory probe, the
    // document omh's servers are rendered into, and — where the harness can
    // be asked — whether it loaded them under their `omh-…` names. The last
    // two were skipped on every install with no `mcp.json` of its own.
    let rows: Vec<&str> = documents
        .iter()
        .flat_map(|d| d["checks"].as_array().into_iter().flatten())
        .filter_map(|c| c["name"].as_str())
        .collect();
    let asked = ["claude", "codex"].contains(&s.harness.name);
    for row in ["memory", "mcp"]
        .into_iter()
        .chain(asked.then_some("mcp-loaded"))
    {
        assert!(
            rows.contains(&row),
            "omh doctor never checked `{row}` on {}: {rows:?}\n{}",
            s.harness.name,
            crate::sandbox::text(&out)
        );
    }
    assert_eq!(
        documents.len(),
        1,
        "omh doctor --json printed {} documents on {}: {}",
        documents.len(),
        s.harness.name,
        crate::sandbox::text(&out)
    );
}

fn ok(out: &std::process::Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what}: {}",
        crate::sandbox::text(out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn lifecycle_diff(s: &Session) {
    let turn = s.turn(
        "Write it.",
        &[(s.harness.write)("lifecycle.txt", "from the agent\n")],
    );
    ran(&turn);
    let diff = ok(&s.omh(&["s01", "diff", "--json"]), "omh s01 diff");
    assert!(
        diff.contains("lifecycle.txt"),
        "omh s01 diff does not name lifecycle.txt: {diff}"
    );
}

fn lifecycle_log(s: &Session) {
    let turn = s.turn(
        "Commit it.",
        &[(s.harness.shell)(
            "git add lifecycle.txt && git commit -q -m 'e2e: the agent commits'",
        )],
    );
    ran(&turn);
    let log = ok(&s.omh(&["s01", "log", "--json"]), "omh s01 log");
    assert!(
        log.contains("e2e: the agent commits"),
        "omh s01 log does not list the agent's commit: {log}\n{}",
        turn.printed()
    );
}

fn lifecycle_commit(s: &Session) {
    let turn = s.turn("Write it.", &[(s.harness.write)("landed.txt", "landed\n")]);
    ran(&turn);
    ok(
        &s.omh(&["s01", "commit", "-m", "e2e: landed", "--json"]),
        "omh s01 commit",
    );
    let shown = s.git(&["show", "omh/s01:landed.txt"]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("landed"),
        "landed.txt is not on omh/s01"
    );
}

/// A commit lands on `main` while the session runs. `sync` refuses a running
/// session — it would move files under an agent that believes otherwise — so
/// `--down` stops it first; the commit is then in the worktree, and `git-note`
/// hands the next session start the note the sync left.
fn lifecycle_sync(s: &Session) {
    s.git(&["switch", "-q", "main"]);
    std::fs::write(s.repo.join("from-main.txt"), "from main\n").unwrap();
    s.git(&["add", "from-main.txt"]);
    s.git(&["commit", "-q", "-m", "e2e: main moves"]);
    ok(
        &s.omh(&["s01", "sync", "--down", "--json"]),
        "omh s01 sync --down",
    );
    assert!(
        s.read("from-main.txt").is_some(),
        "sync did not bring from-main.txt into the worktree"
    );
    s.resume();
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    let told = Turn::seen(turn.asks()[0]).contains("main moved 1 commit under you");
    worked_or_named(s, "git-note", told);
}

/// `down` removes the container — it runs with `--rm` — and `resume` starts a
/// new one, which stops at the terminal like `omh new` does. The worktree
/// carries over; what the driver wrote into the old container does not.
fn lifecycle_down_resume(s: &Session) {
    ok(&s.omh(&["s01", "down"]), "omh s01 down");
    assert!(!s.running(), "the session is still running after down");
    s.resume();
    let turn = s.turn("Read it.", &[(s.harness.read)("lifecycle.txt")]);
    ran(&turn);
}

fn lifecycle_rm(s: &Session) {
    ok(&s.omh(&["s01", "rm", "--yes", "--json"]), "omh s01 rm");
    assert!(!s.running(), "the container is still running after rm");
    assert!(
        !s.worktree().exists(),
        "the worktree is still there after rm"
    );
    s.git(&["rev-parse", "--verify", "-q", "omh/s01"]);
}
