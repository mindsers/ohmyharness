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
    ///
    /// `ps` lists the `containers` file, which is what omh's probe reads: it
    /// asks for the running set and compares names itself, so the shim has no
    /// pattern to honour. An earlier version of this tried to emulate
    /// `--filter name=` and got the containment backwards — it tested whether
    /// the container name was a substring of the argv, so a shim asked about
    /// `omh-repo-s10` reported `omh-repo-s1` as running. Nothing in the tree
    /// needs that emulation now.
    ///
    /// Writing `docker-refuses` into the bin directory makes the shim exit
    /// non-zero, which is how a runtime that cannot be reached is reachable
    /// from a test at all.
    fn fake_docker(&self) -> PathBuf {
        let log = self.bin.join("docker.log");
        let shim = self.bin.join("docker");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
                 [ -f {refuses} ] && {{ echo 'cannot connect to the daemon' >&2; exit 1; }}\n\
                 if [ \"$1\" = inspect ]; then echo true; fi\n\
                 if [ \"$1\" = ps ]; then cat {containers} 2>/dev/null; fi\nexit 0\n",
                log = log.display(),
                refuses = self.bin.join("docker-refuses").display(),
                containers = self.bin.join("containers").display()
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

    /// A `docker` that has no images yet, so a build actually happens, and
    /// answers `images`/`ps` from files the test seeds.
    ///
    /// Separate from `fake_docker` because that one exits 0 for everything:
    /// `image inspect` succeeds, `exists()` reports the tag present, and the
    /// build — and therefore the reap — is skipped before it is reached. A
    /// shim that says yes to everything cannot test the path taken when
    /// something is missing.
    fn fake_docker_with_nothing_built(&self, tags: &[&str], in_use: &[&str]) -> PathBuf {
        let log = self.bin.join("docker.log");
        let images = self.bin.join("images");
        let containers = self.bin.join("containers");
        std::fs::write(&images, format!("{}\n", tags.join("\n"))).unwrap();
        std::fs::write(&containers, format!("{}\n", in_use.join("\n"))).unwrap();
        let shim = self.bin.join("docker");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
                 case \"$1 $2\" in\n\
                 \"image inspect\") exit 1 ;;\n\
                 \"image rm\") echo \"Untagged: $3\"; echo 'Deleted: sha256:00'; exit 0 ;;\n\
                 esac\n\
                 # omh sends the Dockerfile on stdin (`-f -`) so nothing is\n\
                 # written to disk. A shim that exits without reading it leaves\n\
                 # omh writing into a pipe with no reader, and omh sets SIGPIPE\n\
                 # to SIG_DFL on purpose — so it dies of signal 13, silently,\n\
                 # whenever the Dockerfile loses the race with this exit. That\n\
                 # is what failed this test on the linux runner three times\n\
                 # across three branches, each time saying only `init failed`.\n\
                 if [ \"$1\" = build ]; then cat > /dev/null; fi\n\
                 if [ \"$1\" = images ]; then cat {images}; fi\n\
                 if [ \"$1\" = ps ]; then cat {containers}; fi\n\
                 if [ \"$1\" = inspect ]; then echo true; fi\nexit 0\n",
                log = log.display(),
                images = images.display(),
                containers = containers.display(),
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

    /// A sandbox repository holding one commit no branch has.
    ///
    /// Deliberately minimal, and **not** a re-run of `Shadow::ensure`: it needs
    /// a seed record and a commit past it, which is all `unkept_work` reads.
    /// Getting it wrong makes the test fail loudly — `rm` would succeed and the
    /// assertion is that it refuses — rather than pass over a fixture that
    /// proved nothing, which is the failure mode that kept shadows out of this
    /// file until now.
    fn sandbox_repo_with_unkept_work(&self, id: &str, worktree: &std::path::Path) {
        let shadow = self
            .home
            .join(".omh/shadow")
            .join(self.repo.file_name().unwrap());
        std::fs::create_dir_all(&shadow).unwrap();
        let gitdir = shadow.join(format!("{id}.git"));
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&gitdir)
                .arg("--work-tree")
                .arg(worktree)
                .args(args)
                .output()
                .expect("git must be installed to run this test");
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&gitdir)
            .output()
            .unwrap();
        git(&["config", "user.email", "sandbox@omh.invalid"]);
        git(&["config", "user.name", "omh sandbox"]);
        git(&["commit", "-q", "--allow-empty", "--no-verify", "-m", "seed"]);
        std::fs::write(
            shadow.join(format!("{id}.seed")),
            git(&["rev-parse", "HEAD"]),
        )
        .unwrap();
        std::fs::write(worktree.join("agent.rs"), "fn agent() {}\n").unwrap();
        git(&["add", "-A", "."]);
        git(&["commit", "-q", "--no-verify", "-m", "the agent's own work"]);
    }

    /// Where a branch points, for asserting that it did not move.
    fn head_of_branch(&self, branch: &str) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["rev-parse", branch])
            .output()
            .expect("git must be installed to run this test");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
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
        // Once per repository, so a test can ask for a second session. It could
        // not before — `git_init` *deletes* `.git`, so a second call left the
        // first session's worktree pointing at a gitdir that no longer existed
        // and every command in it answered `not a git repository: (null)`. That
        // is why every test naming a session had exactly one to name, which is
        // the arrangement where `--session s01` and naming nothing give the
        // same answer and a selector that did nothing would pass them all.
        let first_session = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["remote"])
            .output()
            .is_ok_and(|o| !o.status.success() || o.stdout.is_empty());
        if first_session {
            self.git_init();
            git(&["config", "user.email", "t@example.com"]);
            git(&["config", "user.name", "t"]);
            git(&["commit", "-q", "--allow-empty", "-m", "root"]);
            git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        }

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

