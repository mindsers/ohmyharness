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
    /// Prepended to `PATH`, so a test can put a recording `docker` in front of
    /// the developer's real one.
    bin: PathBuf,
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    Sandbox {
        _dir: dir,
        repo,
        home,
        bin,
    }
}

impl Sandbox {
    fn omh(&self, args: &[&str]) -> Output {
        let path = match std::env::var("PATH") {
            Ok(rest) => format!("{}:{rest}", self.bin.display()),
            Err(_) => self.bin.display().to_string(),
        };
        Command::new(env!("CARGO_BIN_EXE_omh"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("PATH", path)
            .output()
            .expect("the binary under test must run")
    }

    /// A `docker` that records every invocation and claims the session
    /// container is up. Returns the log path.
    ///
    /// Which runtime gets picked is pinned too: `auto` prefers `sbx`, so on a
    /// box that has one this shim would never be consulted and the test would
    /// pass by not looking.
    fn fake_docker(&self) -> PathBuf {
        let log = self.bin.join("docker.log");
        let shim = self.bin.join("docker");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n\
                 if [ \"$1\" = inspect ]; then echo true; fi\n\
                 if [ \"$1\" = ps ]; then cat {} 2>/dev/null; fi\nexit 0\n",
                log.display(),
                self.bin.join("containers").display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::create_dir_all(self.repo.join(".omh")).unwrap();
        std::fs::write(
            self.repo.join(".omh/settings.toml"),
            "runtime = \"docker\"\n",
        )
        .unwrap();
        log
    }

    fn docker_calls(&self, log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Put the shipped base manifest where `Paths::base()` looks.
    ///
    /// `omh init` would do it, and needs a container runtime to finish — so the
    /// commands that only read the manifest get it this way instead, and stay
    /// runnable on a box with no docker.
    fn seed_base(&self) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("base");
        let dst = self.home.join(".omh/base");
        std::fs::create_dir_all(&dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }

    fn settings(&self) -> String {
        std::fs::read_to_string(self.repo.join(".omh/settings.toml")).unwrap_or_default()
    }

    fn catalogue(&self, entries: &[&str]) {
        for entry in entries {
            let p = self.home.join(".omh").join(entry);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
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
        sb.repo.join(".omh/memory.toml"),
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

/// The launcher discloses this repo's hooks, and a dry run leaves no trace.
///
/// `notice::hooks` and `Record::commit` are both well covered by unit tests,
/// and the wire between them and `run()` is not: deleting the `say_hooks` call
/// entirely, or committing the snapshot on a dry run, leaves the whole suite
/// green. That is the failure this file's module doc says it exists to notice,
/// and it is the same shape as `own.mcp_env = settings.mcp_env` was.
///
/// The snapshot's *absence* is what makes the second half checkable without a
/// container: a dry run that recorded would spend the one call-out about
/// somebody else's executable content changing under you, and the next real
/// launch would be silent.
///
/// `#[ignore]`d because it needs git and a container runtime to reach `run()`.
/// CI's linux job runs `--include-ignored`, which is where this bites.
#[test]
#[ignore]
fn a_dry_run_discloses_the_repos_hooks_and_records_nothing() {
    let sb = sandbox();
    sb.git_init();
    std::fs::write(sb.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );
    std::fs::write(
        sb.repo.join(".omh/hooks/rust-test.json"),
        r#"{ "on": "turn-end", "run": "cargo test" }"#,
    )
    .unwrap();

    let out = sb.omh(&["--dry-run", "claude"]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("this repo's hooks") && said.contains("rust-test"),
        "a launch has to name the executable content it was handed: {said}"
    );

    let snapshot = sb
        .home
        .join(".omh/run")
        .join(sb.repo.file_name().unwrap())
        .join("hooks.json");
    assert!(
        !snapshot.exists(),
        "a dry run recorded {} — the next real launch would be silent about a change",
        snapshot.display()
    );
}

/// The launcher says what this repo is *not* using from your catalogue.
///
/// Same wire, same gap: `notice::selection` and `Selection::unselected` are both
/// covered, and deleting the `say_selection` call leaves every one of those
/// tests green. It is the report that makes an expanded `[use]` safe — `init`
/// writes the list once and never revisits it, so without this a skill added
/// afterwards is off and nothing about the repo says why.
///
/// `#[ignore]`d because it needs git and a container runtime to reach `run()`.
/// CI's linux job runs `--include-ignored`, which is where this bites.
#[test]
#[ignore]
fn a_dry_run_names_the_catalogue_entries_this_repo_is_not_using() {
    let sb = sandbox();
    sb.git_init();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );
    // Added to the catalogue *after* init wrote the list, which is the whole
    // case: the entry is off, and the reason is invisible without this report.
    std::fs::create_dir_all(sb.home.join(".omh/skills/refactor")).unwrap();
    std::fs::write(sb.home.join(".omh/skills/refactor/SKILL.md"), "x").unwrap();

    let said = String::from_utf8_lossy(&sb.omh(&["--dry-run", "claude"]).stderr).to_string();
    assert!(
        said.contains("skills/refactor"),
        "a launch has to name what it is not doing: {said}"
    );
    assert!(
        said.contains("omh use skills refactor"),
        "and the command that fixes it: {said}"
    );
}

// ── selection, and the two scopes ───────────────────────────────────────────

/// `omh use` writes the **committed** file. What a project uses is a fact about
/// the project, and a teammate cloning it should get the same selection — the
/// opposite default from `omh repo set`, which holds `carry_in` paths and MCP
/// env and must not be committable by accident. One flag could not express both,
/// which is why `--layer` split into two commands.
#[test]
fn use_writes_the_committed_file_and_unuse_takes_a_name_back_out() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md", "skills/refactor/SKILL.md"]);

    assert!(sb.omh(&["use", "skills", "review-diff"]).status.success());
    let written = sb.settings();
    assert!(written.contains("review-diff"), "got: {written}");
    assert!(
        written.contains("refactor"),
        "a capability that was following the whole catalogue must not be \
         narrowed to one name by adding one: {written}"
    );
    assert!(
        !sb.repo.join(".omh/settings.local.toml").exists(),
        "the gitignored file is `omh repo set`'s, not this command's"
    );

    assert!(sb.omh(&["unuse", "skills", "refactor"]).status.success());
    let written = sb.settings();
    assert!(written.contains("review-diff"), "got: {written}");
    assert!(!written.contains("refactor"), "taken back out: {written}");
}

/// Selecting something already selected is not a write and not an error.
#[test]
fn use_is_idempotent_and_unuse_refuses_a_name_this_repo_never_used() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);
    sb.omh(&["use", "skills", "review-diff"]);

    // The invariant, not the message: "already used" is what it says, and
    // "not a write" is what it means. Asserting the sentence left a mutation
    // that writes the list back before printing it entirely green.
    let before = sb.settings();
    let out = sb.omh(&["use", "skills", "review-diff"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already used"));
    assert_eq!(sb.settings(), before, "selecting it again touched the file");

    // Refused rather than written as a no-op: a name this repo never used is a
    // typo, and writing the list back would report success for it.
    let out = sb.omh(&["unuse", "skills", "nosuchthing"]);
    assert!(!out.status.success(), "a typo must not report success");
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuchthing"));
}

/// `[use]` names *your* entries; a feature is `[omh]`'s business, and the CLI
/// has to teach that rather than leave it in the docs.
#[test]
fn use_refuses_a_feature_and_disable_refuses_an_entry() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);

    let out = sb.omh(&["use", "mcp", "codegraph"]);
    assert!(!out.status.success(), "codegraph is omh's, not yours");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("codegraph"), "must name it: {err}");
    assert!(
        err.contains("omh repo disable codegraph"),
        "and point at the switch that does work: {err}"
    );

    // And the other direction, so the distinction is not one-way.
    let out = sb.omh(&["repo", "disable", "review-diff"]);
    assert!(!out.status.success(), "a skill is not a feature");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("omh use"), "point back the other way: {err}");
}

/// `omh repo disable` writes `[omh]` in the committed file, and says plainly
/// that nothing was uninstalled — the distinction the whole feature rests on.
#[test]
fn repo_disable_switches_a_feature_off_here_without_uninstalling_it() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["repo", "disable", "codegraph"]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sb.settings().contains("codegraph = false"),
        "{}",
        sb.settings()
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("nothing was uninstalled"));

    assert!(sb.omh(&["repo", "enable", "codegraph"]).status.success());
    assert!(
        sb.settings().contains("codegraph = true"),
        "{}",
        sb.settings()
    );
}

