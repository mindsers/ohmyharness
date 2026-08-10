//! What the commands do, rather than what the functions under them return.
//!
//! Every guard in `memory.rs` is a pure function with a unit test, and that is
//! most of the value — but it is not the part a user meets. Twice now a guard
//! has been correct while the wiring that reaches it was missing or wrong, and
//! the suite stayed green both times: deleting `lint`'s whole exit-code block,
//! or `init`'s call to stage the note rules, changed nothing any test could
//! see. Those are the two failures this file exists to notice.
//!
//! Driven through the built binary, because an exit code is not observable
//! from inside the process that would have produced it, and `Paths` reads
//! `$HOME` — which a subprocess can own and an in-process test cannot.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A repo and a home, isolated from the developer's own.
///
/// `repo_root` only looks for a `.git` directory, so an empty one is a repo
/// as far as omh is concerned — no `git init`, and no dependency on git being
/// installed to run the tests.
struct Sandbox {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Sandbox {
        _dir: dir,
        repo,
        home,
    }
}

impl Sandbox {
    fn omh(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_omh"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .output()
            .expect("the binary under test must run")
    }

    fn local_store(&self) -> PathBuf {
        self.home
            .join(".omh/notes")
            .join(self.repo.file_name().unwrap())
            .join("local")
    }

    fn seed(&self, at: &str, body: &str) {
        let path = self.local_store().join(at);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

fn note(key: &str, body: &str) -> String {
    format!(
        "---\nkey: {key}\ntype: surprise\nsource: audit\nrecorded: 2026-08-10\n---\n\n# T\n\n{body}"
    )
}

const WHOLE: &str = "## Expected\na\n\n## Observed\nb\n\n## Evidence\nc\n";

/// §14 makes this exit code M1's entire stand-in for the refused write the
/// agent does not get yet. A gate that cannot fail gates nothing: no hook, no
/// CI step and no `&&` can read it.
#[test]
fn lint_fails_the_command_when_the_schema_refused_something() {
    let sb = sandbox();
    sb.seed("broken.md", &note("broken", "## Expected\na\n"));

    let out = sb.omh(&["memory", "lint"]);
    assert!(
        !out.status.success(),
        "a store with refusals must fail the command"
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("refused"),
        "the report is the product and prints before the exit code: {printed}"
    );
}

/// The other half, and the reason the gate reads severity rather than
/// counting: `Orphan` fires on every note nothing links to, which is every
/// note `remember` writes without `--relates-to`. A gate that tripped on
/// those would be red for every real store.
#[test]
fn lint_passes_a_store_that_only_has_warnings() {
    let sb = sandbox();
    sb.seed("fine.md", &note("fine", WHOLE));

    let out = sb.omh(&["memory", "lint"]);
    assert!(
        out.status.success(),
        "warnings must not fail the command: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("warning"));
}

/// With no MCP surface in M1, `.omh/profile/AGENTS.md` is the only thing that
/// tells the agent the store exists — so a repo that ran `init` before the
/// store shipped got the feature and no way to know about it.
#[test]
fn init_delivers_the_note_rules_to_a_repo_that_already_had_agents_md() {
    let sb = sandbox();
    let agents = sb.repo.join(".omh/profile/AGENTS.md");
    std::fs::create_dir_all(agents.parent().unwrap()).unwrap();
    let human = "# Project rules\n\n## House style\n\nTabs, and no adverbs.\n";
    std::fs::write(&agents, human).unwrap();

    assert!(sb.omh(&["init"]).status.success());

    let body = std::fs::read_to_string(&agents).unwrap();
    assert!(body.starts_with(human), "a human's file must survive whole");
    assert!(body.contains("## Memory"), "the rules must arrive");
}

/// `--at` exists to reach one of two notes that share a key. Naming a file
/// that holds neither must never fall through to deleting one of them.
#[test]
fn rm_refuses_an_at_that_names_no_note() {
    let sb = sandbox();
    sb.seed("solo.md", &note("solo", WHOLE));

    let out = sb.omh(&["memory", "rm", "solo", "--at", "elsewhere.md"]);
    assert!(!out.status.success());
    assert!(
        sb.local_store().join("solo.md").exists(),
        "a note the caller did not name was removed"
    );
}

/// The escape this store's guards exist for, end to end: a key template is a
/// committed file, so a clone carries it.
#[test]
fn remember_refuses_a_key_template_that_leaves_the_store() {
    let sb = sandbox();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/keys.toml"),
        "[keys]\nsurprise = \"../../escaped/{{slug}}\"\ntopic = \"{{slug}}\"\nstub = \"docs/{{path}}\"\n",
    )
    .unwrap();

    let out = sb.omh(&[
        "memory",
        "remember",
        "--expected",
        "a",
        "--observed",
        "the mount failed",
        "--evidence",
        "c",
    ]);
    assert!(!out.status.success());
    assert!(
        !escaped_notes(sb.home.parent().unwrap()),
        "a note was written outside the store"
    );
}

fn escaped_notes(under: &Path) -> bool {
    let mut stack = vec![under.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md")
                && !path.components().any(|c| c.as_os_str() == "notes")
            {
                return true;
            }
        }
    }
    false
}