/// Asking to see the agent's work before the agent has run is an ordinary
/// thing to do, and the answer is *nothing yet* rather than a failure.
///
/// A session whose sandbox has never started has no repository to read at all:
/// `checkpoints` would ask for the seed and get "no seed recorded", which is
/// true and is not what the user asked. The reading itself is unit-tested
/// against a real sandbox repository in `shadow.rs` — building one here would
/// mean a fixture that reimplements `ensure`, and a fixture that reimplements
/// the thing it tests proves whichever of the two is wrong.
#[test]
fn a_log_for_a_sandbox_that_never_ran_says_nothing_yet() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("in-progress.rs"), "fn main() {}").unwrap();

    let out = sb.omh(&["s01", "log"]);
    let printed = String::from_utf8_lossy(&out.stdout);
    let said = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "there is nothing wrong with an empty sandbox: {said}"
    );
    assert!(
        printed.contains("no checkpoints"),
        "it says so plainly: {printed}"
    );
    // Zero, and correctly: the count is what `--keep` would sweep out of the
    // *sandbox*, and there is no sandbox. The file sitting in the worktree is a
    // fact about the session, which `omh s ls` and `omh s diff` answer — this
    // line answers what the harvest is about to do, so it must not borrow a
    // number measured somewhere else.
    assert!(
        printed.contains("uncommitted in the sandbox: 0 files"),
        "nothing is staged for a harvest that has nothing to harvest: {printed}"
    );
}

/// `--json` never hands the terminal to a pager, and says which of the two it
/// gave you.
///
/// A script asking for a patch gets the patch as a field. Paging is for a
/// person, and `less` between a program and the object it asked for is a hang
/// with no error — the failure mode that has no output to diagnose it by.
///
/// The **key** is the other half. `Diff`'s own doc comment argued the field
/// should be named for what it holds, because `jq -r .patch | git apply` on a
/// `--stat` fails on every session that changed anything; `-p` then put a real
/// patch under `summary`, which is that footgun with the labels swapped. One
/// key or the other, never both, so a script can tell without sniffing for
/// `@@`.
///
/// Asserted through the binary rather than on the branch inside `diff`,
/// because the branch is the thing that could be wrong: a unit test of the two
/// arms would agree with whichever one was written.
#[test]
fn a_patch_asked_for_by_a_program_is_a_field_named_for_what_it_holds() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn added() {}\n").unwrap();

    let read = |args: &[&str]| -> serde_json::Value {
        let out = sb.omh(args);
        let printed = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&printed).unwrap_or_else(|e| panic!("not JSON: {e}: {printed}"))
    };

    let patch = read(&["s01", "diff", "-p", "--json"]);
    assert_eq!(patch["changed"], serde_json::json!(true));
    assert!(
        patch["patch"]
            .as_str()
            .is_some_and(|s| s.contains("+fn added() {}")),
        "the patch itself, under `patch`: {patch}"
    );
    assert!(
        patch["summary"].is_null(),
        "and not also under `summary`: {patch}"
    );

    let summary = read(&["s01", "diff", "--json"]);
    assert!(
        summary["summary"]
            .as_str()
            .is_some_and(|s| s.contains("feature.rs") && !s.contains("+fn added() {}")),
        "a --stat, under `summary`: {summary}"
    );
    assert!(
        summary["patch"].is_null(),
        "nothing a script could hand to `git apply`: {summary}"
    );
    assert_eq!(
        summary["session"],
        serde_json::json!("s01"),
        "the id, not a phrase: {summary}"
    );
}

/// The flag is the only thing that decides which of the two you get.
///
/// End to end, because the wiring is what could be wrong: hardcoding
/// `What::Patch` in `diff_report`'s `false` arm left the whole suite green
/// while `omh sNN diff` dumped a full patch to stdout unbidden. Every existing
/// test asserted the file was *named*, which a patch also does.
///
/// No pty is needed: git suppresses its pager when stdout is not a terminal,
/// so the patch arrives captured.
#[test]
fn the_flag_is_what_decides_whether_a_diff_is_a_summary_or_a_patch() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn added() {}\n").unwrap();

    let summary = String::from_utf8_lossy(&sb.omh(&["s01", "diff"]).stdout).to_string();
    let patch = String::from_utf8_lossy(&sb.omh(&["s01", "diff", "-p"]).stdout).to_string();

    assert!(
        summary.contains("feature.rs") && !summary.contains("+fn added() {}"),
        "without the flag, the shape of the change: {summary}"
    );
    assert!(
        patch.contains("+fn added() {}"),
        "with it, the change: {patch}"
    );
}

