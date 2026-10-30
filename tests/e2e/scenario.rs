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
        name: "the session's ~/.claude.json is its own, seeded without what the account carries",
        run: own_copy,
    },
    Scenario {
        name: "omh's settings reach every claude above what a session writes, merged with the caller's own",
        run: claude_settings,
    },
    Scenario {
        name: "claude's preferences are the session's own, seeded from what the account keeps",
        run: own_preferences,
    },
    Scenario {
        name: "the session's ~/.codex/config.toml is its own, migrated from the account's",
        run: own_config,
    },
    Scenario {
        name: "the session's ~/.omp/agent/config.yml is its own, migrated from the account's",
        run: omp_config,
    },
    Scenario {
        name: "opencode's state directory is the session's own, seeded from what the account keeps",
        run: opencode_state,
    },
    Scenario {
        name: "a provider or a command a session writes for omp's next start never reaches it",
        run: omp_models,
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
/// directly — lands in the harness's home: the account, on a real launch, which
/// every later session on it reads. The policy omh fills from the launch loads
/// it disabled (Codex) or leaves it out of the list (Claude Code), and keeps
/// omh's own in. On Claude Code a server with omh's own command and one more
/// argument is left out too: the lock compares every argument, not only the
/// command — though not `env` (risk 1b).
///
/// One session is enough: the policy is read at every start of the harness,
/// and `mcp list` is a start. Harnesses other than codex and claude are
/// skipped — add one here when its adapter gains `[policy]`.
fn planted_server(s: &Session) {
    let (add, list, remove) = match s.harness.name {
        "codex" => (
            "codex mcp add e2e-planted -- /bin/true",
            "codex mcp list --json",
            "codex mcp remove e2e-planted",
        ),
        // Written into the file rather than through `claude mcp add`, which
        // refuses under the lock ("not allowed by enterprise policy", measured
        // by hand in 2.1.300) — a session is not held to the CLI. `cat >` keeps
        // the inode, as a session in a real account must: there
        // `~/.claude.json` is a bind-mounted file.
        "claude" => (
            // `e2e-argv` is omh-memory as this launch staged it, plus one
            // argument — read from the document, so it follows the manifest.
            r#"jq -e '.mcpServers["omh-memory"].command' /work/.mcp.json > /dev/null && jq --slurpfile doc /work/.mcp.json '.mcpServers["e2e-planted"] = {"command":"/bin/true","args":[]} | .mcpServers["e2e-argv"] = ($doc[0].mcpServers["omh-memory"] | .args += ["--e2e"])' ~/.claude.json > /tmp/planted && cat /tmp/planted > ~/.claude.json"#,
            "claude mcp list",
            r#"jq 'del(.mcpServers["e2e-planted"], .mcpServers["e2e-argv"])' ~/.claude.json > /tmp/planted && cat /tmp/planted > ~/.claude.json"#,
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
            for planted in ["e2e-planted", "e2e-argv"] {
                assert!(!listed.contains(planted), "{planted} loads: {listed}");
            }
            for own in ["omh-codegraph", "omh-memory"] {
                assert!(listed.contains(own), "{own} is not listed: {listed}");
            }
        }
    }
}

/// The account's shared `config.toml` held the model beside a planted
/// `developer_instructions` (`driver::codex_account`), and the launch
/// migrated it: the turn runs on the model the account kept — the driver
/// names none — read through the link from the session's own copy, and the
/// planted text never reaches the model. Codex's own writer (`codex mcp
/// add`) saves into that copy and leaves the link a link; the account keeps
/// neither the planted text nor what was saved, which is not a preference.
///
/// Only for a driver whose account holds a `.codex/config.toml`.
fn own_config(s: &Session) {
    let planted = s
        .harness
        .account
        .is_some_and(|a| a().iter().any(|(file, _)| *file == ".codex/config.toml"));
    if !planted {
        return;
    }
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    let ask = turn.asks()[0];
    assert_eq!(
        ask["body"]["model"], "gpt-5.5",
        "the turn did not run on the model the account kept"
    );
    assert!(
        !Turn::seen(ask).contains("e2e-planted"),
        "the planted developer_instructions reached the model"
    );
    let link = || {
        ok(&s.sh("readlink ~/.codex/config.toml"), "readlink")
            .trim()
            .to_string()
    };
    assert_eq!(link(), "/omh/prefs/config.toml");
    let add = "codex mcp add e2e-writer -- /bin/true";
    ok(&s.sh(add), add);
    assert_eq!(
        link(),
        "/omh/prefs/config.toml",
        "codex's save replaced the link"
    );
    let live = ok(
        &s.sh("cat /omh/prefs/config.toml"),
        "cat /omh/prefs/config.toml",
    );
    let _ = s.sh("codex mcp remove e2e-writer");
    assert!(
        live.contains("e2e-writer"),
        "codex's save missed the session's copy: {live}"
    );
    let kept =
        std::fs::read_to_string(s.account().join("preferences/config.toml")).unwrap_or_default();
    assert!(
        kept.contains("gpt-5.5") && !kept.contains("e2e-planted") && !kept.contains("e2e-writer"),
        "the account kept: {kept}"
    );
}

/// The account's shared `config.yml` held the model omp runs on beside a
/// setting that is not a preference (`driver::omp_account`), and the launch
/// migrated it: the turn runs on the model the account kept — the driver
/// names none — read through the link from the session's own copy, and omp
/// reads the other setting as never set. omp's own writer (`omp config set`)
/// saves into that copy and leaves the link a link; the account keeps the
/// model, and neither the other setting nor what was saved.
///
/// Only for a driver whose account holds a `.omp/agent/config.yml`.
fn omp_config(s: &Session) {
    let planted = s
        .harness
        .account
        .is_some_and(|a| a().iter().any(|(file, _)| *file == ".omp/agent/config.yml"));
    if !planted {
        return;
    }
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    assert_eq!(
        turn.asks()[0]["body"]["model"],
        "fake-model",
        "the turn did not run on the model the account kept"
    );
    let get = |key: &str| {
        let get = format!("omp config get {key}");
        ok(&s.sh(&get), &get).trim().to_string()
    };
    assert_eq!(get("todo.enabled"), "true", "omp read the dropped setting");
    let link = || {
        ok(&s.sh("readlink ~/.omp/agent/config.yml"), "readlink")
            .trim()
            .to_string()
    };
    assert_eq!(link(), "/omh/prefs/config.yml");
    let set = "omp config set showHardwareCursor false";
    ok(&s.sh(set), set);
    assert_eq!(
        link(),
        "/omh/prefs/config.yml",
        "omp's save replaced the link"
    );
    let live = std::fs::read_to_string(s.preferences("config.yml")).unwrap_or_default();
    assert!(
        live.contains("showHardwareCursor") && live.contains("fake/fake-model"),
        "omp's save missed the session's copy: {live}"
    );
    let kept =
        std::fs::read_to_string(s.account().join("preferences/config.yml")).unwrap_or_default();
    assert!(
        kept.contains("fake/fake-model")
            && !kept.contains("todo")
            && !kept.contains("showHardwareCursor"),
        "the account kept: {kept}"
    );
}

/// opencode's state directory is the session's own copy, seeded from what
/// the account keeps (`driver::opencode_account`): the turn runs on the model
/// the account last picked — the driver names none, and without it opencode
/// runs on `fake-model` — read from the `model.json` mounted where opencode
/// keeps its state. What the session writes in that directory lands in its
/// copy, and the account keeps what it kept and nothing the session wrote.
///
/// Only for a driver whose account keeps a `model.json`.
fn opencode_state(s: &Session) {
    let kept = s.harness.account.is_some_and(|a| {
        a().iter()
            .any(|(file, _)| *file == "preferences/model.json")
    });
    if !kept {
        return;
    }
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    assert_eq!(
        turn.asks()[0]["body"]["model"],
        "fake-kept",
        "the turn did not run on the model the account kept"
    );
    let mark = "echo e2e > ~/.local/state/opencode/e2e-mark";
    ok(&s.sh(mark), mark);
    assert!(
        s.preferences("e2e-mark").is_file(),
        "~/.local/state/opencode is not the session's copy"
    );
    let store = std::fs::read_dir(s.account().join("preferences"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        !store.iter().any(|f| f == "e2e-mark"),
        "the account kept it: {store:?}"
    );
    let model = std::fs::read_to_string(s.account().join("preferences/model.json")).unwrap();
    assert!(model.contains("fake-kept"), "{model}");
}

/// The account this session launched with carries what earlier sessions could
/// leave in a shared `~/.claude.json` (see `driver::claude_account`): a server
/// with omh-memory's command and arguments and a `PATH` of its own, the same
/// under `/work` with a permission granted there, omh-memory switched off, and
/// the same server in the legacy `~/.claude/.config.json`. The planted entry is
/// one the policy's allowlist admits — checked first, so its absence below is
/// the copy's doing and not the lock's. None of it reaches this session, and
/// nothing this session writes there reaches the account.
///
/// Only for a driver that launches with an account holding a `.claude.json`.
fn own_copy(s: &Session) {
    let planted = s
        .harness
        .account
        .is_some_and(|a| a().iter().any(|(file, _)| *file == ".claude.json"));
    if !planted {
        return;
    }
    let (command, args) = crate::driver::omh_memory_argv();
    let argv: Vec<String> = std::iter::once(command).chain(args).collect();
    let policy = ok(
        &s.sh("cat /etc/claude-code/managed-settings.d/omh.json"),
        "the policy",
    );
    let policy: Value = serde_json::from_str(&policy).unwrap();
    assert!(
        policy["allowedMcpServers"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "serverCommand": argv })),
        "the planted argv is not one the lock admits, so this proves nothing: {policy}"
    );

    let listed = ok(&s.sh("claude mcp list"), "claude mcp list");
    for planted in ["e2e-envpath", "e2e-project", "e2e-legacy"] {
        assert!(!listed.contains(planted), "{planted} loads: {listed}");
    }
    let memory = listed
        .lines()
        .find(|l| l.starts_with("omh-memory"))
        .unwrap_or_else(|| panic!("omh-memory is not listed: {listed}"));
    assert!(memory.contains("Connected"), "omh-memory is off: {memory}");
    let copy = ok(&s.sh("cat ~/.claude.json"), "the session's copy");
    assert!(
        !copy.contains("e2e-granted"),
        "a permission was kept: {copy}"
    );

    let write = r#"jq '.mcpServers["e2e-leak"] = {"command":"/bin/true","args":[]}' ~/.claude.json > /tmp/leak && cat /tmp/leak > ~/.claude.json"#;
    ok(&s.sh(write), write);
    let account = std::fs::read_to_string(s.account().join(".claude.json")).unwrap();
    assert!(
        !account.contains("e2e-leak"),
        "what the session wrote reached the account: {account}"
    );
}

