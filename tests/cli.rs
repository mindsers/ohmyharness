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
/// `repo_root` only looks for a `.git` directory, so an empty one is a repo as
/// far as omh is concerned, and most of this file needs no `git init` and no
/// git on the box. `promote` is the exception — it asks git whether the
/// destination is ignored and refuses to guess — so those tests call
/// `git_init` and do depend on git being installed.
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

    /// A real repository, for the commands that ask git a question rather than
    /// just needing somewhere to be. `promote` is the only one: it will not
    /// plan against a destination it cannot establish the ignore status of,
    /// and the empty `.git` above is exactly the case git refuses to answer
    /// about — so a promotion in the bare sandbox is correctly always blocked.
    fn git_init(&self) {
        std::fs::remove_dir_all(self.repo.join(".git")).unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["init", "-q", "-b", "main"])
            .output()
            .expect("git must be installed to run this test");
        assert!(out.status.success(), "git init failed");
    }

    fn team_store(&self) -> PathBuf {
        self.repo.join(".omh/notes")
    }
}

fn note(key: &str, body: &str) -> String {
    format!(
        "---\nkey: {key}\ntype: surprise\nsource: audit\nrecorded: 2026-08-10\n---\n\n# T\n\n{body}"
    )
}

/// A note the schema has nothing to refuse, so what `lint` reports about it is
/// warnings and only warnings. Every required `surprise` section is here —
/// including `## Answers`, without which this fixture would be testing a
/// refusal rather than the warning it is named for.
const WHOLE: &str =
    "## Expected\na\n\n## Observed\nb\n\n## Evidence\nc\n\n## Answers\n\n- what happens here\n";

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
///
/// Ignored by default because `init` builds the base image, and this is the
/// only test in the suite that needs a container runtime. Marking it beats
/// excluding whole test targets per platform: the run *reports* it as ignored,
/// and a new integration test is included everywhere by default instead of
/// silently dropping off macOS. CI runs it with `-- --include-ignored`.
#[test]
#[ignore = "needs a container runtime; run with --include-ignored"]
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

/// `promote` is the one command whose failure must not be quiet: it is the
/// human gate, and a gate that reports a refusal only on stdout — or exits 0
/// having refused — is a gate somebody scripts straight past. Nothing under
/// `plan` can observe either, because both live in `main`.
#[test]
fn promote_fails_the_command_and_moves_nothing_when_a_key_is_blocked() {
    let sb = sandbox();
    sb.git_init();
    sb.seed("private.md", &note("private", WHOLE));
    sb.seed(
        "candidate.md",
        &note(
            "candidate",
            &format!("{WHOLE}\n## Related\n\n- [[private]]\n"),
        ),
    );

    let out = sb.omh(&["memory", "promote", "candidate"]);
    assert!(
        !out.status.success(),
        "a refused promotion must fail the command"
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("private"),
        "the blocker names what to fix, on stderr: {said}"
    );
    assert!(
        sb.local_store().join("candidate.md").exists(),
        "and the note is still in the gitignored layer"
    );
    assert!(
        !sb.team_store().join("candidate.md").exists(),
        "and nothing was committed-layer written"
    );
}

/// The other half. Without it the test above passes on a `promote` that
/// refuses everything, which is the failure mode a fail-closed ignore check
/// makes easy to ship.
#[test]
fn promote_moves_the_note_and_says_it_is_not_shared_yet() {
    let sb = sandbox();
    sb.git_init();
    sb.seed("fine.md", &note("fine", WHOLE));

    let out = sb.omh(&["memory", "promote", "fine"]);
    assert!(
        out.status.success(),
        "a clean note promotes: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("not shared until committed"),
        "moving the file is not sharing it: {printed}"
    );
    assert!(
        sb.team_store().join("fine.md").exists(),
        "the note is in the committed layer"
    );
    assert!(
        !sb.local_store().join("fine.md").exists(),
        "and no longer in the gitignored one"
    );
}