/// `--edit` without a terminal refuses rather than reporting a curation that
/// never happened.
///
/// Measured before this landed: with stdin not a terminal, `rebase -i` runs
/// the **unedited** todo, exits 0, and omh reports the work as curated. So the
/// flag that opens an editor is the one that has to ask whether there is
/// anywhere to draw — and it is now the only path that needs one, which is
/// what makes a single guard enough.
///
/// A test process has no tty, so this is the ordinary case here rather than a
/// contrived one.
#[test]
fn edit_without_a_terminal_refuses_rather_than_pretending_to_curate() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["s01", "commit", "--keep", "--edit"]);
    let said = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "there is nowhere to draw: {said}");
    assert!(
        said.contains("no terminal"),
        "and it says why, rather than reporting a curation: {said}"
    );
    assert!(
        said.contains("--keep 1,3-4") || said.contains("Drop `--edit`"),
        "with something to do instead: {said}"
    );
}

/// A selection is refused before the branch moves.
///
/// The refusals themselves are asserted in `src/main.rs`, against a session
/// with a real sandbox behind it — an earlier version of this test lived here
/// alone and proved nothing: `sb.session()` builds a worktree and no sandbox
/// repository, so `9`, `0`, `two` and `4-2` all died identically inside
/// `seed()`, about a record the user has never heard of. Gutting the parser
/// left it green.
///
/// What is left here is the half that needs the whole binary: whatever the
/// refusal says, the branch is where it was.
#[test]
fn a_refused_selection_leaves_the_branch_where_it_was() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("work.rs"), "fn work() {}\n").unwrap();
    let before = sb.head_of_branch("omh/s01");

    for selection in ["9", "0", "two", "4-2"] {
        let out = sb.omh(&["s01", "commit", "--keep", selection]);
        assert!(
            !out.status.success(),
            "`--keep {selection}` was accepted: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    assert_eq!(
        sb.head_of_branch("omh/s01"),
        before,
        "the branch never moved"
    );
}

/// Two sessions changing one file are named together.
///
/// The collision git will not mention until a merge, said while both sessions
/// are open and either could be redirected. End to end, because it is wiring:
/// the paths come from a `status --porcelain` that `s ls` already ran for its
/// uncommitted count and used to throw away, and the grouping is a table in
/// `report.rs` that a unit test cannot connect to the sessions on disk.
#[test]
fn sessions_changing_the_same_file_are_named_together() {
    let sb = sandbox();
    let one = sb.session("s01");
    let two = sb.session("s02");
    for (worktree, extra) in [(&one, "only-in-s01.rs"), (&two, "only-in-s02.rs")] {
        std::fs::write(worktree.join("shared.rs"), "fn shared() {}\n").unwrap();
        std::fs::write(worktree.join(extra), "fn mine() {}\n").unwrap();
    }

    let printed = String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stdout).to_string();

    assert!(
        printed.contains("s01 and s02 both change shared.rs"),
        "the file both are changing, and who: {printed}"
    );
    assert!(
        !printed.contains("only-in-s01.rs"),
        "and nothing about what only one of them touches: {printed}"
    );
    // Part of the answer rather than an aside: this is the most consequential
    // line in a record of what is in flight, and stderr is not where a
    // redirected listing keeps it.
    assert!(
        !String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stderr).contains("both change"),
        "it is the answer, not a warning"
    );

    // The document says what the sentence says. `--json` is the scripting
    // contract, and deleting the field entirely left every other assertion
    // here green.
    let doc: serde_json::Value =
        serde_json::from_slice(&sb.omh(&["s", "ls", "--json"]).stdout).unwrap();
    assert_eq!(
        doc["overlaps"],
        serde_json::json!([{"sessions": ["s01", "s02"], "paths": ["shared.rs"]}]),
        "one answer, two renderings: {doc}"
    );
    assert_eq!(doc["unreadable"], serde_json::json!([]));
}

/// A session omh cannot read is said so, because its absence from the overlap
/// section otherwise means it collides with nobody.
///
/// A stale `.git` pointer is the real case — `work_state`'s own comment names
/// it, "a checkout moves" — and the listing renders that as `?` in one column
/// while the section below quietly computes over a subset. No overlap line is
/// exactly how "no collisions" looks.
#[test]
fn a_session_omh_cannot_read_is_named_rather_than_left_out() {
    let sb = sandbox();
    let one = sb.session("s01");
    let two = sb.session("s02");
    std::fs::write(one.join("shared.rs"), "fn shared() {}\n").unwrap();
    std::fs::write(two.join("shared.rs"), "fn shared() {}\n").unwrap();
    // s02's worktree loses its way back to the repository.
    std::fs::write(two.join(".git"), "gitdir: /nowhere-at-all\n").unwrap();

    let out = sb.omh(&["s", "ls"]);
    let printed = String::from_utf8_lossy(&out.stdout).to_string();

    assert!(
        printed.contains("could not read what s02 is changing"),
        "the session omh could not read is named: {printed}"
    );
    assert!(
        printed.contains("incomplete"),
        "and what that means for the rest: {printed}"
    );
    // …and the collision it would have been part of is not asserted as absent.
    assert!(
        !printed.contains("s01 and s02 both change"),
        "omh does not invent a collision it could not check: {printed}"
    );
}