/// The two opposite defaults, side by side. `omh repo set` must not be able to
/// put a token in a file git will commit unless asked in so many words.
#[test]
fn repo_set_is_gitignored_and_shared_says_it_is_not() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb
        .omh(&["repo", "set", "carry_in", "[\".env\"]"])
        .status
        .success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(local.contains(".env"), "got: {local}");

    let out = sb.omh(&["repo", "set", "--shared", "idle_timeout", "30m"]);
    assert!(out.status.success());
    assert!(sb.settings().contains("30m"), "{}", sb.settings());
    // On **stderr**, and that is the stronger place for it. This warning is
    // the last thing standing between somebody and a token in git history,
    // and the invocation where that actually happens is the scripted one —
    // `omh repo set --shared … > log`, where anything on stdout goes to the
    // file unread. stderr is the stream that still reaches a person there.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("COMMITTED"),
        "writing the committed file has to say so, where a redirect cannot hide it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("COMMITTED"),
        "and it is a diagnostic, so it does not land in the answer"
    );
}

/// `omh config set` means **you** now. It used to default to the repo's
/// gitignored file; the secret-safety argument survives intact, because the
/// personal file is not committed either.
#[test]
fn config_set_writes_your_defaults() {
    let sb = sandbox();
    sb.seed_base();
    assert!(sb
        .omh(&["config", "set", "idle_timeout", "45m"])
        .status
        .success());
    let personal = std::fs::read_to_string(sb.home.join(".omh/settings.toml")).unwrap();
    assert!(personal.contains("45m"), "got: {personal}");
    assert!(
        !sb.repo.join(".omh/settings.local.toml").exists(),
        "this is not a repo-scoped command any more"
    );
}