/// The hash git would record for a file, so a fixture can pin the real thing
/// rather than a value that is stale by construction.
fn hash_object(repo: &Path, rel: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", "--"])
        .arg(rel)
        .output()
        .expect("git must be installed to run this test");
    assert!(out.status.success(), "git hash-object failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn note_expiring(key: &str, trigger: &str) -> String {
    format!(
        "---\nkey: {key}\ntype: surprise\nsource: audit\nrecorded: 2026-08-10\n\
         invalidated_by: {trigger}\n---\n\n# T\n\n{WHOLE}"
    )
}

/// **`stale` said nothing at the only boundary a script reads.** Four notes
/// stale exited 0; git missing so that not one probe could be answered exited
/// 0; an empty store exited 0. `lint` in the same file has bothered to bail
/// since M1, and CI cannot tell "the store is clean" from "omh checked
/// nothing".
#[test]
fn stale_fails_the_command_when_a_note_is_out_of_date() {
    let sb = sandbox();
    sb.git_init();
    std::fs::write(sb.repo.join("t.txt"), "before\n").unwrap();
    sb.seed("pinned.md", &note_expiring("pinned", "file:t.txt@0000000"));

    let out = sb.omh(&["memory", "stale"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a stale store must fail: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("stale"),
        "the report is the product: {printed}"
    );
}

/// The other half, or the test above passes on a `stale` that always fails.
#[test]
fn stale_exits_zero_when_every_note_is_current() {
    let sb = sandbox();
    sb.git_init();
    std::fs::write(sb.repo.join("t.txt"), "before\n").unwrap();
    let real = hash_object(&sb.repo, "t.txt");
    sb.seed(
        "pinned.md",
        &note_expiring("pinned", &format!("file:t.txt@{real}")),
    );

    let out = sb.omh(&["memory", "stale"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "nothing is stale: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **"omh cannot tell" is not "fine".** Folding it into 0 is the same lie the
/// `Unknown` verdict exists to refuse, arriving one layer later — a scripted
/// caller reads the code, not the prose.
#[test]
fn stale_reports_a_separate_code_when_it_cannot_tell() {
    let sb = sandbox();
    sb.git_init();
    // `symbol:` is unanswerable from the host by design.
    sb.seed("sym.md", &note_expiring("sym", "symbol:GUEST_HOME"));

    let out = sb.omh(&["memory", "stale"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "cannot-tell has its own code: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The grouping is the last hop, and the heading a note lands under is the
/// whole claim. Swapping the two headings, or filing `Unknown` under `stale`,
/// kept the suite green while `contributing.md` listed the opposite as guarded.
#[test]
fn stale_never_files_what_it_cannot_tell_under_stale() {
    let sb = sandbox();
    sb.git_init();
    sb.seed("sym.md", &note_expiring("sym", "symbol:GUEST_HOME"));

    let printed = String::from_utf8_lossy(&sb.omh(&["memory", "stale"]).stdout).to_string();
    let cannot = printed.find("omh cannot tell").expect(&printed);
    let key = printed.find("sym").expect(&printed);
    assert!(
        printed.find("stale:").is_none(),
        "nothing is known to be stale here: {printed}"
    );
    assert!(
        key > cannot,
        "the note belongs under that heading: {printed}"
    );
}

// ── getting work out of a session ───────────────────────────────────────────

impl Sandbox {
    /// A session as omh would have left one: a real worktree on `omh/<id>`.
    ///
    /// Built with plain git rather than by launching a container, because what
    /// these tests are about is the host-side path out of a session — the half
    /// that has to work whether or not a sandbox is running.
    fn session(&self, id: &str) -> PathBuf {
        self.git_init();
        let origin = self._dir.path().join("origin.git");
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&origin)
            .output()
            .expect("git must be installed to run this test");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .args(args)
                .output()
                .expect("git must be installed to run this test");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "root"]);
        git(&["remote", "add", "origin", origin.to_str().unwrap()]);

        let worktree = self
            .home
            .join(".omh/worktrees")
            .join(self.repo.file_name().unwrap())
            .join(id);
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(&[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            &format!("omh/{id}"),
        ]);
        worktree
    }
}

/// `pick` invents the next id when none exists, which is right for a launch
/// about to create that worktree and wrong here. Reaching for it would make
/// this fail somewhere further down, about a path nobody named.
#[test]
fn committing_with_no_session_says_so_rather_than_inventing_one() {
    let sb = sandbox();

    let out = sb.omh(&["s", "commit", "-m", "anything"]);

    assert!(!out.status.success(), "there is nothing to commit to");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no sessions"), "got: {err}");
}

/// `s diff` compares `base...branch` and so sees only commits, which means
/// nothing a session did is visible until something commits it. This is that
/// pair, end to end.
#[test]
fn work_committed_from_the_host_is_what_diff_then_reports() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn main() {}").unwrap();

    let out = sb.omh(&["s", "commit", "-m", "Add the feature"]);
    assert!(
        out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let printed = String::from_utf8_lossy(&sb.omh(&["s", "diff", "s01"]).stdout).to_string();
    assert!(printed.contains("feature.rs"), "got: {printed}");
}

/// A session id is a path component and `Session::new` joins it into the
/// worktree path. `s rm` already validates; so must anything else that takes
/// one from the command line.
///
/// Asserting the *reason*, not just the failure: a missing worktree fails this
/// too, so a bare `!success` here stays green with the validation deleted —
/// confirmed by deleting it.
#[test]
fn a_session_id_that_is_a_path_is_refused() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["-s", "../escape", "s", "commit", "-m", "x"]);

    assert!(!out.status.success(), "`../escape` is not a session id");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("not a path"),
        "refused for the wrong reason: {err}"
    );
}

/// A committed session with no upstream has everything to push and nothing to
/// compare against. Without the base-branch fallback that prints a blank —
/// indistinguishable from a session nobody has touched — so measuring against
/// the base is what makes "never report work as clean" true in the state the
/// loop passes through every time.
#[test]
fn a_session_that_has_committed_but_never_pushed_is_not_reported_as_clean() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn main() {}").unwrap();
    assert!(sb
        .omh(&["s", "commit", "-m", "Add the feature"])
        .status
        .success());

    let printed = String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stdout).to_string();

    assert!(printed.contains("to push"), "got: {printed}");
}