/// A sandbox repository with no session is reported, not left to rot.
///
/// [risks](../docs/design/risks.md) 8c. The most valuable of the three orphans
/// `s ls` looks for: a container is re-creatable and a run directory holds a
/// timestamp, while this holds every commit an agent made and nothing points
/// at it.
#[test]
fn a_sandbox_repository_with_no_session_is_reported() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    // s01 gets a real repository too, so the live-session filter has something
    // to filter. Without it this test passed with the filter deleted: `s01`
    // had no shadow, so it was never in the list to be removed from.
    sb.sandbox_repo_with_unkept_work("s01", &worktree);
    let orphan = sb
        .home
        .join(".omh/shadow")
        .join(sb.repo.file_name().unwrap())
        .join("s09.git");
    std::fs::create_dir_all(&orphan).unwrap();

    let out = sb.omh(&["s", "ls"]);
    let said = String::from_utf8_lossy(&out.stderr);

    assert!(
        said.contains("s09"),
        "a repository nothing points at is named: {said}"
    );
    assert!(
        !said.contains("s01"),
        "and a session that is still here is not — every live session has a \
         repository, so this filter is the only thing between a healthy checkout \
         and being told to `rm` all of it: {said}"
    );

    // The hint is only worth printing if it works. `--force` because the
    // orphan holds a commit, which is #58 doing its job.
    assert!(
        sb.omh(&["s09", "rm", "--force"]).status.success(),
        "the hint `s ls` prints has to be a command that clears it"
    );
    assert!(!orphan.exists(), "and it did");
    assert!(
        !String::from_utf8_lossy(&sb.omh(&["s", "ls"]).stderr).contains("s09"),
        "so a second listing no longer names it"
    );
}

/// `rm` refuses over work that exists nowhere else, and `--force` is the way
/// past.
///
/// End to end, because the guard's whole value is being *reached*: the
/// decision is a table in `src/main.rs`, and this is the half that says `rm`
/// asks it. Removing the call left that table green.
#[test]
fn removing_a_session_holding_unkept_work_is_refused_until_it_is_meant() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    sb.sandbox_repo_with_unkept_work("s01", &worktree);

    let log = sb.fake_docker();
    let run = sb
        .home
        .join(".omh/run")
        .join(sb.repo.file_name().unwrap())
        .join("s01");
    std::fs::create_dir_all(&run).unwrap();
    let gitdir = sb
        .home
        .join(".omh/shadow")
        .join(sb.repo.file_name().unwrap())
        .join("s01.git");

    let out = sb.omh(&["s01", "rm"]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "the agent's commit is on no branch: {said}"
    );
    assert!(
        said.contains("s01 has 1 commit that no branch has"),
        "it says what is at stake, in the singular: {said}"
    );
    assert!(said.contains("--force"), "and how to mean it: {said}");

    // "Nothing was taken down" is about the things that go *first*. The
    // worktree is removed last, so its survival is true of any ordering that
    // fails anywhere — moving the guard below the container teardown would
    // leave it standing and prove nothing.
    assert!(
        !sb.docker_calls(&log).iter().any(|c| c.starts_with("rm ")),
        "the container was taken down on the way to refusing: {:?}",
        sb.docker_calls(&log)
    );
    assert!(run.exists(), "so was the marker `s ls` reads");
    assert!(gitdir.exists(), "and the repository the refusal is about");
    assert!(worktree.exists());

    let out = sb.omh(&["s01", "rm", "--force"]);
    assert!(
        out.status.success(),
        "--force means it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!worktree.exists(), "the session is gone");
    // The repository is the thing the refusal was about, and the only test in
    // the tree where `rm` runs with a real one on disk. Left behind, the next
    // session issued this id adopts a dead session's history.
    assert!(
        !gitdir.exists(),
        "…and so is the sandbox repository it was protecting"
    );
}

/// An empty patch is a sentence, not a blank screen.
///
/// `Diff::human` exists partly to say *no changes on … (against …)*, because
/// silence reads as breakage. Handing the terminal straight to git skipped it,
/// and three quite different states then rendered identically: nothing
/// changed, the worktree had left its branch, and the pager was broken. The
/// first is the common one, so it is the one that made the other two look
/// survivable.
#[test]
fn a_patch_with_nothing_in_it_says_so_rather_than_showing_a_blank_screen() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["s01", "diff", "-p"]);
    let printed = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "an unchanged session is not an error");
    assert!(
        printed.contains("no changes"),
        "the reader is told which comparison came up empty: {printed:?}"
    );
}