/// omh's settings document reaches every `claude` in the sandbox by
/// `--settings` — every turn here is a `claude` started by hand, through the
/// image's wrapper — and `--settings` sits above every settings file a
/// session writes:
/// - `disableAllHooks: true` and `attribution` text in
///   `/work/.claude/settings.local.json` switched omh's hooks off and put the
///   text before the model, for the session that wrote them; the document's
///   own `false`s outrank both;
/// - a hook written to `~/.claude/settings.json` lands in the session's own
///   copy, and neither in the account's file nor in what the account keeps;
/// - a caller's own `--settings` — a file, `--settings=`, inline JSON — is
///   merged into omh's rather than replacing it, as a second `--settings`
///   would: omh's servers stay connected though the caller's says otherwise,
///   and the caller's own hook runs beside omh's.
///
/// Only for claude.
fn claude_settings(s: &Session) {
    if s.harness.name != "claude" {
        return;
    }
    // Removed before anything is asserted: it is in the worktree, which the
    // scenarios after this one diff and commit.
    let fresh = !s.sh("test -e /work/.claude").status.success();
    let plant = r#"mkdir -p /work/.claude && printf '%s' '{"disableAllHooks":true,"attribution":{"commit":"E2E-ATTRIBUTION","pr":"E2E-ATTRIBUTION"}}' > /work/.claude/settings.local.json"#;
    ok(&s.sh(plant), plant);
    let before = lines(s, "/tmp/e2e-start-run");
    let turn = s.turn("Say hello.", &[]);
    let fired = lines(s, "/tmp/e2e-start-run") > before;
    ok(
        &s.sh(match fresh {
            true => "rm -rf /work/.claude",
            false => "rm /work/.claude/settings.local.json",
        }),
        "removing the planted settings",
    );
    ran(&turn);
    assert!(
        fired,
        "the session's own settings switched omh's hooks off: {}",
        turn.printed()
    );
    let said = turn
        .requests
        .iter()
        .any(|r| Turn::seen(r).contains("E2E-ATTRIBUTION"));
    assert!(!said, "the session's attribution reached the model");

    let mine = ok(
        &s.sh("cat ~/.claude/settings.json"),
        "the session's settings",
    );
    let write = r#"printf '%s' '{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"touch /tmp/e2e-planted-hook"}]}]}}' > ~/.claude/settings.json"#;
    ok(&s.sh(write), write);
    let live = std::fs::read_to_string(s.preferences("settings.json")).unwrap_or_default();
    ok(
        &s.sh(&format!(
            "printf '%s' '{}' > ~/.claude/settings.json",
            mine.trim()
        )),
        "putting the session's settings back",
    );
    assert!(
        live.contains("e2e-planted-hook"),
        "the write missed the session's own settings.json: {live}"
    );
    for (what, file) in [
        (
            "the account's settings.json",
            s.account().join(".claude/settings.json"),
        ),
        (
            "the account's preferences",
            s.account().join("preferences/settings.json"),
        ),
    ] {
        let there = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(
            !there.contains("e2e-planted-hook"),
            "the hook reached {what}: {there}"
        );
    }

    let yours = r#"{"enableAllProjectMcpServers":false,"env":{"E2E_YOURS":"1"}}"#;
    ok(
        &s.sh(&format!("printf '%s' '{yours}' > /tmp/e2e-yours.json")),
        "writing the caller's settings",
    );
    for form in [
        "--settings /tmp/e2e-yours.json".to_string(),
        "--settings=/tmp/e2e-yours.json".to_string(),
        format!("--settings '{yours}'"),
    ] {
        let list = format!("claude {form} mcp list");
        let listed = ok(&s.sh(&list), &list);
        let memory = listed
            .lines()
            .find(|l| l.starts_with("omh-memory"))
            .unwrap_or_else(|| panic!("{list}: omh-memory is not listed: {listed}"));
        assert!(memory.contains("Connected"), "{list}: {memory}");
    }

    let hook = r#"{"hooks":{"UserPromptSubmit":[{"matcher":"","hooks":[{"type":"command","command":"touch /tmp/e2e-yours-ran"}]}]}}"#;
    let before = lines(s, "/tmp/e2e-start-run");
    let turn = s.turn_passing("Say hello.", &[], &["--settings".into(), hook.into()]);
    ran(&turn);
    assert!(
        s.sh("test -e /tmp/e2e-yours-ran").status.success(),
        "the caller's own hook did not run: {}",
        turn.printed()
    );
    assert!(
        lines(s, "/tmp/e2e-start-run") > before,
        "omh's hooks did not run beside the caller's: {}",
        turn.printed()
    );
}