/// `--layer` keeps working for one release and says what replaced it. A flag
/// that outlives its documentation is how people learn a form that is about to
/// stop existing; a hard error would cost more than it protects, since this one
/// is recoverable by retyping.
#[test]
fn layer_still_works_and_names_what_replaced_it() {
    let sb = sandbox();
    sb.seed_base();
    let out = sb.omh(&["config", "set", "--layer", "shared", "idle_timeout", "1h"]);
    assert!(out.status.success(), "still works");
    assert!(sb.settings().contains("1h"), "and writes where it said");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("going away"), "got: {said}");
    assert!(
        said.contains("omh repo set --shared"),
        "and names the form that replaces it: {said}"
    );
}

/// A name is checked where it is minted, so `edit` cannot be talked into
/// joining a path to the catalogue directory.
#[test]
fn edit_refuses_a_name_that_climbs_out_of_the_catalogue() {
    let sb = sandbox();
    sb.seed_base();
    let out = sb.omh(&["config", "edit", "skills", "../../../.ssh/id_rsa"]);
    assert!(!out.status.success(), "traversal must not reach $EDITOR");
    assert!(String::from_utf8_lossy(&out.stderr).contains("never a path"));
}

/// Bare `omh repo` is where the reporting this design keeps promising surfaces:
/// with a curated list the useful question stops being "what is this set to" and
/// becomes "why is this skill not here".
#[test]
fn bare_repo_reports_what_is_used_what_is_not_and_what_decided_it() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md", "skills/refactor/SKILL.md"]);
    sb.omh(&["use", "skills", "review-diff"]);
    sb.omh(&["unuse", "skills", "refactor"]);
    sb.omh(&["repo", "disable", "codegraph"]);
    sb.omh(&["repo", "set", "carry_in", "[\".env\"]"]);

    let out = sb.omh(&["repo"]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("review-diff"), "what is used: {said}");
    assert!(said.contains("refactor"), "and what is not: {said}");
    assert!(said.contains("codegraph"), "omh's features: {said}");
    assert!(said.contains("off here"), "and their state: {said}");
    assert!(said.contains("carry_in"), "settings: {said}");
    assert!(
        said.contains("local"),
        "and which file decided each: {said}"
    );
}

/// A settings file is a file somebody maintains by hand, and comments are part
/// of what they wrote.
///
/// Before P4 a write to `.omh/settings.toml` was rare — `omh config set` and
/// nothing else. Now `omh use`, `omh unuse` and `omh repo enable` all touch it,
/// and `init` writes it *full* of explanatory comments, so a round trip through
/// a serializer would have the first `omh use` silently delete everything init
/// had just explained. That is data loss, not formatting.
#[test]
fn writing_a_setting_keeps_what_you_wrote_around_it() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.toml"),
        "# why this repo carries an env file\ncarry_in = [\".env.local\"]  # the app needs it\n",
    )
    .unwrap();

    assert!(sb.omh(&["use", "skills", "review-diff"]).status.success());

    let after = sb.settings();
    assert!(
        after.contains("# why this repo carries an env file"),
        "the comment above a setting is part of the setting: {after}"
    );
    assert!(
        after.contains("# the app needs it"),
        "and so is the one beside it: {after}"
    );
    assert!(
        after.contains("review-diff"),
        "and the write happened: {after}"
    );
}