/// Both routes to a patch refuse the same worktrees.
///
/// The paged path was written as a second copy of the unpaged one and dropped
/// the guard against a worktree that left its branch — so `omh sNN diff`
/// refused, naming the branch, and `omh sNN diff -p` printed an empty patch
/// and exited 0, one flag apart. Four reviewers found it independently.
///
/// Asserted as an agreement rather than as a second copy of the guard's
/// wording: what matters is that the two routes answer the same question about
/// whether there is an answer at all, which survives a third route being added.
#[test]
fn a_worktree_that_left_its_branch_is_refused_whichever_way_you_ask() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn added() {}\n").unwrap();

    let healthy: Vec<bool> = [vec!["s01", "diff"], vec!["s01", "diff", "-p"]]
        .iter()
        .map(|args| sb.omh(args).status.success())
        .collect();
    assert_eq!(healthy, vec![true, true], "both work on a healthy session");

    // Look at something else for a moment, the way a person does.
    let head = Command::new("git")
        .arg("-C")
        .arg(&worktree)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let out = Command::new("git")
        .arg("-C")
        .arg(&worktree)
        .args(["checkout", "-q", "--detach", &head])
        .output()
        .unwrap();
    assert!(out.status.success(), "detaching the worktree: {out:?}");

    for args in [vec!["s01", "diff"], vec!["s01", "diff", "-p"]] {
        let out = sb.omh(&args);
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success(),
            "`omh {}` handed over a review from a worktree that left its branch",
            args.join(" ")
        );
        assert!(
            said.contains("omh/s01"),
            "and the refusal names the branch: {said}"
        );
    }
}

/// `--base` alongside a checkpoint is refused rather than dropped.
///
/// A checkpoint is measured against its own parent, so a `--base` given with
/// one can only be ignored — and `omh s01 diff 4 --base v1.2` silently
/// answering about the parent is the resolve-by-quietly-dropping-one this
/// codebase refuses for `--new` and `--session`.
#[test]
fn a_base_given_with_a_checkpoint_is_refused() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["s01", "diff", "4", "--base", "main"]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && said.contains("--base"),
        "the flag cannot be honoured here and is not silently dropped: {said}"
    );
}

/// A never-launched sandbox answers `diff <n>` the way `log` answers.
///
/// `log` goes to some trouble to say *no checkpoints* and exit 0 for a session
/// whose sandbox has never run. `diff 1` on the same session used to reach
/// `seed()` and quote the path of a record the user has never heard of — two
/// commands one word apart, one of them speaking about omh's internals.
#[test]
fn a_checkpoint_asked_for_before_the_sandbox_ran_says_so_plainly() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["s01", "diff", "1"]);
    let said = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "there is no checkpoint 1: {said}");
    assert!(
        said.contains("has not committed anything"),
        "and it says so in the words `log` uses: {said}"
    );
    assert!(
        !said.contains("seed"),
        "rather than naming a record the user has never heard of: {said}"
    );
}