/// `s ls` is where every one of these measurements is actually read, and the
/// rendering is the part no unit test reaches. Each state is one the loop sits
/// in, not one it passes through, so a blank column is a wrong answer rather
/// than a missing one.
#[test]
fn s_ls_renders_each_state_a_session_can_sit_in() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    let ls = || String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stdout).to_string();

    std::fs::write(worktree.join("a.rs"), "fn a() {}").unwrap();
    assert!(ls().contains("1 uncommitted"), "got: {}", ls());

    assert!(sb.omh(&["s", "commit", "-m", "Add a"]).status.success());
    assert!(ls().contains("1 to push"), "got: {}", ls());

    assert!(sb.omh(&["s", "push", "feat/a"]).status.success());
    assert!(ls().contains("→ feat/a"), "got: {}", ls());
}

/// The worktree's `.git` is a pointer at an absolute path, and a checkout that
/// moves leaves it dangling — a state `Session::remove` already treats as real.
/// Every accessor then fails, and defaulting them to zero renders a session
/// holding a day of work as clean, which is what leads someone to `s rm` it.
#[test]
fn a_session_omh_cannot_read_is_never_rendered_as_clean() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("a.rs"), "fn a() {}").unwrap();
    // Break the pointer the way a moved or re-cloned checkout would.
    std::fs::write(worktree.join(".git"), "gitdir: /nowhere/that/exists").unwrap();

    let printed = String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stdout).to_string();

    assert!(
        printed.contains("s01"),
        "the session is still listed: {printed}"
    );
    assert!(
        printed.contains('?'),
        "omh cannot tell, and must say so rather than imply clean: {printed}"
    );
}

/// `omh s push <name>` has to carry the name through the CLI, and `--pr` has to
/// treat `gh`'s exit code as the answer it is. Both are wiring no unit test on
/// `Session::push` can reach.
#[test]
fn the_push_command_carries_its_name_and_refuses_without_one() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("a.rs"), "fn a() {}").unwrap();
    assert!(sb.omh(&["s", "commit", "-m", "Add a"]).status.success());

    let bare = sb.omh(&["s", "push"]);
    assert!(!bare.status.success(), "a session id is not a branch name");
    assert!(String::from_utf8_lossy(&bare.stderr).contains("not a branch name"));

    assert!(sb.omh(&["s", "push", "feat/a"]).status.success());
    let printed = String::from_utf8_lossy(&sb.omh(&["s", "push", "feat/a"]).stdout).to_string();
    assert!(printed.contains("origin/feat/a"), "got: {printed}");
}

/// `existing_session` refuses an id with no worktree so the failure names the
/// session rather than arriving from inside git, about a path nobody chose.
#[test]
fn a_session_that_does_not_exist_is_named_in_the_refusal() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["-s", "s99", "s", "commit", "-m", "x"]);

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("s99"), "the refusal must name it: {err}");
}