/// Everything this repo ships as data reaches `~/.omh`, where the code reads it.
///
/// The failure this catches has already happened twice in this project, and
/// `bundled.rs` opens by describing it: a guard is correct while the wiring that
/// reaches it is missing, and the suite stays green. `bundled`'s own tests
/// iterate what is *embedded*, so a directory absent from `build.rs`'s `SHIPPED`
/// is neither embedded nor noticed — the guard is structurally blind to exactly
/// the mistake of forgetting to add one.
///
/// Asserted against the repository's own directories rather than a list written
/// here, so a fifth kind is covered the day somebody adds it.
///
/// Runs anywhere: `init` seeds these before it needs a container, so it does not
/// matter that it fails later for want of one.
#[test]
fn init_installs_every_directory_this_repo_ships() {
    let sb = sandbox();
    std::fs::write(sb.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_omh"))
        .arg("init")
        .current_dir(&sb.repo)
        .env("HOME", &sb.home)
        .env("PATH", "/nonexistent")
        .output()
        .expect("the binary under test must run");

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for kind in ["adapters", "base", "editors", "stacks"] {
        let src = repo_root.join(kind);
        let shipped: Vec<String> = std::fs::read_dir(&src)
            .unwrap_or_else(|e| panic!("this repo must ship {kind}: {e}"))
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".toml"))
            .collect();
        assert!(!shipped.is_empty(), "{kind} ships nothing");

        for name in shipped {
            let landed = sb.home.join(".omh").join(kind).join(&name);
            assert!(
                landed.exists(),
                "{kind}/{name} is in the repo and not in ~/.omh — embedded but \
                 never installed, or never embedded. stdout: {} stderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

/// Provisioning stays behind the runtime check, and records nothing without it.
///
/// This is an **ordering** guard, not the cannot-tell one — with no runtime
/// `init` never reaches the provisioning block, so it would pass even if
/// `fired_from` lost its empty-answer branch. That rule is guarded where it can
/// actually be exercised, by `a_resolution_nobody_measured_is_never_recorded`.
/// What this catches is provisioning being moved *above* `runtime::select`,
/// which would write `[provision]` on a box that never asked a sandbox
/// anything — and since `stack::reconcile` drops every `true` it is not told
/// about, that write would erase the table rather than add to it.
///
/// Runs anywhere, because "no container runtime" is the condition under test.
#[test]
fn a_missing_container_runtime_records_no_resolution() {
    let sb = sandbox();
    std::fs::write(sb.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_omh"))
        .arg("init")
        .current_dir(&sb.repo)
        .env("HOME", &sb.home)
        .env("PATH", "/nonexistent")
        .output()
        .expect("the binary under test must run");

    let settings = sb.settings();
    assert!(
        !settings.contains("[provision]"),
        "an unmeasured resolution must not be written: {settings}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // And the repo is still configured — the container work stays last.
    assert!(settings.contains("[use]"), "got: {settings}");
}

/// Setting a repo up must not be abandoned half-done because the machine has no
/// container runtime.
///
/// The image build moved above the rest of `init` so a toolchain could be probed
/// before hooks were seeded. It propagates, so on a box with neither docker nor
/// sbx — somebody installing omh before a runtime, which is the first thing they
/// would do — `init` now bails after writing hooks and **before**
/// `config::write_selection` and before gitignoring `settings.local.toml`.
///
/// The second of those is the one that bites quietly: a `settings.local.toml`
/// left tracked is how a machine-local override reaches the team's repo, which
/// is the whole reason that line is written at all.
///
/// Runs anywhere, because "no container runtime" is the condition under test.
#[test]
fn a_missing_container_runtime_still_leaves_the_repo_configured() {
    let sb = sandbox();
    sb.git_init();
    std::fs::write(sb.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);

    // Nothing on PATH, so `runtime::select` can find no backend. Whether init
    // reports failure is not what is asserted — what it left behind is.
    let out = Command::new(env!("CARGO_BIN_EXE_omh"))
        .arg("init")
        .current_dir(&sb.repo)
        .env("HOME", &sb.home)
        .env("PATH", "/nonexistent")
        .output()
        .expect("the binary under test must run");

    let gitignore = std::fs::read_to_string(sb.repo.join(".omh/.gitignore")).unwrap_or_default();
    assert!(
        gitignore.contains("settings.local.toml"),
        "a tracked settings.local.toml is how a machine-local override gets \
         committed to the team's repo. stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sb.settings().contains("[use]"),
        "and the selection is what switches this repo's own hooks on: {}",
        sb.settings()
    );
}

/// `omh init` writes the selection out with every entry named.
///
/// Expanded rather than `"*"`, because an explicit list is editable and
/// reviewable in a way a wildcard is not — you curate by deleting lines. The
/// repo's own detected hooks are in it, because `init` wrote those a moment
/// earlier and a list that omitted them would switch off what init just
/// created; omh's own are not, because `[omh]` governs those and `[use]`
/// refuses to name one.
///
/// Both halves are asserted, because they are different claims. The first is
/// about what `init` wrote into `<repo>/.omh/hooks/` — the derived hooks, the
/// ones only this project could want — and it is checked against the directory
/// rather than against a spelling, so a hook added there later inherits it.
///
/// The second is about the conventional ones, which since hooks were separated
/// from stacks live in the **catalogue**: `cargo test` is what a rust project
/// runs, not what *this* rust project runs, so one body per ecosystem is the
/// honest scope. They reach a launch by being named in `[use]`, so that is
/// where this asserts they are — unconditionally, because `[toolchain]` governs
/// whether a hook *runs* here and never whether it is selected, which is why
/// this holds on an image with no rust in it. Both directions of the ecosystem
/// filter are checked: the loop above would pass just as happily on a selection
/// that named every ecosystem omh ships as on one that named none.
///
/// `#[ignore]`d because `init` builds an image, so it needs a container runtime.
/// CI's linux job runs `--include-ignored`, which is where this bites.
#[test]
#[ignore]
fn init_writes_the_selection_expanded() {
    let sb = sandbox();
    sb.git_init();
    std::fs::write(sb.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);
    assert!(sb.omh(&["init"]).status.success());

    let written = sb.settings();
    assert!(written.contains("[use]"), "got: {written}");
    assert!(written.contains("review-diff"), "your catalogue: {written}");

    // Everything init wrote is named in the list init then wrote. A hook on
    // disk and absent from `[use]` is a hook switched off by the same run that
    // created it.
    let hooks: Vec<String> = std::fs::read_dir(sb.repo.join(".omh/hooks"))
        .expect("init creates the hooks directory")
        .flatten()
        .map(|e| {
            e.file_name()
                .to_string_lossy()
                .trim_end_matches(".json")
                .to_string()
        })
        .collect();
    for h in &hooks {
        assert!(
            written.contains(h.as_str()),
            "init wrote {h} and then left it out of the selection: {written}"
        );
    }
    // And not vacuously. The detected stack's hooks are written whatever this
    // machine's image holds — the file is the repo's, and `[toolchain]` decides
    // whether it *runs*, never whether it exists. An empty directory would
    // satisfy the loop above while meaning init had stopped writing hooks.
    assert!(
        written.contains("\"rust-test\"") && written.contains("\"rust-format\""),
        "the detected stack's hooks are unconditional: {written}"
    );
    assert!(
        !written.contains("go-test") && !written.contains("python-test"),
        "and an ecosystem this repo is not must not be selected into it: {written}"
    );
    assert!(
        !written.contains("codegraph") || !written.contains("mcp = [\"codegraph"),
        "omh's own are `[omh]`'s, not `[use]`'s: {written}"
    );
    // The comment block init writes is what explains the file. A selection
    // appended by a serializer round trip would have deleted all of it.
    assert!(
        written.contains("# carry_in"),
        "init's own explanation has to survive its own write: {written}"
    );

    // Re-running must not resync a list somebody pruned on purpose.
    assert!(sb.omh(&["unuse", "skills", "review-diff"]).status.success());
    assert!(sb.omh(&["init"]).status.success());
    assert!(
        !sb.settings().contains("review-diff"),
        "init writes the list once; `omh use --all` is how you ask for a resync"
    );
}

/// A command that removes something has to remove it.
///
/// `omh use` and `omh unuse` write the committed file, but the selection is
/// resolved across all three settings files with the gitignored one last and
/// winning. So a `[use]` in `settings.local.toml` made `omh unuse` write
/// correctly, report success, and change nothing the session could see — the
/// shape the invariant table is built around ("nothing to commit is never a
/// successful commit").
///
/// Both files are written when both declare it. Refusing was the other option
/// and it is worse: the local table is usually there on purpose, and a command
/// that will not act until you delete it teaches people to stop using it.
#[test]
fn use_writes_every_repo_layer_that_already_declares_the_capability() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md", "skills/refactor/SKILL.md"]);
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "[use]\nskills = [\"review-diff\", \"refactor\"]\n",
    )
    .unwrap();

    let out = sb.omh(&["unuse", "skills", "refactor"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        !local.contains("refactor"),
        "the layer that decides has to be the layer that changed: {local}"
    );
    assert!(
        local.contains("review-diff"),
        "and only that name went: {local}"
    );
    assert!(
        sb.settings().contains("review-diff") && !sb.settings().contains("refactor"),
        "the committed file is still the one a teammate gets: {}",
        sb.settings()
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("settings.local.toml"),
        "and it says both files were written: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The other half: a local file that says nothing about this capability must
/// not acquire a `[use]` table because a committed one was edited. A selection
/// silently appearing in a gitignored file is how a teammate stops getting what
/// the repo says it uses.
#[test]
fn a_local_file_that_declares_nothing_stays_that_way() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "carry_in = [\".env\"]\n",
    )
    .unwrap();

    assert!(sb.omh(&["use", "skills", "review-diff"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        !local.contains("[use]"),
        "nothing was declared there: {local}"
    );
}

/// `[omh]` layers the same way, so `omh repo enable` has the same hole.
#[test]
fn a_feature_switch_reaches_the_layer_that_decides() {
    let sb = sandbox();
    sb.seed_base();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "[omh]\ncodegraph = false\n",
    )
    .unwrap();

    assert!(sb.omh(&["repo", "enable", "codegraph"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("codegraph = true"),
        "the local switch is what decides, so it is what has to move: {local}"
    );
}

/// Regression, seen live: `omh rm s01` deleted the worktree and left the
/// session container running. The next launch recreated the directory — a new
/// inode the running container's bind mount does not follow — and `session_up`,
/// seeing a container that was up, execed into it. Docker answered
///
///   OCI runtime exec failed: ... current working directory is outside of
///   container mount namespace root -- possible container breakout detected
///
/// on every command, forever: nothing in omh ever tears that container down, so
/// the session id stayed bricked until the user ran `docker rm -f` by hand.
///
/// A session *is* the container plus the worktree. Removing half of it is what
/// created a half that cannot be reached.
#[test]
fn rm_takes_the_session_container_down_with_the_worktree() {
    let sb = sandbox();
    let log = sb.fake_docker();
    let worktree = sb.home.join(".omh/worktrees/repo/s01");
    std::fs::create_dir_all(&worktree).unwrap();
    let run = sb.home.join(".omh/run/repo/s01");
    std::fs::create_dir_all(&run).unwrap();

    let out = sb.omh(&["s", "rm", "s01"]);
    assert!(
        out.status.success(),
        "rm failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!worktree.exists(), "the worktree must be gone");
    assert!(
        !run.exists(),
        "the session's staging and its last-used marker outlived it"
    );

    let calls = sb.docker_calls(&log);
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("rm ") && c.contains("omh-repo-s01")),
        "the container outlived the worktree it mounts: {calls:?}"
    );
}

/// The same half-removed state, seen from the other side. Nothing cleaned up
/// after `s rm` before this, so every removed session left a run directory and
/// often a container — and neither is visible from any command. The container
/// is the one that matters: an orphan holding a session id is what produced the
/// mount-namespace failure in the first place.
///
/// `doctor` and `auth` stage into the same tree under their own names and are
/// not sessions anybody can resume, so the marker `idle::touch` writes is what
/// separates a session that ran from scratch staging that never was one.
#[test]
fn s_ls_names_what_removed_sessions_left_behind() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();

    let launched = sb.home.join(".omh/run/repo/s02");
    std::fs::create_dir_all(&launched).unwrap();
    std::fs::write(launched.join("last-used"), "").unwrap();
    std::fs::create_dir_all(sb.home.join(".omh/run/repo/doctor")).unwrap();

    let out = sb.omh(&["s", "ls"]);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("s02"),
        "a run directory left behind: {printed}"
    );
    assert!(
        printed.contains("s03"),
        "a container left behind: {printed}"
    );
    assert!(
        !printed.contains("doctor"),
        "scratch staging is not a removed session: {printed}"
    );
}

// ── omh import hooks ────────────────────────────────────────────────────────
//
// The one part of this feature whose end-to-end path runs here: importing
// needs no container and no git, so these drive the real binary against a real
// file rather than asserting on a function's return value.

impl Sandbox {
    /// The shipped adapters, where `Paths::adapters()` looks. `init` would put
    /// them there and needs a container to finish.
    fn seed_adapters(&self) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters");
        let dst = self.home.join(".omh/adapters");
        std::fs::create_dir_all(&dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }

    /// A harness config to import from, in Claude Code's own shape.
    fn harness_hooks(&self, body: &str) -> PathBuf {
        let p = self.home.join("their-settings.json");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn repo_hook(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.repo.join(".omh/hooks").join(format!("{name}.json"))).ok()
    }
}

/// What somebody already configured arrives as omh hooks, in **this repo**.
///
/// The destination is the whole point of the first assertion: a catalogue hook
/// runs in every repo you ever open, so importing one project's formatter there
/// would put it in front of every other project you touch.
#[test]
fn importing_hooks_writes_them_into_this_repo() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    let theirs = fx.harness_hooks(
        r#"{"hooks":{
            "Stop":[{"matcher":"","hooks":[{"type":"command","command":"cargo test"}]}],
            "PostToolUse":[{"matcher":"Edit|Write|MultiEdit","hooks":[{"type":"command","command":"cargo fmt"}]}]}}"#,
    );

    let out = fx.omh(&[
        "import",
        "hooks",
        "claude",
        "--from",
        theirs.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let test = fx
        .repo_hook("turn-end-cargo")
        .expect("the turn-end hook must be in this repo");
    assert!(test.contains("cargo test"), "got: {test}");
    assert!(
        test.contains("\"on\": \"turn-end\""),
        "in omh's words, not Claude's: {test}"
    );

    let fmt = fx
        .repo_hook("after-tool-cargo")
        .expect("the after-tool hook must be in this repo");
    assert!(
        fmt.contains("cargo fmt") && fmt.contains("edit"),
        "got: {fmt}"
    );

    // And nowhere else. The catalogue is yours, across every project.
    assert!(
        !fx.home.join(".omh/hooks/turn-end-cargo.json").exists(),
        "a repo's hook must not be installed into the catalogue"
    );
}

/// **Copy, never move.** Adopting omh is not a migration somebody cannot back
/// out of: the harness they were using keeps working exactly as it did.
///
/// Asserted on the source's **bytes**, not its existence — a file truncated,
/// rewritten or reformatted in place is still there.
#[test]
fn importing_leaves_the_harness_config_untouched() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    let body = r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"cargo test"}]}]}}"#;
    let theirs = fx.harness_hooks(body);

    fx.omh(&[
        "import",
        "hooks",
        "claude",
        "--from",
        theirs.to_str().unwrap(),
    ]);

    assert_eq!(
        std::fs::read_to_string(&theirs).unwrap(),
        body,
        "the harness's own config must be byte-for-byte what it was"
    );
}