/// The session named first is the session acted on — not the one omh would
/// have picked.
///
/// Two sessions, because with one the question cannot be asked: `pick` falls
/// back to the only session there is, so every assertion holds whether the
/// prefix works or is dropped on the floor. Both directions, because whichever
/// of the two `pick` prefers would otherwise carry a test that proves nothing.
#[test]
fn the_session_named_first_is_the_one_the_command_acts_on() {
    let sb = sandbox();
    let one = sb.session("s01");
    let two = sb.session("s02");
    std::fs::write(one.join("only-in-s01.rs"), "fn main() {}").unwrap();
    std::fs::write(two.join("only-in-s02.rs"), "fn main() {}").unwrap();

    for (named, mine, theirs) in [
        ("s01", "only-in-s01.rs", "only-in-s02.rs"),
        ("s02", "only-in-s02.rs", "only-in-s01.rs"),
    ] {
        let out = sb.omh(&[named, "diff"]);
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(
            printed.contains(mine) && !printed.contains(theirs),
            "`omh {named} diff` has to report {named}: {printed}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The spellings the prefix replaced are gone, not quietly still accepted.
///
/// A deletion nothing asserts is one a later change restores by accident — and
/// `diff` taking an id in two places is how the session came to have two
/// answers in the first place.
#[test]
fn the_spellings_the_prefix_replaced_are_refused() {
    let sb = sandbox();
    sb.session("s01");
    for line in [
        vec!["s", "diff", "s01"],
        vec!["s", "rm", "s01"],
        vec!["s", "down", "s01"],
        vec!["graph", "s01"],
    ] {
        let out = sb.omh(&line);
        let said = String::from_utf8_lossy(&out.stderr);
        // Refused *for naming it there*, not for some unrelated reason further
        // down — a test that only asks for a non-zero exit passes on the day
        // the command breaks for a different cause entirely. The refusal has
        // to quote the token, which is what makes it about that token; the
        // wording differs by slot and is not the invariant. `diff` says
        // *invalid value 's01' for '[CHECKPOINT]'* now that the slot takes a
        // checkpoint number, which is a better answer than the one this used
        // to pin.
        assert!(
            !out.status.success() && said.contains("'s01'"),
            "`omh {}` names the session where it no longer goes: {said}",
            line.join(" ")
        );
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

/// Committing does not *stop* `s diff` reporting: the work is the same work
/// before and after, and a review that changed its answer at the moment of a
/// commit would be reporting the commit rather than the session.
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

    let printed = String::from_utf8_lossy(&sb.omh(&["s01", "diff"]).stdout).to_string();
    assert!(printed.contains("feature.rs"), "got: {printed}");
}

/// The refusal is wired to the command, not just to a function that could
/// refuse.
///
/// The unit test decides *what* to say about markers; this asserts the command
/// asks at all. Worth its own case because the failure mode is a deleted line
/// rather than a wrong answer: `commit` would keep passing every other test
/// and quietly land a half-resolved merge on the branch.
#[test]
fn a_commit_over_conflict_markers_is_refused_by_the_command_itself() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(
        worktree.join("tap.rs"),
        "<<<<<<< main\nfn ours() {}\n=======\nfn theirs() {}\n>>>>>>> s01\n",
    )
    .unwrap();

    let out = sb.omh(&["s01", "commit", "-m", "Add the tap"]);
    assert!(!out.status.success(), "a half-resolved merge does not land");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("tap.rs:1"), "and it says where: {err}");

    let out = sb.omh(&["s01", "commit", "-m", "Add the tap", "--force"]);
    assert!(
        out.status.success(),
        "and the user can still mean it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--keep` is the harvest, and it says so rather than pretending it committed.
///
/// A session whose sandbox never ran has no repository to keep anything from,
/// and the honest answer is "nothing to keep" — not a cheerful "committed" over
/// an empty branch, which is the report a user would act on.
#[test]
fn keeping_a_sessions_own_commits_says_so_when_there_are_none() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn main() {}").unwrap();

    let out = sb.omh(&["s", "commit", "--keep"]);

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("no sandbox repository") || said.contains("nothing to keep"),
        "a session with no sandbox repository has nothing to keep, and has to \
         say which: {said}"
    );
    assert!(
        !said.contains("committed to"),
        "and must not report a commit it did not make: {said}"
    );
}

/// `-m` and `--keep` are two ways to land the same work and must not both run:
/// the squash lands the content first, and git's patch-id then drops every
/// replanted commit as already applied — the granular history `--keep` exists
/// to deliver, gone with nothing said.
#[test]
fn a_message_and_keeping_the_agents_commits_are_refused_together() {
    let sb = sandbox();
    sb.session("s01");

    let out = sb.omh(&["s", "commit", "-m", "squashed", "--keep"]);

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cannot be used with"), "got: {err}");
}

/// The agent cannot commit — that is the whole shape of a session — so the
/// window in which `s diff` is the *only* way to see the work is the entire
/// time the agent is running. It reported an empty diff for all of it, while
/// the rules omh ships told the agent the user reviews before committing.
///
/// End to end rather than against `Session::diff`, because the unit tests
/// would stay green if `s diff` stopped reaching it.
#[test]
fn diff_reports_a_sessions_work_before_anyone_commits_it() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    std::fs::write(worktree.join("feature.rs"), "fn main() {}").unwrap();

    let out = sb.omh(&["s01", "diff"]);

    assert!(
        out.status.success(),
        "diff failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("feature.rs"),
        "uncommitted work is the only work there is yet: {printed}"
    );
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

    let out = sb.omh(&["s01", "rm"]);
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
/// The everyday path, end to end: work is on the branch, the session goes, and
/// the branch stays for review.
///
/// `Session::remove`'s decision is pinned by unit tests. What this pins is that
/// the command wired to it does not delete the branch and reports the count
/// that decided the outcome — the number now travels *with* the outcome rather
/// than being asked for a second time.
///
/// Asserted against git and the JSON document rather than the sentence: the
/// prose may be reworded, the branch may not go missing.
#[test]
fn removing_a_session_that_committed_keeps_the_branch_for_review() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    let worktree = sb.session("s01");

    std::fs::write(worktree.join("work.txt"), "agent output").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "agent work"]] {
        let out = Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(&args)
            .output()
            .expect("git must be installed to run this test");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    let out = sb.omh(&["s01", "rm", "--json"]);
    assert!(
        out.status.success(),
        "rm failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let alive = Command::new("git")
        .arg("-C")
        .arg(&sb.repo)
        .args(["rev-parse", "--verify", "omh/s01"])
        .output()
        .unwrap();
    assert!(
        alive.status.success(),
        "unreviewed work must outlive the session that made it"
    );

    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`--json` is one document");
    assert_eq!(doc["branch_kept"], serde_json::json!(true));
    assert_eq!(
        doc["commits"],
        serde_json::json!(1),
        "and the count reported is the one that decided it"
    );
}

/// `ls` is the one verb the prefix cannot scope, so it says so.
///
/// Every other session verb acts on one session; `ls` is about the set. Taking
/// the prefix and ignoring it would list every session while looking like it
/// had listed one, which is the kind of quiet wrongness this whole selector
/// exists to remove.
#[test]
fn scoping_the_one_verb_that_lists_them_all_is_refused() {
    let sb = sandbox();
    let _log = sb.fake_docker();

    let out = sb.omh(&["s01", "ls"]);
    assert!(!out.status.success(), "it has to refuse, not quietly widen");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("s01") && said.contains("omh s ls"),
        "and name both what it dropped and what to type: {said}"
    );
}