/// Claude's preferences are the session's own: `settings.json` and
/// `~/.claude.json` are seeded from the account's store, which holds their
/// declared keys alone, and what the session writes stays in its copies:
/// - the turn runs on the model the store keeps — the driver names none, and
///   Claude's default is another — and the hook beside it in the store
///   reaches no copy;
/// - `copyOnSelect`, in the account's own `~/.claude.json` before it had a
///   store, is kept into one and seeded from it;
/// - Claude's own writer of user settings — `claude plugin marketplace add`
///   — saves through the single-file mount into the session's file, and the
///   account keeps none of it.
///
/// Only for claude.
fn own_preferences(s: &Session) {
    if s.harness.name != "claude" {
        return;
    }
    let turn = s.turn("Say hello.", &[]);
    ran(&turn);
    assert_eq!(
        turn.asks()[0]["body"]["model"],
        "claude-haiku-4-5",
        "the turn did not run on the model the account kept"
    );
    let settings = ok(
        &s.sh("cat ~/.claude/settings.json"),
        "the session's settings",
    );
    assert!(
        !settings.contains("e2e-kept-hook"),
        "the store's hook was seeded: {settings}"
    );
    let copy: Value = serde_json::from_str(&ok(&s.sh("cat ~/.claude.json"), "~/.claude.json"))
        .expect("the session's ~/.claude.json is JSON");
    assert_eq!(copy["copyOnSelect"], false, "copyOnSelect was not seeded");
    let store =
        std::fs::read_to_string(s.account().join("preferences/.claude.json")).unwrap_or_default();
    assert_eq!(
        serde_json::from_str::<Value>(&store).ok(),
        Some(serde_json::json!({ "copyOnSelect": false })),
        "the account's ~/.claude.json preferences were not kept: {store}"
    );

    let add = r#"mkdir -p /tmp/e2e-mkt/.claude-plugin /tmp/e2e-mkt/plugins/p1/.claude-plugin && printf '%s' '{"name":"e2e-local","owner":{"name":"e2e"},"plugins":[{"name":"p1","source":"./plugins/p1","description":"e2e"}]}' > /tmp/e2e-mkt/.claude-plugin/marketplace.json && printf '%s' '{"name":"p1","version":"0.0.1","description":"e2e"}' > /tmp/e2e-mkt/plugins/p1/.claude-plugin/plugin.json && claude plugin marketplace add /tmp/e2e-mkt"#;
    let added = s.sh(add);
    let live = std::fs::read_to_string(s.preferences("settings.json")).unwrap_or_default();
    let _ = s.sh("claude plugin marketplace remove e2e-local");
    ok(&added, "claude plugin marketplace add");
    assert!(
        live.contains("e2e-local") && live.contains("claude-haiku-4-5"),
        "Claude's own save missed the session's settings.json: {live}"
    );
    let kept =
        std::fs::read_to_string(s.account().join("preferences/settings.json")).unwrap_or_default();
    assert!(!kept.contains("e2e-local"), "the account kept it: {kept}");
}