/// **An imported hook that is not selected is a hook no session ships.**
///
/// `[use]` is what the launcher reads. A file written without being named there
/// is one `omh import` counted and reported and no launch will ever run — the
/// report says `+2` and the session ships none of them, which is the most
/// likely silent failure this feature has.
#[test]
fn imported_hooks_are_selected_or_they_land_dead() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    // A repo that has curated its selection: `init` writes one, and after that
    // a hook not in it is off.
    std::fs::create_dir_all(fx.repo.join(".omh")).unwrap();
    std::fs::write(fx.repo.join(".omh/settings.toml"), "[use]\nhooks = []\n").unwrap();
    let theirs = fx.harness_hooks(
        r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"cargo test"}]}]}}"#,
    );

    let out = fx.omh(&[
        "import",
        "hooks",
        "claude",
        "--from",
        theirs.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        fx.settings().contains("turn-end-cargo"),
        "an imported hook must reach `[use]`, or it never runs: {}",
        fx.settings()
    );
}

/// **A hook answering to a name omh ships is refused**, and the reason is
/// worse than shadowing: `render::merge_hooks` treats it as an error naming
/// both files, so the whole session fails rather than that one hook.
///
/// Asserted through the real consumer — `omh why`, which builds the same
/// profile a launch does — rather than by checking the file is absent. What
/// matters is that omh still works afterwards.
#[test]
fn importing_refuses_a_name_omh_ships() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    let theirs = fx.harness_hooks(
        r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"graph-refresh --now"}]}]}}"#,
    );

    let out = fx.omh(&[
        "import",
        "hooks",
        "claude",
        "--from",
        theirs.to_str().unwrap(),
    ]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "importing is not fatal: {said}");

    // `graph-refresh` is omh's. Whatever omh did with it, a launch must still
    // be able to compose this repo.
    let why = fx.omh(&["why", "codegraph"]);
    assert!(
        why.status.success(),
        "importing left this repo unable to launch: {}",
        String::from_utf8_lossy(&why.stderr)
    );
}