/// A runtime omh cannot reach is never reported as a sandbox that is stopped.
///
/// End to end, because the unit tests decide what each layer *says* and this
/// decides that the layers are wired to each other. The failure it guards is
/// specific and was live: with the Docker daemon down, `omh s ls` printed
/// `stopped` beside every session — in both formats, with nothing on stderr —
/// and `omh sNN sync` read the same false all-clear and would have written
/// over the files of a live agent.
#[test]
fn a_runtime_that_cannot_be_reached_is_not_reported_as_a_stopped_sandbox() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    std::fs::write(sb.bin.join("docker-refuses"), "").unwrap();

    let out = sb.omh(&["s", "ls"]);
    let printed = String::from_utf8_lossy(&out.stdout);
    // Anchored on the row being there at all. `Sessions::human` renders an
    // empty list as `no sessions`, which contains neither `stopped` nor `s01`
    // — so the absence assertion below passed on a listing with nothing in it.
    assert!(printed.contains("s01"), "the session is listed: {printed}");
    assert!(
        !printed.contains("stopped"),
        "a question omh could not answer is not an answer: {printed}"
    );
    assert!(
        printed.contains("up?"),
        "it is rendered as the question it is: {printed}"
    );
    // The reason, which the first version of this built, carried through two
    // layers and dropped — while the docs promised it was here.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("could not tell whether s01's sandbox is running"),
        "and the reason reaches stderr: {err}"
    );

    // The JSON has no second signal — `--json` returns before asides — so the
    // field is the whole of what a script gets.
    let json = sb.omh(&["s", "ls", "--json"]);
    let doc: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("s ls --json is a document");
    // `serde_json` indexing returns `Null` for a missing key, a non-array and
    // an out-of-range index alike, so `doc["sessions"][0]["running"]` was
    // `Null` for an empty document too. The length and the id anchor it.
    assert_eq!(
        doc["sessions"].as_array().map(Vec::len),
        Some(1),
        "one session in the document: {doc}"
    );
    assert_eq!(
        doc["sessions"][0]["id"],
        serde_json::json!("s01"),
        "and it is s01: {doc}"
    );
    assert_eq!(
        doc["sessions"][0]["running"],
        serde_json::Value::Null,
        "and a script is not told `false`: {doc}"
    );
    assert!(
        doc["sessions"][0]["running_unknown"].is_string(),
        "with the reason beside it, since `--json` never sees the warning: {doc}"
    );

    // The one that matters. A sync here would land files under an agent that
    // may well be mid-turn.
    let sync = sb.omh(&["s01", "sync"]);
    assert!(!sync.status.success(), "sync does not proceed on a guess");
    let err = String::from_utf8_lossy(&sync.stderr);
    assert!(
        err.contains("could not tell whether s01 is running"),
        "and says why it stopped: {err}"
    );
}

/// `down` over an unreachable runtime reports the session it could not ask
/// about, rather than leaving a hole where the row should be.
///
/// The hole was the point: skipping the push meant `omh down` printed
/// **`no sessions`** on stdout — the answer channel — and `"sessions": []` in
/// JSON, over a daemon it never reached. A missing row is worse than a wrong
/// one; a script iterating the list sees nothing at all and no error.
#[test]
fn down_over_an_unreachable_runtime_still_names_the_session() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    std::fs::write(sb.bin.join("docker-refuses"), "").unwrap();

    let out = sb.omh(&["s01", "down", "--json"]);
    assert!(!out.status.success(), "it is a failure, and exits like one");

    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("down --json is a document");
    assert_eq!(
        doc["sessions"].as_array().map(Vec::len),
        Some(1),
        "the session is in the document: {doc}"
    );
    assert_eq!(doc["sessions"][0]["session"], serde_json::json!("s01"));
    assert_eq!(
        doc["sessions"][0]["stopped"],
        serde_json::Value::Null,
        "`null`, not `false` — omh never asked it to stop: {doc}"
    );
    assert!(
        doc["sessions"][0]["why"].is_string(),
        "with the runtime's reason beside it: {doc}"
    );

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("could not be asked") && !err.contains("would not stop"),
        "and it does not claim the container refused to stop: {err}"
    );
}

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
    // On **stderr**: a leftover is something wrong, not what `s ls` was asked
    // for, and `omh s ls > sessions.txt` must not collect it.
    let printed = String::from_utf8_lossy(&out.stderr);
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

/// The promise in `docs/commands.md`, pinned against the command that broke it.
///
/// *stdout is the answer; stderr is everything else* was documented and then
/// contradicted by this exact invocation: the leftovers warning and its
/// `omh s rm` hint were appended to the table, so both landed in the file. A
/// prose rule nothing checks is a rule that drifts back — this is the check.
#[test]
fn a_redirected_s_ls_collects_the_sessions_and_nothing_else() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();

    let out = sb.omh(&["s", "ls"]);
    let answer = String::from_utf8_lossy(&out.stdout);
    let aside = String::from_utf8_lossy(&out.stderr);

    assert!(
        !answer.contains("rm"),
        "a next step is not part of a redirected answer — got {answer:?}"
    );
    assert!(
        !answer.contains("left something behind"),
        "nor is a warning — got {answer:?}"
    );
    assert!(
        aside.contains("left something behind") && aside.contains("rm"),
        "both still reach the person watching — got {aside:?}"
    );
}