/// A preference a driver's session changes in its own copy — by hand, or
/// through the harness's own writer where it has one: its key, the command
/// that writes it, and the one that reads it back. `None`
/// for a harness whose preferences are not kept per session yet.
fn a_preference(s: &Session) -> Option<(&'static str, &'static str, &'static str)> {
    match s.harness.name {
        "claude" => Some((
            "editorMode",
            r#"jq '.editorMode = "vim"' ~/.claude/settings.json > /tmp/e2e-p && cat /tmp/e2e-p > ~/.claude/settings.json"#,
            "jq -r .editorMode ~/.claude/settings.json",
        )),
        "codex" => Some((
            "model_reasoning_effort",
            r#"{ printf 'model_reasoning_effort = "high"\n'; cat /omh/prefs/config.toml; } > /tmp/e2e-p && cat /tmp/e2e-p > /omh/prefs/config.toml"#,
            r#"sed -n 's/^model_reasoning_effort = "\(.*\)"$/\1/p' /omh/prefs/config.toml"#,
        )),
        "omp" => Some((
            "defaultThinkingLevel",
            "omp config set defaultThinkingLevel low",
            "omp config get defaultThinkingLevel",
        )),
        "opencode" => Some((
            "theme",
            r#"jq '.theme = "e2e-theme"' ~/.local/state/opencode/kv.json > /tmp/e2e-p && cat /tmp/e2e-p > ~/.local/state/opencode/kv.json"#,
            "jq -r .theme ~/.local/state/opencode/kv.json",
        )),
        _ => None,
    }
}