/// A handler omh cannot express whole is **left where it is**, and named. It is
/// still in the harness's own file and still running there, which is honest —
/// but somebody who was not told would think omh had taken everything.
#[test]
fn what_omh_cannot_import_is_reported_rather_than_dropped() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    let theirs = fx.harness_hooks(
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[
            {"type":"command","command":"guard","if":"tool.name == 'Bash'"}]}]}}"#,
    );

    let out = fx.omh(&[
        "import",
        "hooks",
        "claude",
        "--from",
        theirs.to_str().unwrap(),
    ]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("left"), "the residue is reported: {said}");
    assert!(
        fx.repo_hook("before-tool-guard").is_none(),
        "and a hook whose permission gate omh cannot express is not written \
         without it"
    );
}

// ── omh import <capability> ─────────────────────────────────────────────────

impl Sandbox {
    /// A harness's own catalogue, on the host, where `import` reads.
    fn theirs(&self, at: &str, body: &str) -> PathBuf {
        let p = self.home.join(at);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    fn mine(&self, at: &str) -> Option<String> {
        std::fs::read_to_string(self.home.join(".omh").join(at)).ok()
    }
}

/// **Skills, commands and subagents go to the catalogue, not the repo.**
///
/// The opposite of hooks, and the reason is the one the docs give: a skill is a
/// way *you* work and travels with you across projects, while a hook binds to
/// one repo's commands. A skill imported into a repo would be a skill you only
/// had in one place.
#[test]
fn importing_a_skill_puts_it_in_your_catalogue() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    fx.theirs(
        ".claude/skills/review-diff/SKILL.md",
        "---\nname: review-diff\ndescription: read a diff\n---\n\nbody\n",
    );
    fx.theirs(".claude/skills/review-diff/notes/extra.md", "more\n");