/// `--json` carries the same facts as fields, and drops the prose entirely.
///
/// The asides are suppressed rather than merely unstyled: `leftovers` is
/// already in the document, so a sentence about it on stderr is a second copy
/// for something that is parsing the first.
#[test]
fn json_carries_leftovers_as_a_field_and_says_nothing_about_them() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();

    let out = sb.omh(&["s", "ls", "--json"]);
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`--json` is one document");

    assert_eq!(
        doc["leftovers"],
        serde_json::json!(["s03"]),
        "the fact is a field"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "",
        "and the prose about it is gone"
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

/// **`--json` emits exactly one document, from every command that emits any.**
///
/// The bug this exists to stop is invisible to a unit test by construction.
/// Every `Report::json` in `src/report.rs` is exercised by calling `.json()` on
/// a hand-built value, which can only ever produce one object — but a *command*
/// chooses how many times to call `Ctx::say`, and four of them called it inside
/// a loop over the repo layers. Two layers, two objects, concatenated: valid
/// JSON twice over and a parse error once, in the format whose entire purpose
/// is to be parsed.
///
/// This is the file's stated reason for existing, in the module docs above — a
/// guard that is correct while the wiring reaching it is wrong.
///
/// Counted by closing braces in column zero, which is exact for
/// `serde_json::to_string_pretty`: a top-level object closes there and nothing
/// nested does. Cruder than a parser and it needs no dependency the test target
/// does not already have.
#[test]
fn every_json_answer_is_one_document_and_not_several() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/alpha/SKILL.md", "skills/beta/SKILL.md"]);
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();

    // Both repo layers declare the same capability, which is what makes the
    // writers loop. A repo with one layer passes whatever the command does.
    std::fs::write(
        sb.repo.join(".omh/settings.toml"),
        "[use]\nskills = [\"alpha\"]\n\n[omh]\ncodegraph = true\n",
    )
    .unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "[use]\nskills = [\"alpha\"]\n\n[omh]\ncodegraph = true\n",
    )
    .unwrap();

    for args in [
        vec!["--json", "ls"],
        vec!["--json", "repo"],
        vec!["--json", "config"],
        vec!["--json", "use", "skills", "beta"],
        vec!["--json", "unuse", "skills", "beta"],
        vec!["--json", "repo", "disable", "codegraph"],
        vec!["--json", "s", "ls"],
        vec!["--json", "memory", "ls"],
    ] {
        let out = sb.omh(&args);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        if stdout.trim().is_empty() {
            continue; // a command with nothing to say is not this test's business
        }
        let documents = stdout.lines().filter(|l| *l == "}").count();
        assert_eq!(
            documents,
            1,
            "`omh {}` emitted {documents} JSON documents, and a parser reads one:\n{stdout}",
            args.join(" ")
        );
    }
}

/// The reap has to be wired to the build, or the whole feature is a set of
/// well-tested functions nobody calls.
///
/// `superseded` is pure and thoroughly tested, and every one of those tests
/// stays green with the `reap` call deleted from `build` — measured: the unit
/// suite, `tests/cli.rs` under `--include-ignored`, and the doc tests all pass
/// with the feature disconnected, leaving four dead-code warnings as the only
/// signal. That is the original bug in its original form: images stop being
/// collected, nothing says so, and it is invisible until a disk fills.
///
/// Driven through `omh init` with a `docker` that reports nothing built, so a
/// real build runs and the reap after it is reached. No container runtime is
/// involved: what is asserted is which removals omh *asks* for, which is omh's
/// half of the bargain. Whether docker honours them is docker's, and no test
/// here can settle it.
#[test]
fn a_build_asks_docker_to_remove_the_tags_it_replaced() {
    let sb = sandbox();
    sb.git_init();
    let log = sb.fake_docker_with_nothing_built(
        &["omh/base:stale", "omh/base:latest", "omh/base:held"],
        &["omh/base:held"],
    );

    // Asked, not assumed. This has failed on CI with an empty removal list,
    // which is what a build that never ran looks like from here — and the
    // discarded status meant the message said nothing about why. Every
    // neighbouring test in this file already checks it.
    //
    // Both streams and the code, because the first version of this printed
    // stderr alone and the next failure put nothing there but progress lines:
    // a refusal with no reason, which is the state this whole file exists to
    // stop omh from producing. It has since failed on the linux runner three
    // times across three branches — including before any of the work that was
    // in flight when it first appeared — and no run has yet said why.
    let init = sb.omh(&["init"]);
    assert!(
        init.status.success(),
        "init failed ({}), so no build ran and no reap followed it\n\
         --- stderr ---\n{}\n--- stdout ---\n{}\n--- docker was asked ---\n{}",
        init.status,
        String::from_utf8_lossy(&init.stderr),
        String::from_utf8_lossy(&init.stdout),
        sb.docker_calls(&log).join("\n")
    );

    let removals: Vec<String> = sb
        .docker_calls(&log)
        .into_iter()
        .filter(|c| c.starts_with("image rm "))
        .collect();
    assert!(
        removals.iter().any(|c| c.ends_with("omh/base:stale")),
        "the build never asked for the tag it replaced: {removals:?}"
    );
    assert!(
        !removals.iter().any(|c| c.ends_with("omh/base:latest")),
        "`latest` is the one removal that cannot be undone: {removals:?}"
    );
    assert!(
        !removals.iter().any(|c| c.ends_with("omh/base:held")),
        "a container still references it: {removals:?}"
    );
}