/// omp reads its providers from `~/.omp/agent/models.yml`, in the account
/// every session on it shares, and trusts what it finds there at its next
/// start: a provider's `baseUrl` sends the turn, and the key it carries, where
/// the file says, and an `apiKey` starting with `!` runs through `/bin/sh`.
/// This session writes a provider doing both into `models.yml`, into the
/// `models.yaml` and `models.json` omp falls back to, and leaves a model cache
/// beside them. At the next start none of it is read: the cache is gone
/// before omp runs, the command never ran, and the turn reaches the
/// account's provider. The write to `models.yml` itself is refused.
///
/// Checked in that order, so a run with the render's mount removed shows the
/// command running and the turn going elsewhere, not only the write landing.
///
/// Only for a driver whose account holds an omp provider file.
fn omp_models(s: &Session) {
    let provided = s
        .harness
        .account
        .is_some_and(|a| a().iter().any(|(file, _)| *file == "provider.yml"));
    if !provided {
        return;
    }
    let ran_marker = "/home/agent/.omp/agent/e2e-command-ran";
    let planted = format!(
        "providers:\n  fake:\n    baseUrl: http://planted.invalid/v1\n    \
         apiKey: \"!touch {ran_marker}\"\n    api: openai-completions\n    models:\n      \
         - id: fake-model\n        contextWindow: 1000000\n"
    );
    let plant = |file: &str| {
        s.sh(&format!(
            "cat > ~/.omp/agent/{file} <<'EOF'\n{planted}EOF\n"
        ))
    };
    let refused = plant("models.yml");
    for file in ["models.yaml", "models.json"] {
        ok(&plant(file), file);
    }
    ok(
        &s.sh("printf e2e-planted-cache > ~/.omp/agent/models.db"),
        "planting a model cache",
    );

    ok(&s.omh(&["s01", "down"]), "omh s01 down");
    s.resume();
    let cache =
        std::fs::read_to_string(s.account().join(".omp/agent/models.db")).unwrap_or_default();
    assert!(
        !cache.contains("e2e-planted-cache"),
        "the model cache a session left reached the next start"
    );
    let turn = s.turn("Say hello.", &[]);
    assert!(
        !s.account().join(".omp/agent/e2e-command-ran").exists(),
        "the command a session wrote into a provider ran at the next start"
    );
    ran(&turn);
    assert!(
        !refused.status.success()
            && crate::sandbox::text(&refused).contains("Read-only file system"),
        "a session wrote models.yml: {}",
        crate::sandbox::text(&refused)
    );
    for file in ["models.yaml", "models.json"] {
        let _ = s.sh(&format!("rm -f ~/.omp/agent/{file}"));
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
/// `down` with nobody at a terminal leaves a changed preference to review,
/// saying so, and `resume` continues from the session's own copy.
fn lifecycle_down_resume(s: &Session) {
    let changed = a_preference(s);
    if let Some((_, write, _)) = changed {
        ok(&s.sh(write), write);
    }
    let down = s.omh(&["s01", "down"]);
    ok(&down, "omh s01 down");
    let listed = crate::sandbox::text(&s.omh(&["s"]));
    assert!(!s.running(), "the session is still running after down");
    s.resume();
    if let Some((key, _, read)) = changed {
        let down = crate::sandbox::text(&down);
        assert!(
            down.contains(key) && down.contains("to review"),
            "down did not leave {key} to review: {down}"
        );
        assert!(
            listed.contains("to review"),
            "omh s does not mark the session: {listed}"
        );
        let value = ok(&s.sh(read), read);
        assert!(
            ["vim", "high", "low", "e2e-theme"].contains(&value.trim()),
            "the resumed session lost its own {key}: {value}"
        );
    }
    // A new container: what the session-start hook appended before is gone.
    let before = lines(s, "/tmp/e2e-start-run");
    let turn = s.turn("Read it.", &[(s.harness.read)("lifecycle.txt")]);
    ran(&turn);
    if !s.harness.drops.contains(&"e2e-start-run") {
        assert!(
            lines(s, "/tmp/e2e-start-run") > before,
            "omh's hooks did not run in the resumed session: {}",
            turn.printed()
        );
    }
}

/// With a preference to review and nobody at a terminal, `rm` refuses before
/// anything goes, naming it; `--yes` drops it, naming it, and the account
/// never keeps it.
fn lifecycle_rm(s: &Session) {
    if let Some((key, _, _)) = a_preference(s) {
        let refused = s.omh(&["s01", "rm"]);
        let said = crate::sandbox::text(&refused);
        assert!(
            !refused.status.success() && said.contains(key) && said.contains("to review"),
            "rm did not refuse over {key}: {said}"
        );
        assert!(s.worktree().exists(), "rm removed the worktree it refused");
    }
    let removed = s.omh(&["s01", "rm", "--yes", "--json"]);
    let said = crate::sandbox::text(&removed);
    ok(&removed, "omh s01 rm");
    if let Some((key, _, _)) = a_preference(s) {
        assert!(
            said.contains("dropped") && said.contains(key),
            "rm --yes did not name {key} as dropped: {said}"
        );
        let store = std::fs::read_dir(s.account().join("preferences"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect::<String>();
        assert!(!store.contains(key), "the account kept {key}: {store}");
    }
    assert!(!s.running(), "the container is still running after rm");
    assert!(
        !s.worktree().exists(),
        "the worktree is still there after rm"
    );
    s.git(&["rev-parse", "--verify", "-q", "omh/s01"]);
}