    let out = fx.omh(&["import", "skills", "claude"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        fx.mine("skills/review-diff/SKILL.md")
            .is_some_and(|s| s.contains("read a diff")),
        "the skill must be in your catalogue"
    );
    assert!(
        fx.mine("skills/review-diff/notes/extra.md").is_some(),
        "a skill is a directory and arrives whole, not just its SKILL.md"
    );
    assert!(
        !fx.repo.join(".omh/skills").exists(),
        "a skill is yours across every project, not this repo's"
    );
}

/// **Rules are imported from your own file, never this project's.**
///
/// `rules::compose` already puts the repo's `CLAUDE.md` into every session, so
/// importing that one would hand the agent the same prose twice — and would go
/// on doing it in every other repo, because the catalogue travels.
#[test]
fn importing_rules_takes_yours_and_not_this_projects() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    fx.theirs(".claude/CLAUDE.md", "always write the test first\n");
    std::fs::write(fx.repo.join("CLAUDE.md"), "this project uses tabs\n").unwrap();

    let out = fx.omh(&["import", "rules", "claude"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let imported = fx.mine("rules/claude.md").expect("your own rules");
    assert!(
        imported.contains("always write the test first"),
        "got: {imported}"
    );
    assert!(
        !imported.contains("tabs"),
        "the project's own rules are composed already — importing them delivers \
         the same prose twice: {imported}"
    );
}

/// **A symlink is refused**, rather than followed or copied as a link.
///
/// The catalogue is mounted into every sandbox omh launches, so a link reaching
/// outside a skill would become a file the agent can read — in every project,
/// from a copy nobody had reason to inspect. Following it is an exfiltration
/// path; copying the link verbatim points somewhere else once the entry moves.
#[cfg(unix)]
#[test]
fn importing_refuses_a_skill_that_reaches_outside_itself() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    let secret = fx.theirs("secrets/id_rsa", "PRIVATE KEY\n");
    fx.theirs(".claude/skills/sneaky/SKILL.md", "---\nname: sneaky\n---\n");
    std::os::unix::fs::symlink(&secret, fx.home.join(".claude/skills/sneaky/borrowed.pem"))
        .unwrap();

    let out = fx.omh(&["import", "skills", "claude"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "one bad entry is not fatal: {said}");
    assert!(said.contains("skipped"), "and it is reported: {said}");
    assert!(
        fx.mine("skills/sneaky/borrowed.pem").is_none()
            && fx.mine("skills/sneaky/SKILL.md").is_none(),
        "an entry omh cannot copy whole is not copied in part"
    );
}

/// A name that is not a name never becomes a catalogue entry. `..` and a
/// separator are refused by the same rule `[use]` applies, so a path cannot be
/// smuggled in where an entry belongs.
#[cfg(unix)]
#[test]
fn importing_refuses_an_entry_whose_name_is_a_path() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    fx.theirs(".claude/commands/.hidden.md", "not an entry\n");
    fx.theirs(".claude/commands/real.md", "an entry\n");

    let out = fx.omh(&["import", "commands", "claude"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        fx.mine("commands/real.md").is_some(),
        "the good one arrives"
    );
    assert!(
        fx.mine("commands/.hidden.md").is_none(),
        "a dotfile is not a catalogue entry"
    );
}

/// Import never clobbers. An entry you have since edited is left exactly as it
/// is, and re-running is a no-op — the rule `omh config mcp import` already
/// follows.
#[test]
fn importing_twice_changes_nothing_the_second_time() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    fx.theirs(".claude/commands/review.md", "theirs\n");

    fx.omh(&["import", "commands", "claude"]);
    std::fs::write(fx.home.join(".omh/commands/review.md"), "mine, edited\n").unwrap();
    let out = fx.omh(&["import", "commands", "claude"]);

    assert!(out.status.success());
    assert_eq!(
        fx.mine("commands/review.md").as_deref(),
        Some("mine, edited\n"),
        "an import must not replace what you have since written"
    );
}

/// **A copy that fails part-way leaves nothing behind.**
///
/// The symlink check runs before anything is written, so it never reaches this
/// path — which is exactly why the cleanup needed its own test: deleting it
/// changed nothing, and the failure it guards against is the one nobody
/// arranges. A skill half-copied into the catalogue is mounted into every
/// sandbox exactly as a whole one is, and reads as an entry somebody chose.
///
/// Triggered with an unreadable file, so the failure is real rather than
/// injected. Skipped as root, where the permission would not bite and the test
/// would pass for the wrong reason.
#[cfg(unix)]
#[test]
fn a_copy_that_fails_part_way_leaves_nothing_behind() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root reads an unreadable file, so this proves nothing");
        return;
    }
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    fx.theirs(".claude/skills/big/SKILL.md", "---\nname: big\n---\n");
    let locked = fx.theirs(".claude/skills/big/zz-locked.md", "secret\n");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let out = fx.omh(&["import", "skills", "claude"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "one bad entry is not fatal: {said}");
    assert!(said.contains("skipped"), "and is reported: {said}");
    assert!(
        !fx.home.join(".omh/skills/big").exists(),
        "a half-copied entry must not survive: it is mounted into every sandbox \
         exactly as a whole one is"
    );
}

/// **A hook `init` derives on a re-run reaches `[use]`, or it lands dead.**
///
/// `merge_hooks` drops any hook the selection does not name, and a repo that
/// has been `init`ed once has a curated `[use]` that `init` will not resync. So
/// a project that gains a `package.json` six months later gets `pnpm-test.json`
/// written, sees it reported, and never runs it — the exact failure
/// `imported_hooks_are_selected_or_they_land_dead` pins for `omh import`.
///
/// Runs without a container: everything up to the harness block executes, and
/// the derived hook and the selection are both written before it.
#[test]
fn a_hook_init_derives_later_is_selected_too() {
    let fx = sandbox();
    fx.seed_base();
    fx.seed_adapters();
    // A repo already set up, with a list somebody has since curated.
    std::fs::create_dir_all(fx.repo.join(".omh")).unwrap();
    std::fs::write(fx.repo.join(".omh/settings.toml"), "[use]\nhooks = []\n").unwrap();
    // …which has since become a node project.
    std::fs::write(
        fx.repo.join("package.json"),
        r#"{"scripts":{"test":"vitest run"}}"#,
    )
    .unwrap();
    std::fs::write(fx.repo.join("pnpm-lock.yaml"), "").unwrap();

    let out = fx.omh(&["init"]);
    let said = String::from_utf8_lossy(&out.stdout);

    assert!(
        fx.repo.join(".omh/hooks/pnpm-test.json").exists(),
        "the hook is derived: {said}"
    );
    assert!(
        fx.settings().contains("pnpm-test"),
        "and named in `[use]`, or no session will ever run it: {}",
        fx.settings()
    );
}
