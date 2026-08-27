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
    // `$EDITOR`, for every run. `edit` spawns it and falls back to `vi`,
    // which in a test harness waits forever — and the developer's own
    // `$EDITOR` leaking in made every `edit` test a different test on every
    // machine. This one records the path it was asked to open, so *which
    // file* is a thing a test can assert rather than something only the
    // person watching their screen finds out.
    let editor = bin.join("fake-editor");
    std::fs::write(
        &editor,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
            bin.join("editor.log").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    Sandbox {
        _dir: dir,
        repo,
        home,
        bin,
    }
}

impl Sandbox {
    fn omh(&self, args: &[&str]) -> Output {
        self.omh_in(&self.repo.clone(), args)
    }

    /// Run somewhere other than the sandbox repo — outside one, for the
    /// commands whose subject is a file that does not live in a repository.
    fn omh_in(&self, cwd: &Path, args: &[&str]) -> Output {
        let path = match std::env::var("PATH") {
            Ok(rest) => format!("{}:{rest}", self.bin.display()),
            Err(_) => self.bin.display().to_string(),
        };
        Command::new(env!("CARGO_BIN_EXE_omh"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("EDITOR", self.bin.join("fake-editor"))
            .output()
            .expect("the binary under test must run")
    }

    /// An editor on the sandbox's PATH that records the argv it was given.
    ///
    /// Without one, `omh s attach zed` finds the **real** Zed: `runtime::installed`
    /// asks PATH, the sandbox only *prepends* to PATH, and the launch is an
    /// ordinary `Command::new("zed")`. Running the suite opened windows on the
    /// developer's desktop pointing at `omh-repo-s01` — a host alias that
    /// exists only inside a `TempDir` that is deleted seconds later, so each
    /// one failed to resolve and reported it as Zed's error.
    ///
    /// The log is the assertion worth making anyway: stdout says which session
    /// omh *reported* opening, and this says which one it actually passed.
    fn fake_editor(&self, name: &str) -> PathBuf {
        let log = self.bin.join(format!("{name}.log"));
        let shim = self.bin.join(name);
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        log
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
    /// `docker-probe-refuses` fails only the `--pull=never` run, which is the
    /// facts probe and nothing else — the predicate run that decides which
    /// stack provides apply carries no such flag. That is the narrowest of the
    /// three, and it is the one whose failure `measure` reports and swallows:
    /// the answers stay as they were, which reads as *nobody has looked*.
    ///
    /// Writing `docker-refuses` into the bin directory makes the shim exit
    /// non-zero, which is how a runtime that cannot be reached is reachable
    /// from a test at all. `docker-exec-refuses` fails only `exec`, which is
    /// the narrower thing the launch path needs: a runtime that answers *the
    /// container is running* and then will not let omh in. Everything
    /// destructive in a launch hangs off telling those two apart.
    fn fake_docker(&self) -> PathBuf {
        let log = self.bin.join("docker.log");
        let shim = self.bin.join("docker");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
                 [ -f {refuses} ] && {{ echo 'cannot connect to the daemon' >&2; exit 1; }}\n\
                 if [ \"$1\" = exec ] && [ -f {exec_refuses} ]; then \
                 cat {exec_refuses} >&2; exit 1; fi\n\
                 case \"$*\" in *--pull=never*) [ -f {probe_refuses} ] && {{ \
                 echo 'no such image' >&2; exit 125; }} ;; esac\n\
                 if [ \"$1\" = inspect ]; then echo true; fi\n\
                 if [ \"$1\" = ps ]; then cat {containers} 2>/dev/null; fi\nexit 0\n",
                log = log.display(),
                refuses = self.bin.join("docker-refuses").display(),
                exec_refuses = self.bin.join("docker-exec-refuses").display(),
                probe_refuses = self.bin.join("docker-probe-refuses").display(),
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

    /// What `$EDITOR` was asked to open, across every run in this sandbox.
    fn opened_in_the_editor(&self) -> String {
        std::fs::read_to_string(self.bin.join("editor.log")).unwrap_or_default()
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

/// The shipped hook body, with the guest's paths swapped for a fixture's.
///
/// Written here rather than imported: this file drives the binary and cannot
/// reach `shadow::turn_hook_command`. The unit test in `shadow.rs` runs the
/// real function; this only gets a fixture a snapshot to look at.
fn omh_turn_hook_body(gitdir: &std::path::Path, worktree: &std::path::Path) -> String {
    let g = format!(
        "git -C {w} --git-dir={g} --work-tree={w}",
        w = worktree.display(),
        g = gitdir.display()
    );
    format!(
        "{{ i={i}; GIT_INDEX_FILE=$i {g} read-tree HEAD && GIT_INDEX_FILE=$i {g} add -A \
         && t=$(GIT_INDEX_FILE=$i {g} write-tree) \
         && p=$({g} rev-parse -q --verify refs/omh/turn || true) \
         && if [ -n \"$p\" ] && [ \"$({g} rev-parse \"$p^{{tree}}\")\" = \"$t\" ]; then :; \
         else c=$({g} commit-tree \"$t\" ${{p:+-p}} ${{p:+\"$p\"}} -m \"turn end\") \
         && {g} update-ref refs/omh/turn \"$c\"; fi; }} >/dev/null 2>&1 || true",
        i = gitdir.join("omh-turn.index").display()
    )
}

/// `omh sNN rm` names the snapshots it is about to delete.
///
/// The wiring, not the decision: `may_remove` decides and is unit-tested with
/// a count handed to it, so nothing proved that `rm` asks for a real one.
/// Replacing that call with a literal `0` left the whole suite green.
#[test]
fn rm_names_the_snapshots_it_takes_and_force_still_removes() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    sb.sandbox_repo_with_unkept_work("s01", &worktree);
    let gitdir = sb
        .home
        .join(".omh/shadow")
        .join(sb.repo.file_name().unwrap())
        .join("s01.git");

    std::fs::write(worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();
    Command::new("sh")
        .arg("-c")
        .arg(omh_turn_hook_body(&gitdir, &worktree))
        .output()
        .expect("sh must be installed");

    let refused = sb.omh(&["s01", "rm"]);
    assert!(!refused.status.success(), "unharvested work still refuses");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("1 turn snapshot"),
        "and the count is a real one, asked of the sandbox: {said}"
    );
    assert!(
        said.contains("omh s01 log --turns"),
        "with the command that reads them: {said}"
    );

    assert!(
        sb.omh(&["s01", "rm", "--force"]).status.success(),
        "and `--force` still means it"
    );
}

/// `omh sNN log --turns` reads omh's own snapshots, and the default view does
/// not show them.
///
/// End to end because the separation is the design: one parser, two views, two
/// commands. The unit tests decide what each says; this decides that asking
/// for one never hands you the other — and that a snapshot sitting in the
/// sandbox does not make the ordinary `log` start warning about work on no
/// branch, which is what all three ref-walking guards would have done.
#[test]
fn log_turns_reads_the_snapshots_and_the_default_view_does_not() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    sb.sandbox_repo_with_unkept_work("s01", &worktree);
    let gitdir = sb
        .home
        .join(".omh/shadow")
        .join(sb.repo.file_name().unwrap())
        .join("s01.git");

    std::fs::write(worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();
    let ran = Command::new("sh")
        .arg("-c")
        .arg(omh_turn_hook_body(&gitdir, &worktree))
        .output()
        .expect("sh must be installed");
    assert!(ran.status.success(), "the hook never fails a turn: {ran:?}");

    let turns = sb.omh(&["s01", "log", "--turns"]);
    let printed = String::from_utf8_lossy(&turns.stdout);
    assert!(
        printed.contains("1 turn"),
        "the snapshot is there to read: {printed}"
    );

    // The default view is untouched, and — the part that matters — nothing
    // warns. All three guards would have called this snapshot stranded work.
    let plain = sb.omh(&["s01", "log"]);
    let out = String::from_utf8_lossy(&plain.stdout);
    let err = String::from_utf8_lossy(&plain.stderr);
    // Anchored: every assertion below is negative, and a `log` that exited 1
    // with empty stdout would satisfy all of them.
    assert!(plain.status.success(), "the default log works: {err}");
    assert!(
        out.contains("agent"),
        "and lists the agent's own commit: {out}"
    );
    assert!(
        !out.contains("turn end"),
        "omh's own snapshot is not in the agent's list: {out}"
    );
    assert!(
        !err.contains("on no branch") && !out.contains("on no branch"),
        "and nothing calls it work the agent stranded: {out}{err}"
    );
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
    // fact about the session, which `omh s` and `omh s diff` answer — this
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
/// the paths come from a `status --porcelain` that `omh s` already ran for its
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

    let printed = String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();

    assert!(
        printed.contains("s01 and s02 both change shared.rs"),
        "the file both are changing, and who: {printed}"
    );
    assert!(
        !printed.contains("only-in-s01.rs"),
        "and nothing about what only one of them touches: {printed}"
    );

    // A collision is a fact about *two* sessions, so it has to survive being
    // asked about one of them — it is the most useful line on that screen.
    // This is also why the focused view still reads every session: the other
    // one's paths are what make the line sayable.
    let focused = String::from_utf8_lossy(&sb.omh(&["s01"]).stdout).to_string();
    assert!(
        focused.contains("s01 and s02 both change shared.rs"),
        "a collision involving s01 survives the focus: {focused}"
    );

    // Part of the answer rather than an aside: this is the most consequential
    // line in a record of what is in flight, and stderr is not where a
    // redirected listing keeps it.
    assert!(
        !String::from_utf8_lossy(&sb.omh(&["s"]).stderr).contains("both change"),
        "it is the answer, not a warning"
    );

    // The document says what the sentence says. `--json` is the scripting
    // contract, and deleting the field entirely left every other assertion
    // here green.
    let doc: serde_json::Value = serde_json::from_slice(&sb.omh(&["s", "--json"]).stdout).unwrap();
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

    let out = sb.omh(&["s"]);
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
/// `omh s` looks for: a container is re-creatable and a run directory holds a
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

    let out = sb.omh(&["s"]);
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
        "the hint `omh s` prints has to be a command that clears it"
    );
    assert!(!orphan.exists(), "and it did");
    assert!(
        !String::from_utf8_lossy(&sb.omh(&["s"]).stderr).contains("s09"),
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
    assert!(run.exists(), "so was the marker `omh s` reads");
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
/// codebase refuses when a session is named two ways at once.
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

    let printed = String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();

    assert!(printed.contains("to push"), "got: {printed}");
}

/// `omh s` is where every one of these measurements is actually read, and the
/// rendering is the part no unit test reaches. Each state is one the loop sits
/// in, not one it passes through, so a blank column is a wrong answer rather
/// than a missing one.
#[test]
fn the_listing_renders_each_state_a_session_can_sit_in() {
    let sb = sandbox();
    let worktree = sb.session("s01");
    let ls = || String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();

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

    let printed = String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();

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

    let out = sb.omh(&["--dry-run", "new", "claude"]);
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

/// `omh new` starts a session rather than resuming one.
///
/// Today the bare name resumes the most recent session and `--new` is how you
/// ask for a fresh one, which means the common case is a flag and the flag is
/// global — it can be typed anywhere, including after a session prefix that
/// contradicts it. `omh new claude` is the verb for the thing the flag did.
///
/// This asserts the *invariant*, not an id: the session it lands in is not one
/// that already existed. Pinning `s02` would be pinning `next_id`'s format,
/// which is a different claim and one this test has no business making.
///
/// `#[ignore]`d because it needs git and a container runtime to reach `run()`.
/// CI's linux job runs `--include-ignored`, which is where this bites.
#[test]
#[ignore]
fn a_new_launch_never_lands_in_a_session_that_already_exists() {
    let sb = sandbox();
    sb.git_init();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );
    let existing = sb.session("s01");
    let already = existing.file_name().unwrap().to_string_lossy().to_string();

    let out = sb.omh(&["--dry-run", "new", "claude"]);
    let plan = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "`omh new` is a command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !plan.contains(&format!("/{already}")),
        "`new` landed in the session that was already there: {plan}"
    );

    // …and rejoining still lands in the one that was already there. The bare
    // name used to be this half; it is not a launch any more, so the assertion
    // moved onto the verb that replaced it.
    let resumed =
        String::from_utf8_lossy(&sb.omh(&["s01", "--dry-run", "resume", "claude"]).stdout)
            .to_string();
    assert!(
        resumed.contains(&format!("/{already}")),
        "resume lands in the session it names: {resumed}"
    );
}

/// Asking for a fresh session and naming one is refused, not resolved.
///
/// `omh s01 --new claude` said two contradictory things — *this exact session*
/// and *one that does not exist yet* — and omh resolved it by returning `s01`
/// and never looking at the flag. Exit 0, a plan, no warning. `--new` carried
/// `conflicts_with = "session"`, which never fired for the spelling people
/// type: clap checks that against the `--session` flag, and the `sNN` prefix
/// lands in `cli.session` after the parse.
///
/// The flag is deleted, so that line no longer parses — but asserting *that*
/// would only prove clap rejects an unknown argument, which the compiler
/// already proves. The contradiction is still spellable, as `omh s01 new
/// claude`, and that is what this checks: refused by name, not resolved by
/// picking one.
#[test]
fn asking_for_a_fresh_session_while_naming_one_is_refused() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.session("s01");

    let out = sb.omh(&["s01", "new", "claude"]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !out.status.success(),
        "omh answered a line that asked for two different sessions: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        err.contains("s01"),
        "the refusal names the scope it could not honour: {err}"
    );

    // And the session it was pointed at is untouched — a refusal that had
    // already launched would be worse than the silence it replaces.
    assert!(
        !sb.home.join(".omh/run/repo/s01/.harness").exists(),
        "it launched anyway"
    );
}

/// A bare word is not a harness any more.
///
/// A bare word was a launch because `Cmd::Run` swallowed any word omh did not
/// recognise. That one arm is why `RESERVED` existed — nineteen names written
/// out so an adapter could not shadow a command — and why `session_prefix` had
/// to parse the line twice and arbitrate, which is how `omh s01 ls` once became
/// the top-level inventory with the session dropped.
///
/// `omh new claude` is the spelling now, and a word omh does not know is a
/// mistake rather than a launch.
#[test]
fn a_bare_word_is_a_mistake_rather_than_a_launch() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.session("s01");

    for word in ["claude", "clyde"] {
        let out = sb.omh(&[word]);
        assert!(
            !out.status.success(),
            "`omh {word}` is not a command: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    // The verb still works, so this removed a spelling rather than the ability.
    let out = sb.omh(&["--dry-run", "new", "claude"]);
    assert!(
        out.status.success(),
        "`omh new claude` still launches: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A launch records what it launched, and a dry run records nothing.
///
/// This is the line the whole feature rests on, and it was very nearly
/// untested. A first pass asserted it by grepping the source, on the belief
/// that a non-dry launch needed a container. It does not: everything before
/// the write is satisfied by the fake runtime, and `a_launch_that_cannot_read_
/// the_probe_removes_nothing` in this same file has been performing one all
/// along. Deleting the write left the whole suite green.
///
/// Not `#[ignore]`d, so it runs on both CI jobs rather than linux alone.
#[test]
fn a_launch_records_the_harness_it_started_and_a_dry_run_does_not() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.session("s01");
    let marker = sb.home.join(".omh/run/repo/s01/.harness");

    // The launch is allowed to fail after the write — what is under test
    // happened before it got there. Asserting the file rather than the exit
    // code is what makes that safe.
    let _ = sb.omh(&["s01", "resume", "opencode"]);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "opencode",
        "a launch did not write down what it launched"
    );

    // A relaunch records what it launched, not what it launched last time.
    let _ = sb.omh(&["s01", "resume", "claude"]);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "claude",
        "the record is of the last launch, not the first"
    );

    // A dry run leaves no trace — of this or of anything else.
    let dry = sandbox();
    let _log2 = dry.fake_docker();
    dry.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    dry.session("s01");
    let _ = dry.omh(&["s01", "--dry-run", "resume", "opencode"]);
    assert!(
        !dry.home.join(".omh/run/repo/s01/.harness").exists(),
        "a dry run recorded a harness, so the next resume rejoins a session \
         that never ran"
    );
}

/// A resumed session runs the harness it ran before.
///
/// `omh new` creates and `omh s resume` rejoins, which needs a fact almost
/// nothing records. The staging directory does carry the name —
/// `runs()/<id>/<harness>` survives `down` — but a session that has run two
/// harnesses has two of them and no way to say which was last, and staging is
/// a side effect nothing promises to keep.
///
/// Guessing is the wrong answer and the tempting one: `detect::preferred_harness`
/// is right there and would return something plausible every time. It would
/// also silently attach claude to a session an afternoon of opencode built.
///
/// `#[ignore]`d because `omh init` needs a container runtime.
#[test]
#[ignore]
fn a_resumed_session_runs_the_harness_it_ran_before() {
    let sb = sandbox();
    sb.git_init();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );
    sb.session("s01");

    // Recorded by a real launch rather than by hand, so this test also fails
    // if the two halves ever disagree about where the marker lives.
    let _ = sb.omh(&["s01", "resume", "opencode"]);

    let out = sb.omh(&["s01", "resume", "--dry-run"]);
    let plan = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "resume is a verb: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        plan.contains("opencode"),
        "it rejoined as something else: {plan}"
    );
    assert!(
        plan.contains("/s01"),
        "and it rejoined the session that was named: {plan}"
    );
}

/// `resume <harness>` overrides the record, and only after it launches.
///
/// Added late, to replace `omh s01 claude` — which the bare-name deletion took
/// away and which was the remedy the refusal below offers. It went in without
/// a test of its own, and two of its edges are the sort that only a test finds.
///
/// The record is written where the launch is known to have happened. Written
/// earlier, a `resume opencode` that failed at `runtime::select` still left the
/// session recorded as opencode, so the next bare `resume` rejoined a claude
/// worktree as opencode — the exact harm the refusal below exists to prevent,
/// produced by the thing meant to prevent it.
#[test]
fn resuming_as_another_harness_records_it_only_if_it_launched() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.session("s01");
    let marker = sb.home.join(".omh/run/repo/s01/.harness");

    let _ = sb.omh(&["s01", "resume", "claude"]);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "claude",
        "resume with a name records it"
    );

    // Arguments still reach the harness through the separator, the same way
    // `omh new` passes them. This block is a copy of `new`'s, which is exactly
    // where a missing separator would hide.
    let out = sb.omh(&["s01", "--dry-run", "resume", "claude", "--", "--json"]);
    let plan = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        plan.contains("--json"),
        "the harness never got what followed the separator: {plan}"
    );
    assert!(
        !plan.trim_start().starts_with('{'),
        "and omh did not take it for itself: {plan}"
    );

    // A launch that does not happen does not rewrite history.
    //
    // An unusable `runtime` and not a missing shim, because this has to fail
    // in one particular window: after the staging block, where the record used
    // to be written, and before the container comes up. `docker-refuses` fails
    // earlier than that and `docker-exec-refuses` later — with either, the
    // assertion below passes whichever side of the launch the write sits on,
    // which is a test that cannot fail for its own reason. Deleting the shim
    // would work only on a machine with no docker of its own.
    assert!(sb.omh(&["set", "runtime", "bogus"]).status.success());
    let failed = sb.omh(&["s01", "resume", "opencode"]);
    assert!(!failed.status.success(), "the launch failed");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "claude",
        "a failed resume rewrote the record, so the next one rejoins as a \
         harness that never ran here"
    );
}

/// Resuming as a *different* harness says that is what it is doing.
///
/// `resume` with a name is two operations wearing one word. With no name, or
/// with the one already recorded, it rejoins a session as it was. With a
/// different one it is a switch: an image is built per harness, so the sandbox
/// stops and starts on the other image, and the record is rewritten so every
/// later `resume` follows.
///
/// Both are wanted — switching harness inside one session is a feature, and
/// the docs describe it. What is not wanted is doing the second while the word
/// says the first. Said out loud, on stderr, so `omh s01 resume claude > log`
/// still shows it.
#[test]
fn resuming_as_a_different_harness_says_it_is_a_switch() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.session("s01");
    let _ = sb.omh(&["s01", "resume", "opencode"]);

    // The same one again is a resume, and says nothing.
    let same = sb.omh(&["s01", "--dry-run", "resume", "opencode"]);
    let quiet = String::from_utf8_lossy(&same.stderr).to_string();
    assert!(
        !quiet.contains("was running"),
        "rejoining as what it already ran is not a switch: {quiet}"
    );

    // A different one is.
    let switched = sb.omh(&["s01", "--dry-run", "resume", "claude"]);
    let said = String::from_utf8_lossy(&switched.stderr).to_string();
    assert!(
        said.contains("opencode") && said.contains("claude"),
        "a switch has to name both ends of it: {said}"
    );
    assert!(
        said.contains("was running"),
        "and say the session is being changed rather than rejoined: {said}"
    );
}

/// A session omh cannot name a harness for is refused, not guessed at.
///
/// Every session made before this release is in that state, and so is one
/// `omh s attach` created for an editor without ever running a harness in it.
/// `detect::preferred_harness` would answer anyway, and the answer would look
/// exactly like a correct one — the shape this whole release has been removing.
///
/// The remedy has to be a command that rejoins *this* session. An earlier
/// version offered `omh new <harness>`, which starts a different one, and is
/// itself refused when applied to a session.
///
/// `#[ignore]`d because `omh init` needs a container runtime.
#[test]
#[ignore]
fn a_session_omh_cannot_name_a_harness_for_is_refused_rather_than_guessed() {
    let sb = sandbox();
    sb.git_init();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );
    sb.session("s01");

    let out = sb.omh(&["s01", "resume"]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !out.status.success(),
        "a guess is not an answer: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(err.contains("s01"), "the refusal names the session: {err}");
    assert!(
        err.contains("s01 resume <harness>"),
        "and offers a command that rejoins this session rather than leaving \
         it: {err}"
    );
}

/// `--` is how a flag reaches the harness under `omh new`, and the only way.
///
/// The bare-name form had to guess: a `--json` after the harness might be omh's
/// or the harness's, and `passthrough` decided by refusing omh's long flags and
/// leaving shorts alone — a judgement about which mistake is likelier, which
/// its own comment admits.
///
/// `omh new` does not guess: before `--` is omh's, after it is the harness's,
/// and there is no third category. That makes the escape hatch load-bearing
/// rather than a curiosity, so it is tested end to end rather than only at the
/// parser — the parser was already right, and the argv rebuilt for the launch
/// was dropping the separator on the floor.
///
/// `#[ignore]`d because it needs git and a container runtime to reach `run()`.
/// CI's linux job runs `--include-ignored`, which is where this bites.
#[test]
#[ignore]
fn omh_new_hands_the_harness_what_follows_a_double_dash() {
    let sb = sandbox();
    sb.git_init();
    assert!(
        sb.omh(&["init"]).status.success(),
        "init must set the repo up"
    );

    // A flag omh also has. After `--` it is unambiguously the harness's, and
    // this is the case the bare-name form cannot express without `--` either.
    let out = sb.omh(&["--dry-run", "new", "claude", "--", "--json"]);
    let plan = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "`--` is not a refusal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        plan.contains("--json"),
        "claude never got the flag that was handed to it: {plan}"
    );
    assert!(
        !plan.trim_start().starts_with('{'),
        "and omh did not take it for itself — this is a human report: {plan}"
    );

    // Before the separator it is omh's. Not a bug to fix later: it is what
    // makes the separator mean anything.
    let mine = sb.omh(&["--dry-run", "new", "claude", "--json"]);
    let doc = String::from_utf8_lossy(&mine.stdout).to_string();
    assert!(
        doc.trim_start().starts_with('{'),
        "`--json` before `--` is omh's: {doc}"
    );

    // And an option omh does not have is a mistake, not a silent forward.
    // Without this, `omh new claude --resume x` looks like it worked while
    // `--resume` reached a harness that was never told about it.
    let stray = sb.omh(&["new", "claude", "-x"]);
    assert!(
        !stray.status.success(),
        "an option before `--` has to be omh's or an error: {}",
        String::from_utf8_lossy(&stray.stdout)
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

    let said = String::from_utf8_lossy(&sb.omh(&["--dry-run", "new", "claude"]).stderr).to_string();
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
/// the project, and a teammate cloning it should get the same selection —
/// which is the same default `omh set` now has, for the same reason. The
/// values that must *not* be committable by accident are `carry_in` paths and
/// MCP env, and since 0.7.0 that is a property of the key rather than of
/// which command you reached for.
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
        "the gitignored file is where `--local` and a credential-bearing key \
         go, and a selection is neither"
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
fn use_refuses_a_feature_and_set_refuses_a_catalogue_entry() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md"]);

    let out = sb.omh(&["use", "mcp", "codegraph"]);
    assert!(!out.status.success(), "codegraph is omh's, not yours");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("codegraph"), "must name it: {err}");
    assert!(
        err.contains("omh set codegraph off"),
        "and point at the switch that does work: {err}"
    );

    // And the other direction, so the distinction is not one-way.
    let out = sb.omh(&["set", "review-diff", "off"]);
    assert!(
        !out.status.success(),
        "a catalogue entry is selected, not set"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("omh use"), "point back the other way: {err}");
}

/// `omh set` writes `[omh]` in the committed file, and says plainly
/// that nothing was uninstalled — the distinction the whole feature rests on.
#[test]
fn switching_a_feature_off_here_uninstalls_nothing() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["set", "codegraph", "off"]);
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

    assert!(sb.omh(&["set", "codegraph", "on"]).status.success());
    assert!(
        sb.settings().contains("codegraph = true"),
        "{}",
        sb.settings()
    );
}

/// The key decides the file, and asking for the other one says so out loud.
///
/// It read *two opposite defaults, side by side* when the two halves were two
/// commands. They are one command now, and the first half passes for a
/// different reason than it used to: `carry_in` reaches the gitignored file
/// because the registry classifies it, not because this spelling defaults
/// there.
#[test]
fn the_key_picks_the_file_and_save_says_when_it_is_overridden() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "carry_in", "[\".env\"]"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(local.contains(".env"), "got: {local}");

    let out = sb.omh(&["set", "--save", "idle_timeout", "30m"]);
    assert!(out.status.success());
    assert!(sb.settings().contains("30m"), "{}", sb.settings());
    // On **stderr**, and that is the stronger place for it. This warning is
    // the last thing standing between somebody and a token in git history,
    // and the invocation where that actually happens is the scripted one —
    // `omh set --save … > log`, where anything on stdout goes to the
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

/// `omh set` reads the key registry to decide which file it writes.
///
/// The claim is about **both** files, deliberately. A test asserting only that
/// the value landed where it belongs would pass just as well if `omh set` wrote
/// every layer — and a `carry_in` in the committed file is a map to a secret
/// whether or not the gitignored one also has it.
#[test]
fn set_asks_the_registry_which_file_a_key_belongs_in() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "carry_in", "[\".env\"]"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains(".env"),
        "a key that can name a credential goes to the gitignored file: {local}"
    );
    assert!(
        !sb.settings().contains(".env"),
        "and nowhere near the committed one: {}",
        sb.settings()
    );

    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());
    assert!(
        sb.settings().contains("30m"),
        "a key holding nothing secret is a fact about the project: {}",
        sb.settings()
    );
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        !local.contains("30m"),
        "and it is written once, not to both: {local}"
    );
}

/// A key the gitignored file already carries is updated **there**.
///
/// `settings::resolve` applies the layers with the local one last, so writing
/// the committed file while a local value stands reports success over a value
/// that never changed. That is the defect `omh unuse` shipped once already —
/// same shape, one command over.
#[test]
fn set_updates_a_key_where_it_already_lives() {
    let sb = sandbox();
    sb.seed_base();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "idle_timeout = \"15m\"\n",
    )
    .unwrap();

    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("30m") && !local.contains("15m"),
        "got: {local}"
    );
    assert!(
        !sb.settings().contains("30m"),
        "and not a second copy in the file the local one outranks: {}",
        sb.settings()
    );
}

/// `--shared` is how you say you meant it, and it still says what that costs.
#[test]
fn set_shared_forces_the_committed_file_and_names_the_key() {
    let sb = sandbox();
    sb.seed_base();
    let out = sb.omh(&["set", "--save", "carry_in", "[\".env\"]"]);
    assert!(out.status.success());
    assert!(sb.settings().contains(".env"), "{}", sb.settings());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("COMMITTED"), "got: {err}");
    assert!(
        err.contains("carry_in"),
        "a warning that cannot name the key is one people learn to scroll past: {err}"
    );
}

/// The COMMITTED warning stays rare, now that committed is the default.
///
/// Under `omh set` a committed write was something you asked for, so the
/// sentence was rare and it meant something. `omh set` defaults there, so
/// warning on every committed write would put it on nearly every invocation —
/// and this project has already watched that exact sentence stop being read
/// once, when it could not tell `account` from `carry_in`.
///
/// So the claim is two-sided, and the quiet half is the one that matters: a
/// warning that fires everywhere would pass a test that only checked it fires.
#[test]
fn set_mentions_git_where_git_was_asked_for_or_cannot_be_vouched_for() {
    let sb = sandbox();
    sb.seed_base();

    // Committed because the registry says this key is safe there. The design,
    // not a hazard — and it says nothing alarming.
    let designed = sb.omh(&["set", "idle_timeout", "30m"]);
    let quiet = String::from_utf8_lossy(&designed.stderr).to_string();
    assert!(designed.status.success(), "{quiet}");
    assert!(
        !quiet.contains("COMMITTED"),
        "the default path must not carry the warning, or nothing does: {quiet}"
    );

    // Asked for in so many words.
    // A captured login, because `omh set account` now refuses a name none
    // answers to — `account` is still the example here for the reason the
    // key registry gives it: a name, not a credential, and the one
    // value-taking key the docs show being shared on purpose.
    sb.seed_catalogue(&["adapters"]);
    sb.account("claude", "work");
    let asked = sb.omh(&["set", "--save", "account", "work"]);
    let said = String::from_utf8_lossy(&asked.stderr).to_string();
    assert!(
        said.contains("COMMITTED"),
        "typing --shared is asking, and asking gets an answer: {said}"
    );

    // omh routed a key it has never heard of into a file git carries. It
    // cannot vouch for the value, and that is the half retyping does not undo.
    let unknown = sb.omh(&["set", "carry_ins", "[\".env\"]"]);
    let flagged = String::from_utf8_lossy(&unknown.stderr).to_string();
    assert!(
        flagged.contains("COMMITTED") && flagged.contains("carry_ins"),
        "an unclassified key reaching git is named, with where it went: {flagged}"
    );
}

/// `--local` writes the gitignored file, whatever the key is for.
///
/// The mirror of `--save`, and the reason both exist: the rule decides well,
/// and somebody who disagrees with it about one key needs a way to say so
/// without arguing with a table.
#[test]
fn local_writes_the_gitignored_file_whatever_the_key_is() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--local", "idle_timeout", "45m"])
        .status
        .success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("45m"),
        "a key the rule would have committed goes where you said: {local}"
    );
    assert!(
        !sb.settings().contains("45m"),
        "and not also to the file it would have chosen: {}",
        sb.settings()
    );
}

/// `omh unset` reaches the file `omh set` chose, without being told which.
///
/// Both halves read the same rule, so the pair is symmetric by construction
/// rather than by two lists agreeing.
#[test]
fn unset_reaches_the_file_set_wrote() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "carry_in", "[\".env\"]"]).status.success());
    assert!(sb.omh(&["unset", "carry_in"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(!local.contains(".env"), "got: {local}");

    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());
    assert!(sb.omh(&["unset", "idle_timeout"]).status.success());
    assert!(!sb.settings().contains("30m"), "got: {}", sb.settings());
}

/// `omh unset` reaches a committed value it did not write.
///
/// The defect this replaces: `omh set --shared carry_in` then `omh set
/// carry_in` leaves the key in both repo files — omh wrote both, no hand
/// editing — and `unset` consulted `set`'s rule, removed the gitignored copy,
/// said so, exited 0, and left a map to a credential standing in the file git
/// carries. The command a person runs to get a secret out of git reported
/// success and did not do it.
#[test]
fn unset_removes_a_credential_key_from_the_committed_file_too() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--save", "carry_in", "[\".env.shared\"]"])
        .status
        .success());
    assert!(sb
        .omh(&["set", "--local", "carry_in", "[\".env.local\"]"])
        .status
        .success());
    // Both files, which the two flags are the only way to reach — an
    // unadorned write joins whichever layer already holds it rather than
    // splitting the value across two.
    assert!(sb.settings().contains(".env.shared"), "{}", sb.settings());

    assert!(sb.omh(&["unset", "carry_in"]).status.success());
    assert!(
        !sb.settings().contains("carry_in"),
        "the committed copy is what git carries, so it is the one that had to \
         go: {}",
        sb.settings()
    );
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(!local.contains("carry_in"), "got: {local}");

    // The claim is about what omh *reads*, not about two files' bytes.
    let after = sb.omh(&["info", "--repo"]);
    // The status first, because everything below it is a *negative* on this
    // command's stdout — and a command that failed has no stdout at all, so
    // an argv this test spelled wrong would read as a removal that worked.
    assert!(
        after.status.success(),
        "the report this claim is read from must run: {}",
        String::from_utf8_lossy(&after.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&after.stdout).contains("carry_in"),
        "a removal that leaves the value effective is not a removal: {}",
        String::from_utf8_lossy(&after.stdout)
    );
}

/// A value `--save` or `--local` left behind is named, not passed over.
///
/// Without a flag the rule reaches every repo layer holding the key, so an
/// unadorned `unset` leaves nothing behind. A named flag touches one file on
/// purpose — and silence then leaves somebody watching a setting survive the
/// command whose whole purpose was removing it.
#[test]
fn unset_says_what_still_supplies_the_value() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--save", "idle_timeout", "1h"])
        .status
        .success());
    assert!(sb
        .omh(&["set", "--local", "idle_timeout", "15m"])
        .status
        .success());

    let out = sb.omh(&["unset", "--local", "idle_timeout"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("shared") && said.contains("idle_timeout"),
        "the layer still supplying it has to be named: {said}"
    );
}

/// `--save` and `--local` mean that file on `unset`, and not each other.
///
/// Transposing the two arguments at the dispatch site passed the whole suite
/// once already: `omh unset --save carry_in` would delete from the gitignored
/// file and report the wrong layer, leaving the committed `carry_in` in place.
#[test]
fn unset_honours_the_layer_it_was_given() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--local", "idle_timeout", "45m"])
        .status
        .success());
    assert!(sb
        .omh(&["set", "--save", "idle_timeout", "30m"])
        .status
        .success());

    assert!(sb
        .omh(&["unset", "--save", "idle_timeout"])
        .status
        .success());
    assert!(
        !sb.settings().contains("30m"),
        "--save names the committed file: {}",
        sb.settings()
    );
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("45m"),
        "and it is not the gitignored one: {local}"
    );
}

/// `unset` reports absence as absence, and does not claim a removal.
///
/// `let removed = { config::unset(…)?; true }` passed everything — no test
/// read the `setting-absent` branch at any spelling, so `"removed": true` on a
/// no-op was a lie a `--json` consumer would have believed.
#[test]
fn unset_does_not_claim_a_removal_that_did_not_happen() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["--json", "unset", "persistence"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("\"removed\": false") || said.contains("\"removed\":false"),
        "nothing was removed, and the machine-readable answer has to say so: {said}"
    );
}

/// A write a standing layer outranks says so, rather than reporting success.
///
/// Only reachable with a flag now. Without one the rule reaches every repo
/// layer that already holds the key, so an unadorned write cannot be outranked
/// — that is what the rule is for. `--save` walks past it on purpose, because
/// you named the file, and doing so silently was the bug.
#[test]
fn a_write_something_outranks_says_the_value_did_not_change() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    // Unadorned, over a standing local value: reaches it, so nothing is
    // shadowed and nothing is said. This half stops the warning firing on
    // writes that took.
    assert!(sb
        .omh(&["set", "--local", "idle_timeout", "15m"])
        .status
        .success());
    let quiet = sb.omh(&["set", "idle_timeout", "20m"]);
    assert!(
        !String::from_utf8_lossy(&quiet.stderr).contains("outranks"),
        "a write that took must not be reported as shadowed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("20m"),
        "because it reached the layer already holding it: {local}"
    );

    // Named, so the rule steps aside — and says what that cost.
    let out = sb.omh(&["set", "--save", "idle_timeout", "30m"]);
    assert!(out.status.success(), "the write still happens");
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("outranks") && said.contains("20m"),
        "a value that did not change has to say which layer kept it: {said}"
    );
}

/// The advice names the file the value *belongs* in, not the one just written.
///
/// Pointing it at `w.layer` passed the whole suite and produced omh advising
/// that a credential path belongs in the committed file — the exact inversion
/// of the sentence's purpose. The old test could not tell the two apart
/// because it only asked whether `carry_in` appeared somewhere in stderr.
#[test]
fn the_advice_names_the_gitignored_file_not_the_one_written() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["set", "--save", "carry_in", "[\".env\"]"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    let advice = said
        .lines()
        .find(|l| l.contains("belongs in"))
        .unwrap_or_else(|| panic!("no advice line at all: {said}"));
    assert!(
        advice.contains("settings.local.toml"),
        "omh advised the committed file as the home for a credential: {advice}"
    );
    assert!(
        advice.contains("carry_in"),
        "and the advice names the key: {advice}"
    );
    // The generic sentence is a separate line, so `carry_in` appearing once
    // cannot satisfy both claims.
    assert!(
        said.lines().any(|l| l.contains("COMMITTED")),
        "the standing warning survives beside it: {said}"
    );
}

/// A safe key gets the general warning and not the sharp one.
///
/// Without the negative half, folding the key name into the general sentence
/// and deleting the advice block passes.
#[test]
fn a_key_that_carries_no_secret_is_not_singled_out() {
    let sb = sandbox();
    sb.seed_base();

    // A captured login, because `omh set account` now refuses a name none
    // answers to — `account` is still the example here for the reason the
    // key registry gives it: a name, not a credential, and the one
    // value-taking key the docs show being shared on purpose.
    sb.seed_catalogue(&["adapters"]);
    sb.account("claude", "work");
    let out = sb.omh(&["set", "--save", "account", "work"]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(said.contains("COMMITTED"), "got: {said}");
    assert!(
        !said.contains("belongs in"),
        "a name is not a credential, and the sharp sentence means nothing if \
         it fires for both: {said}"
    );
}

/// A write lands in one layer, never several.
///
/// A stray `config::set(…, Layer::Personal)` beside the real write passed
/// everything: every `omh set carry_in` in every repo would also have written
/// `carry_in` into `~/.omh/settings.toml`, making one project's credential
/// paths a default everywhere. The two repo layers were cross-checked; the
/// personal one was in no check at all.
#[test]
fn a_write_reaches_one_layer_and_no_other() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "carry_in", "[\".env\"]"]).status.success());
    let personal = std::fs::read_to_string(sb.home.join(".omh/default.toml")).unwrap_or_default();
    assert!(
        !personal.contains("carry_in"),
        "a repo's credential paths must not become your default everywhere: {personal}"
    );
    assert!(
        !sb.settings().contains("carry_in"),
        "nor reach the committed file: {}",
        sb.settings()
    );
}

/// The two layer flags are opposites, so clap refuses both at once.
///
/// Dropping `conflicts_with` passed everything. `omh set --shared --personal k v`
/// would then write the personal file — `set_layer` tests `personal` first —
/// while warning about a committed file it never touched.
#[test]
fn the_two_layer_flags_cannot_both_be_given() {
    let sb = sandbox();
    sb.seed_base();

    for argv in [
        vec!["set", "--save", "--local", "idle_timeout", "30m"],
        vec!["unset", "--save", "--local", "idle_timeout"],
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` names two files at once and must be refused",
            argv.join(" ")
        );
    }
}

/// The file omh hides a secret in is a file git actually ignores.
///
/// The whole layer rule rests on `settings.local.toml` being ignored, and the
/// ignore line was written by `omh init` alone — so in a repo that was never
/// `omh init`ed, `omh set carry_in` created a credential map that `git add .`
/// would stage. `omh set` asserted the premise and did not establish it.
#[test]
fn the_file_omh_hides_a_secret_in_is_a_file_git_ignores() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    // No `omh init` — the whole point.
    assert!(sb.omh(&["set", "carry_in", "[\".env\"]"]).status.success());

    let checked = std::process::Command::new("git")
        .args(["check-ignore", ".omh/settings.local.toml"])
        .current_dir(&sb.repo)
        .output()
        .expect("git check-ignore runs");
    assert!(
        checked.status.success(),
        "omh routed a credential-bearing key here *because* it is hidden from \
         git, and nothing was hiding it"
    );
}

/// `--dry-run` prints the plan and writes nothing.
#[test]
fn a_dry_run_set_does_not_touch_the_file() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["--dry-run", "set", "carry_in", "[\".env\"]"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("settings.local.toml"),
        "the plan names the file it would write: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !sb.repo.join(".omh/settings.local.toml").exists(),
        "a dry run that writes is not a dry run"
    );
}

/// `omh why <key>` answers for a settings key.
///
/// Three `--help` strings and two documentation pages tell people to ask here.
/// Until now every one of them came back *nothing recorded under that name*,
/// which `why.rs` calls its own failure mode by name.
#[test]
fn why_answers_for_a_settings_key() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["why", "carry_in"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !said.contains("nothing recorded"),
        "the key registry is what the layer rule rests on, and a person has no \
         other way to read it: {said}"
    );
    assert!(
        said.contains("settings.local.toml") && said.contains("gitignored"),
        "it says where omh keeps it, which is the half a settings file cannot \
         show: {said}"
    );
}

/// One command switches a feature, the same one that sets a value.
///
/// `enable`/`disable` were two more verbs for *write a thing to the
/// settings file*, and which of omh's features a project runs with is as much
/// a fact about the project as which runtime it wants. What the two halves do
/// not share is the file format — a feature lives in the `[omh]` table, a
/// setting is a bare key — and that is the whole hazard here.
#[test]
fn a_feature_is_switched_by_the_command_that_sets_a_value() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "codegraph", "off"]).status.success());
    assert!(
        sb.settings().contains("[omh]") && sb.settings().contains("codegraph = false"),
        "a feature belongs in the [omh] table: {}",
        sb.settings()
    );

    let shown = sb.omh(&["info", "--repo"]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("codegraph"),
        "and omh reports it: {}",
        String::from_utf8_lossy(&shown.stdout)
    );

    assert!(sb.omh(&["set", "codegraph", "on"]).status.success());
    assert!(
        sb.settings().contains("codegraph = true"),
        "back on: {}",
        sb.settings()
    );
}

/// A feature is never written as a bare settings key.
///
/// This is the failure the fork exists to prevent, and it is silent: before
/// `omh set` knew about features, `omh set codegraph off` wrote a top-level
/// `codegraph = "off"`, warned that nothing reads it, exited 0 — and the
/// feature stayed on. A settings file that *looks* like it says what you
/// meant, next to a feature that ignored you.
#[test]
fn a_feature_is_not_written_as_a_bare_settings_key() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["set", "codegraph", "off"]);
    assert!(out.status.success());
    let written = sb.settings();
    assert!(
        !written
            .lines()
            .any(|l| l.trim_start().starts_with("codegraph =") && l.contains('"')),
        "a feature written as a string key is a switch that did nothing: {written}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("nothing in omh reads"),
        "and it is not reported as an unknown key: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A feature takes `on` or `off`, and says so when given anything else.
#[test]
fn a_feature_takes_on_or_off_and_names_them() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["set", "codegraph", "true"]);
    assert!(
        !out.status.success(),
        "a feature is not a free-text setting"
    );
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("on") && said.contains("off"),
        "the refusal names the two words that work: {said}"
    );
    assert!(
        sb.settings().is_empty() || !sb.settings().contains("codegraph"),
        "and nothing was written: {}",
        sb.settings()
    );
}

/// A feature takes the layer flags too, because it lives in the same files.
///
/// It used to refuse them, on the reasoning that a feature is a fact about
/// this checkout and no flag should name its file. Half of that is true and
/// the conclusion did not follow: `[omh]` sits in the same two repo files as
/// every setting, so `--local` and `--save` have exactly as much to mean here.
#[test]
fn a_feature_takes_the_layer_flags_like_any_setting() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--local", "codegraph", "off"])
        .status
        .success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("codegraph = false"),
        "--local puts the switch in the gitignored file: {local}"
    );
    assert!(
        !sb.settings().contains("codegraph"),
        "and not also in the committed one: {}",
        sb.settings()
    );

    // An unadorned write then joins it rather than landing underneath.
    assert!(sb.omh(&["set", "codegraph", "on"]).status.success());
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("codegraph = true"),
        "the switch moved where it already was: {local}"
    );
    assert!(
        !sb.settings().contains("codegraph"),
        "still not a second copy: {}",
        sb.settings()
    );
}

/// `omh unset <feature>` drops the switch, letting omh's own default return.
///
/// Without this, `omh unset codegraph` looked in the wrong shape entirely — a
/// feature lives in `[omh]`, `unset` removed bare keys — so it reported
/// `codegraph was not set in the shared layer` while `[omh] codegraph = false`
/// sat in the file, still off. Same silent-wrong as the one `unset` was just
/// fixed for, one table over.
#[test]
fn unset_a_feature_lets_omhs_own_default_return() {
    let sb = sandbox();
    sb.seed_base();

    assert!(sb.omh(&["set", "codegraph", "off"]).status.success());
    assert!(
        sb.settings().contains("codegraph = false"),
        "{}",
        sb.settings()
    );

    let out = sb.omh(&["unset", "codegraph"]);
    assert!(out.status.success());
    assert!(
        !sb.settings().contains("codegraph = false"),
        "the switch is gone: {}",
        sb.settings()
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("was not set"),
        "and it was not reported absent while it was sitting in the file: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A feature that was never switched is reported as never switched.
///
/// Following omh's default is a third state, not a quiet kind of off, and
/// `dropped: []` is what a `--json` consumer needs to tell them apart. Without
/// this, hardcoding the report to claim a removal passes: `omh unset codegraph`
/// on a repo that never touched it says the switch was dropped, which reads as
/// "it was on, now it is not".
#[test]
fn unsetting_a_feature_nobody_switched_says_so() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["--json", "unset", "codegraph"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("\"dropped\": []") || said.contains("\"dropped\":[]"),
        "nothing was dropped, and the machine-readable answer has to say so: {said}"
    );

    let human = sb.omh(&["unset", "codegraph"]);
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("was not switched"),
        "and a person is told the same thing: {}",
        String::from_utf8_lossy(&human.stdout)
    );
}

/// A catalogue entry is still not a feature, through the new spelling too.
#[test]
fn set_tells_an_entry_from_the_feature_that_contains_it() {
    let sb = sandbox();
    sb.seed_base();

    let out = sb.omh(&["set", "graph-rules", "off"]);
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("codegraph"),
        "naming the feature it belongs to is how somebody finds the grouping: {said}"
    );
}

/// `--dry-run` on a feature plans and writes nothing, like its sibling arm.
///
/// It used to be accepted and discarded here: `omh --dry-run set codegraph off`
/// wrote the committed file and printed `wrote →`. The settings arm of the
/// same command honoured it, which is how somebody learns the wrong lesson
/// about this one.
#[test]
fn a_dry_run_on_a_feature_writes_nothing() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let out = sb.omh(&["--dry-run", "set", "codegraph", "off"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("would switch") && !said.contains("wrote →"),
        "a plan does not report a write: {said}"
    );
    assert!(
        !sb.repo.join(".omh/settings.toml").exists(),
        "a dry run that writes is not a dry run"
    );

    // And the same for the removal half.
    assert!(sb.omh(&["set", "codegraph", "off"]).status.success());
    let before = sb.settings();
    let out = sb.omh(&["--dry-run", "unset", "codegraph"]);
    assert!(out.status.success());
    assert_eq!(sb.settings(), before, "the plan changed the file");
}

/// The plan for a removal agrees with the removal.
///
/// The dry run iterated the layers a *write* must reach, which always names
/// the committed file, so it promised to drop a switch from a repo that had
/// never switched anything — and the real run then said the opposite. A
/// preview that contradicts the act it previews is worse than no preview.
#[test]
fn the_plan_for_a_feature_removal_agrees_with_the_removal() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let planned = sb.omh(&["--dry-run", "unset", "codegraph"]);
    assert!(planned.status.success());
    let plan = String::from_utf8_lossy(&planned.stdout).to_string();
    assert!(
        !plan.contains("would drop"),
        "nothing is switched, so there is nothing to drop: {plan}"
    );

    let real = sb.omh(&["unset", "codegraph"]);
    assert!(
        String::from_utf8_lossy(&real.stdout).contains("was not switched"),
        "and the real run says the same: {}",
        String::from_utf8_lossy(&real.stdout)
    );
}

/// A removal that happened is reported as one.
///
/// `dropped` was pinned in exactly one direction — the empty case — so
/// hardcoding it empty passed, and a real removal reported itself as a no-op.
#[test]
fn a_feature_removal_that_happened_says_which_layer() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb.omh(&["set", "codegraph", "off"]).status.success());
    let out = sb.omh(&["--json", "unset", "codegraph"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("\"shared\""),
        "the layer it was dropped from has to be named: {said}"
    );
    assert!(
        !said.contains("\"dropped\": []") && !said.contains("\"dropped\":[]"),
        "a removal that happened is not an empty list: {said}"
    );
}

/// Unsetting one feature leaves another alone.
///
/// Deleting the "was it even there" check passes every test that unsets from a
/// repo with no `[omh]` table at all, because an earlier guard catches those.
/// It takes a table holding a *different* feature to reach the check.
#[test]
fn unsetting_a_feature_leaves_the_others_switched() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb.omh(&["set", "memory", "off"]).status.success());
    let out = sb.omh(&["--json", "unset", "codegraph"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"dropped\": []")
            || String::from_utf8_lossy(&out.stdout).contains("\"dropped\":[]"),
        "codegraph was never switched: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        sb.settings().contains("memory = false"),
        "and memory is untouched: {}",
        sb.settings()
    );
}

/// `omh unset` refuses an entry name, the same as `omh set` does.
///
/// The two arms share a shape and not a line of code, so nothing carried over:
/// collapsing the entry arm on the `unset` side passed the whole suite, and
/// `omh unset graph-rules` reported on a bare key that was never there.
#[test]
fn unset_tells_an_entry_from_the_feature_that_contains_it() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let out = sb.omh(&["unset", "graph-rules"]);
    assert!(!out.status.success(), "an entry is not a feature");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("codegraph"),
        "and the refusal names the feature that contains it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A settings write does not depend on the health of omh's base set.
///
/// `names` loads the manifest to rule out a feature name. Propagating that
/// failure made an ordinary repo-local write fail for a reason with nothing to
/// do with what was typed — in a home where `omh init` had never run, `omh set
/// my_new_key 1` exited 1, and so did `omh unset`, which is the command a
/// person runs to get a secret out of git.
#[test]
fn a_settings_write_survives_a_base_set_it_cannot_read() {
    let sb = sandbox();
    sb.git_init();
    // Deliberately no `seed_base()`.

    let out = sb.omh(&["set", "my_new_key", "1"]);
    assert!(
        out.status.success(),
        "a hand-editable settings file must not be refused because ~/.omh/base \
         is missing: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sb.settings().contains("my_new_key"),
        "and the value is written: {}",
        sb.settings()
    );
    // Said, not swallowed: omh could not check the name against its features.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("base set"),
        "the reduced check is reported: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        sb.omh(&["unset", "my_new_key"]).status.success(),
        "and the removal half too"
    );
}

/// A key held in two files is updated in **both**, not just the first.
///
/// This is rule 1's whole point and nothing was checking the plural: writing
/// only the first layer passed the suite, and `omh set idle_timeout 20m` would
/// have left the committed file on the old value while reporting success —
/// visible to a teammate cloning the repo and to nobody else.
#[test]
fn a_key_held_in_two_files_is_updated_in_both() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["set", "--save", "idle_timeout", "1h"])
        .status
        .success());
    assert!(sb
        .omh(&["set", "--local", "idle_timeout", "15m"])
        .status
        .success());

    assert!(sb.omh(&["set", "idle_timeout", "20m"]).status.success());
    assert!(
        sb.settings().contains("20m") && !sb.settings().contains("1h"),
        "the committed file was left on the old value: {}",
        sb.settings()
    );
    let local = std::fs::read_to_string(sb.repo.join(".omh/settings.local.toml")).unwrap();
    assert!(
        local.contains("20m") && !local.contains("15m"),
        "and the gitignored one too: {local}"
    );
}

/// A feature switched in your template does not switch this repo.
///
/// This is the change, stated where it bites. `[omh] codegraph = false` in
/// `~/.omh/default.toml` used to switch the feature off in every checkout you
/// opened — a repo behaving differently because of a file the repo does not
/// contain and a teammate cloning it does not have. It seeds a *new* repo now
/// and decides nothing after that.
#[test]
fn a_feature_switched_in_the_template_does_not_switch_this_repo() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "[omh]\ncodegraph = false\n",
    )
    .unwrap();

    let out = sb.omh(&["--json", "unset", "codegraph"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"dropped\": []")
            || String::from_utf8_lossy(&out.stdout).contains("\"dropped\":[]"),
        "nothing in this repo switches it: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("still switched"),
        "and nothing outside the repo does either, so there is nothing to \
         report as still deciding: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A removal that fails partway still says what it already did.
///
/// A `?` inside the loop abandoned the layers already dropped **and** the
/// report that would have named them, so a permission error on the second file
/// left the first one silently rewritten — a committed file, which the user
/// then finds as an unexplained diff and has no reason to connect to a command
/// that exited 1 talking about a different path.
#[test]
fn a_removal_that_fails_partway_says_what_it_already_did() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    // Running as root ignores the mode bits, and a test that cannot fail is
    // worse than one that is absent.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    assert!(sb
        .omh(&["set", "--save", "codegraph", "off"])
        .status
        .success());
    assert!(sb
        .omh(&["set", "--local", "codegraph", "off"])
        .status
        .success());

    let local = sb.repo.join(".omh/settings.local.toml");
    let mut mode = std::fs::metadata(&local).unwrap().permissions();
    mode.set_readonly(true);
    std::fs::set_permissions(&local, mode).unwrap();

    let out = sb.omh(&["unset", "codegraph"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !out.status.success(),
        "the failure is still a failure: {said}"
    );
    assert!(
        said.contains("codegraph"),
        "and what it managed to do is reported before it gives up: {said}"
    );
    assert!(
        !sb.settings().contains("codegraph"),
        "the committed file really was rewritten, which is the half a silent \
         failure hides: {}",
        sb.settings()
    );
}

/// `omh settings` shows what you have set, and what omh reads that you have not.
///
/// The registry is what the whole layer rule rests on and a settings file
/// cannot display it — so the command that shows your defaults is where the
/// keys omh reads become discoverable without `omh why` and a guess at a name.
#[test]
fn settings_shows_your_defaults_and_what_omh_reads() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["settings", "set", "idle_timeout", "45m"])
        .status
        .success());

    let out = sb.omh(&["settings"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("idle_timeout") && said.contains("45m"),
        "what you set: {said}"
    );
    // A key omh reads that you have not set is named, with what it is for.
    assert!(
        said.contains("runtime") && said.contains("sandbox"),
        "and the keys you have not set, with what omh reads them for: {said}"
    );
}

/// `omh settings set` writes your defaults, not this checkout's.
///
/// `omh set` became repo-only, so this is the one route left to
/// `~/.omh/settings.toml`. If it wrote a repo file the capability would be
/// gone rather than moved.
#[test]
fn settings_set_writes_your_own_file_and_no_repo_file() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb
        .omh(&["settings", "set", "idle_timeout", "45m"])
        .status
        .success());
    let personal = std::fs::read_to_string(sb.home.join(".omh/default.toml")).unwrap();
    assert!(personal.contains("45m"), "got: {personal}");
    assert!(
        !sb.settings().contains("45m"),
        "your default is not this repo's: {}",
        sb.settings()
    );
    assert!(
        !sb.repo.join(".omh/settings.local.toml").exists(),
        "nor the gitignored one"
    );

    assert!(sb
        .omh(&["settings", "unset", "idle_timeout"])
        .status
        .success());
    let personal = std::fs::read_to_string(sb.home.join(".omh/default.toml")).unwrap();
    assert!(
        !personal.contains("45m"),
        "and it comes back out: {personal}"
    );
}

/// A credential-bearing key in the template draws no committed-file warning.
///
/// `~/.omh/default.toml` is not in a repo at all, so the sentence about git
/// carrying a file has nothing to fire about. What the command *does* say is
/// which kind of file it wrote, which is asserted here rather than left as a
/// claim in the title.
#[test]
fn settings_set_does_not_warn_about_a_file_git_does_not_carry() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let out = sb.omh(&["settings", "set", "carry_in", "[\".env\"]"]);
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("COMMITTED"),
        "your own file is not committed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("seeds new repos"),
        "and it says what the file it wrote is for: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// One of omh's features is not a cross-project default.
///
/// A feature is a fact about a checkout — `[omh]` is read from the personal
/// layer, so writing one there would switch it for every project you open,
/// which is not a decision this command should make easy to reach by accident.
#[test]
fn settings_refuses_a_feature_name() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    // **Both verbs.** The guard was written for four doors and four commands
    // were routed through it; two of those commands were retired, and the
    // test went with them — leaving `unset` alone behind, where deleting the
    // guard call outright passed the whole suite.
    for argv in [
        vec!["settings", "set", "codegraph", "off"],
        vec!["settings", "unset", "codegraph"],
    ] {
        let out = sb.omh(&argv);
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            !out.status.success(),
            "`omh {}`: a feature is not a default",
            argv.join(" ")
        );
        assert!(
            said.contains("omh set codegraph"),
            "and it names the spelling that works: {said}"
        );
    }
    assert!(
        !std::fs::read_to_string(sb.home.join(".omh/default.toml"))
            .unwrap_or_default()
            .contains("codegraph"),
        "and neither verb left the bare key behind"
    );
}

/// `omh set` and `omh settings` are one letter apart and never confused.
///
/// clap can be told to accept unambiguous prefixes. It is not, and this is
/// what keeps it that way: with inference on, `omh setting` would silently
/// become one of these two, and they have opposite scopes.
#[test]
fn set_and_settings_are_never_inferred_from_each_other() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for argv in [
        vec!["setting", "idle_timeout", "45m"],
        vec!["sett"],
        vec!["se", "idle_timeout", "45m"],
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` is not a command and must not be guessed at",
            argv.join(" ")
        );
    }

    // And both real spellings work, so the refusal above is about inference
    // rather than about either command being broken.
    assert!(sb.omh(&["settings"]).status.success());
    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());
}

/// `omh init` seeds a new repo from your template, and says what it took.
///
/// This is the one moment `~/.omh/default.toml` has any effect. A seed nobody
/// is told about is indistinguishable from a default — and the repo now
/// *carries* the values rather than inheriting them, which is a fact about a
/// committed file and belongs in the report.
///
/// Ignored because `init` builds an image, so this half runs only where a
/// container runtime does. The seeding itself is a pure function and is tested
/// without one by `seed_settings_takes_what_omh_reads_and_refuses_a_token` —
/// this covers the wiring, that covers the rule.
#[test]
#[ignore]
fn init_seeds_the_repo_from_your_template_and_says_so() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"45m\"\nrubbish = \"ignored\"\n\n[use]\nskills = [\"tdd\"]\n",
    )
    .unwrap();

    let out = sb.omh(&["init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("seeded"),
        "the seed is reported: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let seeded = sb.settings();
    assert!(seeded.contains("45m"), "the key travelled: {seeded}");
    assert!(
        seeded.contains("[use]") && seeded.contains("tdd"),
        "and the selection: {seeded}"
    );
    assert!(
        !seeded.contains("rubbish"),
        "a key omh reads nothing from is not propagated into every repo you \
         ever start: {seeded}"
    );
}

/// `init` reports what it did **here**, and not what the machine has.
///
/// Nothing asserted init's printed output at all — every other init test reads
/// files on disk — so the report was unguarded in both directions: a row could
/// be dropped or invented and the suite would not notice. The rewrite that
/// dropped the inventory is exactly the change that needed one.
///
/// The three claims, in the order they broke:
///
/// - **No inventory.** `harnesses 3 (…)` and `editors 4 (…)` opened the report
///   and were true before the command ran. `omh info` answers that.
/// - **The catalogue selection**, which is the thing `init` actually decided
///   about this repo and which nothing said.
/// - **Three next lines**, not one. `omh new` starts a session and
///   `omh s resume` rejoins it, and a reader shown only the first starts a
///   second session instead — which is the mistake splitting the two verbs
///   apart was meant to make unreachable.
///
/// Ignored because `init` builds an image.
#[test]
#[ignore]
fn init_reports_what_it_did_here_and_not_what_the_machine_has() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md", "skills/refactor/SKILL.md"]);

    let out = sb.omh(&["init"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for gone in ["harnesses", "editors"] {
        assert!(
            !said.contains(gone),
            "`{gone}` is a fact about the machine, unchanged by this command \
             — `omh info` is where it lives now: {said}"
        );
    }
    // Separately, and for a different reason: `not yet done: recall` sat four
    // lines under the base set's own `memory` row, which is the MCP server
    // that answers `recall`. A new user's first screen contradicted itself.
    // The constant and the guard reading it went with the line; this is what
    // is left of the claim, and it is the claim rather than the spelling that
    // mattered.
    assert!(
        !said.contains("not yet done"),
        "no line calls undone what the rows above just installed: {said}"
    );
    // The row, not two substrings that could sit anywhere. The gutter is
    // padded to the widest label in the table, so its width is data and
    // cannot be spelled here — the line is found by its label instead.
    let skills = said
        .lines()
        .find(|l| l.trim_start().starts_with("skills"))
        .unwrap_or_else(|| panic!("no skills row: {said}"));
    assert!(
        skills.contains("2 selected"),
        "the count belongs to the capability beside it: {skills:?}"
    );
    for line in ["omh new claude", "omh s resume", "omh s attach"] {
        assert!(
            said.contains(line),
            "`{line}` is one of the three, and a reader shown only the first \
             starts a second session rather than rejoining: {said}"
        );
    }

    // The same answer, and the same shape, as the command whose whole job it
    // is. Two derivations of one fact are two facts.
    let repo = String::from_utf8_lossy(&sb.omh(&["info", "--repo"]).stdout).to_string();
    assert!(
        repo.contains("review-diff") && repo.contains("refactor"),
        "init wrote the selection `omh info --repo` reads: {repo}"
    );

    let json: serde_json::Value = serde_json::from_slice(&sb.omh(&["--json", "init"]).stdout)
        .expect("--json is machine-readable");
    assert!(json["adapters"].is_null(), "the inventory left: {json:#}");
    let skills = json["using"]
        .as_array()
        .unwrap_or_else(|| panic!("no using in {json:#}"))
        .iter()
        .find(|u| u["capability"] == "skills")
        .unwrap_or_else(|| panic!("no skills row in {json:#}"));
    assert_eq!(skills["selected"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["next"][0]["run"], "omh new claude", "got {json:#}");
}

/// A probe that could not run is not a sandbox that has everything.
///
/// The deepest of the gates, and the one that reaches this report looking most
/// like success. `measure` reports a failed probe and swallows it — right for a
/// launch, where an unmeasured program suppresses nothing and no hook is
/// dropped on a guess — so the facts stay as they were, `render::held_back`
/// derives an empty list from them, and `init` printed a clean bill of health
/// issued by a doctor who was out. `--json` carried no trace of it at all: the
/// warning goes to stderr, and a machine consumer read `held_back: []`.
///
/// The healthy run and the broken one were byte-identical.
///
/// Ignored because `init` runs a container.
#[test]
#[ignore]
fn a_probe_that_could_not_run_is_not_a_clean_bill_of_health() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    // **No stack**, deliberately. With one there are provisioning conditions
    // to ask about, the shim answers none of them, and `fired_from` stops the
    // run at the shallower gate — which reports correctly and never reaches
    // the one this test is about. Nothing to provision is what gets `init` all
    // the way down to the ask.
    sb.fake_docker();
    // Only the `--pull=never` run, which is the facts probe. The predicate run
    // carries no such flag and is unaffected.
    std::fs::write(sb.bin.join("docker-probe-refuses"), "x").unwrap();

    let out = sb.omh(&["init"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "a diagnostic that could not run does not fail the setup: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("not measured"),
        "the sandbox was never asked, and an empty list says the opposite: {said}"
    );

    let json: serde_json::Value = serde_json::from_slice(&sb.omh(&["--json", "init"]).stdout)
        .expect("--json is machine-readable");
    assert!(
        !json["hooks_unchecked"].is_null(),
        "the warning is on stderr, so this is all a script has: {json:#}"
    );
}

/// When the hooks were not measured, the report names the gate that stopped it.
///
/// An empty held-back list meant both *nothing was held back* and *nothing was
/// asked*, and only one of those is good news. The field that tells them apart
/// carried a single string set where it was declared — so a repo with a
/// harness, an image, and a probe that came back short was told
/// `not measured — no harness`, two rows under the line naming its harness.
///
/// A field that exists to end a misleading silence must not replace it with a
/// misleading sentence, so the reason is written by whichever gate stopped it
/// and this asserts the *content*, not the presence.
///
/// Ignored because `init` runs a container.
#[test]
#[ignore]
fn a_hook_measurement_that_did_not_happen_says_which_gate_stopped_it() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    // A stack, so there are conditions to ask about — with none, "asked
    // nothing" is the honest answer and this row correctly never appears.
    std::fs::write(
        sb.repo.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // Answers `inspect` but returns nothing from `run`, which is a probe that
    // came back short rather than one that failed — the arm that reported the
    // wrong reason, because it is the one that looks like an ordinary run.
    sb.fake_docker();

    let out = sb.omh(&["init"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("not measured"),
        "an unasked question is not an answer of `none`: {said}"
    );
    assert!(
        !said.contains("not measured — no harness"),
        "this repo has a harness, and the row above says so: {said}"
    );
    assert!(
        said.contains("conditions") || said.contains("could not be asked"),
        "the reason names the gate that actually stopped it: {said}"
    );

    let json: serde_json::Value = serde_json::from_slice(&sb.omh(&["--json", "init"]).stdout)
        .expect("--json is machine-readable");
    assert!(
        !json["hooks_unchecked"].is_null(),
        "and a script reads the same fact: {json:#}"
    );
}

/// The template decides nothing in a repo that already exists.
///
/// The whole argument for the rename. A repo's behaviour is explained by files
/// inside the repo — which is what a teammate cloning it can see, and what
/// `omh info --repo` can account for without pointing at a file they do not have.
#[test]
fn the_template_decides_nothing_in_a_repo_that_already_exists() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"45m\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(sb.repo.join(".omh/settings.toml"), "carry_in = []\n").unwrap();

    let out = sb.omh(&["info", "--repo"]);
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("45m"),
        "a value from outside the repo reached this repo's resolution: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The file 0.7.0 renamed is reported, not silently ignored.
///
/// Somebody upgrades, their defaults stop applying, and nothing anywhere says
/// why — that is the failure this project keeps writing down, and a rename is
/// exactly when it happens.
#[test]
fn the_renamed_template_is_reported_rather_than_dropped() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/settings.toml"),
        "idle_timeout = \"9h\"\n",
    )
    .unwrap();

    let out = sb.omh(&["settings"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("settings.toml") && said.contains("default.toml"),
        "the old name and the new one, so the fix is obvious: {said}"
    );
}

/// A server's environment is never seeded into a committed file.
///
/// `[mcp.<name>.env]` can hold a token and the file `init` writes is committed.
/// It already has a home — on the server in `~/.omh/mcp.json` — so this is
/// refused rather than dropped: silently skipping it would leave somebody
/// believing a token is in force.
#[test]
fn a_server_environment_is_refused_in_the_template() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "[mcp.linear.env]\nTOKEN = \"secret\"\n",
    )
    .unwrap();

    let out = sb.omh(&["init"]);
    assert!(
        !out.status.success(),
        "a token must not reach a committed file"
    );
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("omh settings mcp add"),
        "and it names where a server's environment belongs: {said}"
    );
    assert!(
        !sb.settings().contains("secret"),
        "nothing was written: {}",
        sb.settings()
    );
}

/// Nothing in the template reaches a repo's resolution — not just the keys.
///
/// The bare-key half goes through `config::policy`; `[omh]` and `[use]` go
/// through `settings::resolve`, which is a different function reading a
/// different list. Making that one read the template again passed every test
/// this PR had: the feature switch and the selection would both have come back
/// as a layer while the keys stayed out, which is the same defect wearing the
/// half of the change that did land.
#[test]
fn no_part_of_the_template_resolves_in_this_repo() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"45m\"\n\n[omh]\ncodegraph = false\n\n\
         [use]\nskills = [\"from-template\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(sb.repo.join(".omh/settings.toml"), "carry_in = []\n").unwrap();

    let out = sb.omh(&["info", "--repo"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !said.contains("45m"),
        "a bare key from the template resolved here: {said}"
    );
    // The feature's row, whatever the column widths are. Asserting the padding
    // pinned the table renderer rather than the behaviour.
    let row = said
        .lines()
        .find(|l| l.trim_start().starts_with("codegraph"))
        .unwrap_or_else(|| panic!("no codegraph row: {said}"));
    assert!(
        row.contains(" on") && !row.contains("off"),
        "an `[omh]` switch from the template reached this repo: {row}"
    );
    assert!(
        !said.contains("from-template"),
        "a `[use]` list from the template reached this repo: {said}"
    );
}

/// `init` reports a seed only when it created the file.
///
/// `write_if_absent` never revisits, so re-running `init` in a configured repo
/// changes no settings — and claiming a seed there describes an effect the
/// template did not have, about a file the reader would then go looking in.
///
/// Ignored for the same reason as its sibling: `init` builds an image. The
/// answer it reports off is pinned without a runtime by
/// `write_if_absent_reports_only_a_write_it_made`.
#[test]
#[ignore]
fn init_claims_no_seed_over_a_settings_file_that_was_already_there() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"45m\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(sb.repo.join(".omh/settings.toml"), "carry_in = []\n").unwrap();

    let out = sb.omh(&["init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("seeded"),
        "nothing was seeded, and saying so would send somebody looking for a \
         value that is not there: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !sb.settings().contains("45m"),
        "and the existing file is untouched: {}",
        sb.settings()
    );
}

/// The rename is said by every command, not by the one that names it.
///
/// It lived in `omh settings`, which reaches only people who already know a
/// command was added — while launch, `repo`, `doctor`, `why` and `init` all
/// stayed silent, and `init` is the one moment the template is supposed to
/// matter. Somebody upgrades, their defaults stop applying everywhere at once,
/// and nothing says why.
#[test]
fn the_rename_is_reported_by_commands_other_than_settings() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/settings.toml"),
        "idle_timeout = \"9h\"\n",
    )
    .unwrap();

    for argv in [vec!["info"], vec!["settings"]] {
        let out = sb.omh(&argv);
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            said.contains("default.toml") && said.contains("settings.toml"),
            "`omh {}` said nothing about the file that stopped being read: {said}",
            argv.join(" ")
        );
    }
}

/// A template shape omh cannot turn into a settings file is refused, not written.
///
/// `seed_settings` assembled TOML by string concatenation, which is correct
/// only while every value is a plain value and every key is bare — and a
/// template is hand-edited. `[carry_in]` as a table emitted `carry_in =x = 1`;
/// `init` wrote that, then failed parsing it, and `write_if_absent` never
/// revisits — so re-running `init` could not repair it and the repo stayed
/// broken until somebody found a file nothing had mentioned.
#[test]
fn a_template_omh_cannot_seed_is_refused_before_anything_is_written() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();

    for (template, expected) in [
        ("[carry_in]\nx = 1\n", "carry_in"),
        ("[omh]\nnosuchthing = false\n", "nosuchthing"),
        // The specific message, not just the name — the generic unknown-table
        // refusal would also mention `provision`, so asserting the name alone
        // let the reasoned refusal be deleted with the suite green.
        ("[provision]\n\"go/toolchain\" = true\n", "machine"),
        ("[mcp.linear.env]\nTOKEN = \"secret\"\n", "mcp"),
        ("[typo_table]\nx = 1\n", "typo_table"),
    ] {
        std::fs::write(sb.home.join(".omh/default.toml"), template).unwrap();
        let out = sb.omh(&["init"]);
        assert!(
            !out.status.success(),
            "`{template}` was accepted, and a repo seeded from it"
        );
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            said.contains(expected) && said.contains("default.toml"),
            "the refusal names the offending part and the template it is in — \
             not the repo file, which the user never wrote: {said}"
        );
        assert!(
            !sb.repo.join(".omh/settings.toml").exists(),
            "`{template}` left a settings.toml behind; `write_if_absent` never \
             revisits, so that repo cannot be repaired by re-running init"
        );
    }
}

/// A repo nobody has configured reports nothing configured.
///
/// `init` wrote `carry_in = []` into every new `settings.toml`, so the first
/// thing `omh info --repo` said about a fresh repo was
///
///     settings
///       carry_in  []  ← shared
///
/// which reads as a decision somebody made. Nobody made it. The file already
/// carries the commented example that teaches the key — `# carry_in =
/// [".env.local", "certs/"]` — and a commented line is the honest way to show
/// a setting you are not setting.
///
/// Ignored because `init` builds an image.
#[test]
#[ignore]
fn a_repo_nobody_has_configured_reports_nothing_configured() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    assert!(sb.omh(&["init"]).status.success());

    let said = String::from_utf8_lossy(&sb.omh(&["info", "--repo"]).stdout).to_string();
    let settings = said
        .split("omh's features")
        .next()
        .unwrap_or_else(|| panic!("no settings section: {said}"));
    assert!(
        !settings.contains("carry_in"),
        "nobody set this — the template teaches the key with a commented \
         example, which is what a setting you are not setting looks like: {said}"
    );

    // The teaching line survives, because dropping the value must not drop the
    // explanation with it.
    let file = sb.settings();
    assert!(
        file.contains("# carry_in"),
        "the commented example is how somebody learns the key exists: {file}"
    );
}

/// The COMMITTED warning fires where a secret could land, not on every write.
///
/// `Why` exists to keep this sentence rare, and its doc says why: the warning
/// *"was rare and deliberate while a flag was the only way to reach one and is
/// nearly every write now that the committed file is the default. A sentence
/// that fires on almost every invocation is one people stop reading, and this
/// codebase has already watched that happen once, to this same sentence."*
///
/// `set <key> <value>` got that narrowing. `set <feature> on|off` did not — and
/// a feature switch is **always** a committed write, so the warning fired every
/// single time, on the most ordinary action the command has. That is the
/// crying-wolf the narrowing was built to stop, one arm over.
///
/// The line still has to appear where it matters, so both directions are here.
#[test]
fn the_committed_warning_is_kept_for_what_it_is_for() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let switch = sb.omh(&["set", "codegraph", "off"]);
    assert!(switch.status.success());
    assert!(
        !String::from_utf8_lossy(&switch.stderr).contains("COMMITTED"),
        "switching a feature off for this repo is what the committed file is \
         for: {}",
        String::from_utf8_lossy(&switch.stderr)
    );

    // A key nothing classified, sent to the committed file because that is what
    // omh does with one it has never heard of. Still worth saying out loud.
    let unknown = sb.omh(&["set", "somekey", "somevalue"]);
    assert!(unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("COMMITTED"),
        "a key omh cannot classify reaching a file git carries is the case this \
         sentence exists for: {}",
        String::from_utf8_lossy(&unknown.stderr)
    );
}

/// The account is the setting, and there is no second way to say it.
///
/// `-a` was a global that overrode the setting for one invocation and recorded
/// nothing. So a session started with it could not be resumed without
/// repeating it — and forgetting meant the account mount no longer matched the
/// container's stamp, which either blocked the resume or brought the container
/// back as a different account. It looked like *run this session as work* and
/// was *run this launch as work, then give the session an identity it cannot
/// remember*.
///
/// One account per repo is the shape that was actually wanted, and
/// `omh set account` already expressed it: `resolve_for_launch` is
/// `explicit.or(configured)`, so the setting was the fallback the whole time.
/// Removing the flag leaves the fallback as the answer.
#[test]
fn the_account_is_the_setting_and_there_is_no_second_way_to_say_it() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.seed_catalogue(&["adapters"]);
    sb.account("claude", "work");

    for argv in [
        vec!["-a", "work", "new", "claude"],
        vec!["new", "claude", "-a", "work"],
        vec!["s01", "resume", "-a", "work"],
        vec!["doctor", "-a", "work"],
    ] {
        let out = sb.omh(&argv);
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            !out.status.success(),
            "`omh {}` names an account a second way",
            argv.join(" ")
        );
        assert!(
            said.contains("unexpected argument") || said.contains("--account"),
            "and clap refuses it as an argument that is not there: {said}"
        );
    }
}

/// `omh set account` refuses a name no login answers to.
///
/// The account is one thing with one spelling now: `omh auth <harness> -n work`
/// creates it, `omh set account work` selects it, and every command that
/// launches or probes reads the setting. The global `-a` that used to override
/// it per invocation is gone — it recorded nothing, so a session started with
/// it could not be resumed without repeating it, and forgetting meant the
/// stamp either blocked the resume or brought the container back as a
/// different account.
///
/// Which makes this check the whole safety net: a typo in the setting is now
/// the only way to point a launch at credentials that are not there, and it
/// would otherwise surface as a failed login inside a sandbox.
///
/// Accounts are stored per harness — `~/.omh/creds/<harness>/<account>` — so
/// there are three answers, not two, and the middle one is the common case.
#[test]
fn setting_an_account_refuses_a_name_no_login_answers_to() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    // Accounts are discovered per harness, so there has to be a harness.
    sb.seed_catalogue(&["adapters"]);

    // Nothing captured at all: the answer is how to capture one.
    let none = sb.omh(&["set", "account", "work"]);
    let said = String::from_utf8_lossy(&none.stderr).to_string();
    assert!(!none.status.success(), "no login answers to `work`: {said}");
    assert!(
        said.contains("omh auth"),
        "and the answer is the command that creates one: {said}"
    );

    sb.account("claude", "work");

    // Captured for a harness: accepted, and it says which — `work` is right
    // until the day you run `omh new opencode`, and that is the moment to have
    // been told.
    let ok = sb.omh(&["set", "account", "work"]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(
        String::from_utf8_lossy(&ok.stderr).contains("claude"),
        "the harness it is captured for is worth saying: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // A typo, with something to compare against.
    let typo = sb.omh(&["set", "account", "wrok"]);
    let said = String::from_utf8_lossy(&typo.stderr).to_string();
    assert!(!typo.status.success(), "`wrok` is a typo: {said}");
    assert!(
        said.contains("work"),
        "and the refusal names what does exist: {said}"
    );
}

/// A session scopes a session verb, and nothing else.
///
/// `memory remember` was the one exception, and it did not earn it: the id
/// reached exactly one line, and it was not a scope —
///
///     input.source = match session {
///         Some(id) => format!("session {id}, cli"),
///         None => "cli".into(),
///     };
///
/// Provenance, in a text field. Nothing is filed, scoped or retrieved by
/// session; the store is repo-wide, which is what the design says it is. So the
/// exception bought a global flag for a string that `--source` already writes,
/// and left `consumes_session` claiming a relationship the store does not have.
///
/// The in-sandbox path is unaffected — `memory serve` carries its own
/// `--session`, which is how an agent's notes get attribution.
#[test]
fn a_session_scopes_a_session_verb_and_nothing_else() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    // Complete, deliberately. An incomplete line fails *both* readings, and
    // the arbitration then prefers the sessions error — so a short fixture
    // would have tested clap's message rather than this rule.
    let out = sb.omh(&[
        "s01",
        "memory",
        "remember",
        "--expected",
        "a",
        "--observed",
        "b",
        "--evidence",
        "c",
        "--answers",
        "d",
    ]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !out.status.success(),
        "the store is repo-wide, so a session id here scopes nothing: {said}"
    );
    assert!(
        said.contains("does not act on one"),
        "and it is refused where it was named, like every other command that \
         does not read one: {said}"
    );

    // The provenance it used to supply, said outright.
    let wrote = sb.omh(&[
        "memory",
        "remember",
        "--expected",
        "a",
        "--observed",
        "b",
        "--evidence",
        "c",
        "--answers",
        "d",
        "--source",
        "session s01, cli",
    ]);
    assert!(
        wrote.status.success(),
        "{}",
        String::from_utf8_lossy(&wrote.stderr)
    );
}

/// A name nothing answers to is refused, whichever command you typed.
///
/// `omh settings mcp rm nope` exited **0** and said *"nope is not in your
/// catalogue"* — a typo reported as success, which is the one thing every
/// sibling already refuses to do. `use`, `unuse`, `memory rm` and
/// `memory promote` all exit 1 on the same mistake, and one of them carries a
/// comment saying why: a name this repo never used is a typo, and reporting
/// success for it is how the typo survives.
///
/// A script cannot tell a removal from a misspelling when the exit code is the
/// same, and a person reads a green exit as *done*.
#[test]
fn a_name_nothing_answers_to_is_refused_whichever_command_you_typed() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.catalogue(&["skills/real/SKILL.md"]);

    for argv in [
        vec!["settings", "mcp", "rm", "nope"],
        vec!["unuse", "skills", "nope"],
        vec!["use", "skills", "nope"],
        vec!["memory", "rm", "nope"],
        vec!["memory", "promote", "nope"],
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` reported a typo as success",
            argv.join(" ")
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("nope"),
            "and names the word it could not find: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A name this repo cannot use is not a name your catalogue lacks.
///
/// `omh use hooks go-test` in a rust repo said *"your catalogue has no hooks
/// called `go-test`"* — while `omh info` listed all six, `go-test` among them.
/// The check reads `catalogue_names`, which filters hooks down to the
/// ecosystems this repo actually is, and then worded the filtering as absence.
///
/// Two costs, and the second is worse than the wrong sentence: it went on to
/// offer `omh settings edit hooks go-test`, which creates a *second*
/// `go-test.json` beside the one already there.
///
/// The invariant is that the two states are told apart. A catalogue that
/// genuinely lacks the name is the case where creating it is the answer; a
/// catalogue that has it is never that case.
#[test]
fn a_name_this_repo_cannot_use_is_not_a_name_the_catalogue_lacks() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    // The hook *files*, not just the manifest — `go-test` has to really be in
    // the catalogue for the claim that it is not to be false.
    sb.seed_catalogue(&["hooks", "stacks"]);
    // Rust, so omh's go and python hooks are in the catalogue and inapplicable.
    std::fs::write(
        sb.repo.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let missing = sb.omh(&["use", "hooks", "nosuchhook"]);
    let missing_said = String::from_utf8_lossy(&missing.stderr).to_string();
    assert!(!missing.status.success());
    assert!(
        missing_said.contains("no hooks called"),
        "a name nothing answers to is absent, and creating it is the answer: {missing_said}"
    );

    let present = sb.omh(&["use", "hooks", "go-test"]);
    let present_said = String::from_utf8_lossy(&present.stderr).to_string();
    assert!(
        !present.status.success(),
        "a hook for an ecosystem this repo is not is still refused"
    );
    assert!(
        !present_said.contains("no hooks called"),
        "your catalogue has `go-test` — `omh info` lists it: {present_said}"
    );
    assert!(
        !present_said.contains("settings edit"),
        "and offering to create it would write a second copy beside the one \
         that is already there: {present_said}"
    );
}

/// The way out that `omh why` prints is a way out.
///
/// Every base-set entry carries a `remove` field, and `omh why` prints it
/// verbatim as the answer to "how do I get rid of this". Nothing checked that
/// running it got rid of anything: the existing guard asks only whether the
/// line *parses*, so six entries shipped naming a command that removes a third
/// of what they claim — the MCP server, leaving the feature `on` with nothing
/// behind it and its hooks still selected.
///
/// The invariant, not the wording: **after running what the field says, the
/// feature is no longer on in this repo.** A rewrite of the sentence keeps
/// passing; a command that does not do the job does not.
///
/// Runs everywhere — none of these commands needs a container.
#[test]
fn the_way_out_the_base_set_prints_is_a_way_out() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("base/2026.08.toml"))
            .unwrap();

    // `(feature, remove)` per entry, read positionally: the fields are written
    // in a fixed order in the file and both are required, so a pair that fails
    // to form is a manifest this test should not be silent about.
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut feature: Option<String> = None;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("feature = ") {
            feature = Some(rest.trim_matches('"').to_string());
        }
        if let Some(rest) = line.strip_prefix("remove  = ") {
            let said = rest.trim_matches('"');
            entries.push((
                feature
                    .clone()
                    .expect("`feature` precedes `remove` in an entry"),
                said.to_string(),
            ));
        }
    }
    assert!(
        entries.len() > 8,
        "the scan read {} entries, fewer than the base set has",
        entries.len()
    );

    let mut wrong = Vec::new();
    for (feature, said) in &entries {
        // **The field opens with the command**, then an em dash and what it
        // does or does not take with it. Anchored at the start rather than
        // found anywhere, because these fields are prose and their prose names
        // other commands: one `git-notice` entry mentions `omh sNN log --turns`
        // to say it stops working, and a scan that searched the sentence
        // dutifully ran it.
        //
        // A field with no command is a failure, not a skip. That is the shape
        // three `git-notice` entries hid behind, telling the reader to
        // hand-edit `[omh]` in a TOML file long after `omh set <feature> off`
        // existed — an answer you cannot run, printed under the heading
        // `remove`.
        let Some(rest) = said.strip_prefix("omh ") else {
            wrong.push(format!(
                "`{feature}`'s way out does not open with a command to type: {said}"
            ));
            continue;
        };
        let command = rest.split('—').next().unwrap_or(rest).trim();

        let sb = sandbox();
        sb.git_init();
        sb.seed_base();
        let out = sb.omh(&command.split_whitespace().collect::<Vec<_>>());
        if !out.status.success() {
            wrong.push(format!(
                "`omh {command}` does not run: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            continue;
        }
        let after = sb.omh(&["info", "--repo"]);
        assert!(after.status.success());
        let said_after = String::from_utf8_lossy(&after.stdout).to_string();
        let still_on = said_after
            .lines()
            .any(|l| l.trim_start().starts_with(feature.as_str()) && l.contains("on"));
        if still_on {
            wrong.push(format!(
                "`omh {command}` leaves `{feature}` on — printed as the way out of it"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the base set prints {} way{} out that does not work:\n  {}",
        wrong.len(),
        if wrong.len() == 1 { "" } else { "s" },
        wrong.join("\n  ")
    );
}

/// `omh settings` works where the file it edits lives — outside a repo.
///
/// Its own docs open with "**You**, before a repo exists". It refused with a
/// message about worktree branches, which have nothing to do with writing a
/// default in your home directory.
#[test]
fn settings_works_outside_a_repository() {
    let sb = sandbox();
    sb.seed_base();
    let outside = sb.home.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();

    let out = sb.omh_in(&outside, &["settings", "set", "idle_timeout", "45m"]);
    assert!(
        out.status.success(),
        "the file this command edits is not in a repo: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let template = std::fs::read_to_string(sb.home.join(".omh/default.toml")).unwrap();
    assert!(template.contains("45m"), "got: {template}");

    // Every verb, not just the one that was checked. `set` and the bare form
    // were reached through the `Paths::anywhere` the arm resolves; `edit` and
    // `mcp` were handed the bare `cwd` and called `Paths::discover` on it
    // themselves, so they went on refusing while the test above stayed green.
    for argv in [
        vec!["settings"],
        vec!["settings", "edit"],
        vec!["settings", "mcp", "ls"],
    ] {
        let out = sb.omh_in(&outside, &argv);
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "`omh {}` reads or writes ~/.omh and needs no worktree: {said}",
            argv.join(" ")
        );
        assert!(
            !said.contains("git repository"),
            "`omh {}` answered about worktree branches: {said}",
            argv.join(" ")
        );
    }
}

/// A name from your catalogue is refused by **both** doors, and each names the
/// capability it actually found the name in.
///
/// The refusal checked `Skills` alone, so a rule or a hook of the same name
/// got a bare key in the committed file and exit 0 — the quiet answer the
/// refusal exists to replace. `omh info --repo` prints hook names next to
/// `not selected`, which is exactly where somebody reads one and types it.
///
/// And `omh settings set` let all of them through on the reading that its
/// guard was about features, writing into the file new repos are seeded from.
#[test]
fn a_catalogue_entry_of_any_capability_is_not_a_setting() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&[
        "skills/myskill/SKILL.md",
        "rules/myrule.md",
        "commands/mycommand.md",
        "subagents/mysubagent.md",
        "hooks/myhook.json",
    ]);

    for (name, capability) in [
        ("myskill", "skills"),
        ("myrule", "rules"),
        ("mycommand", "commands"),
        ("mysubagent", "subagents"),
        ("myhook", "hooks"),
    ] {
        for argv in [vec!["set", name, "on"], vec!["settings", "set", name, "on"]] {
            let out = sb.omh(&argv);
            let said = String::from_utf8_lossy(&out.stderr).to_string();
            assert!(
                !out.status.success(),
                "`omh {}` wrote a catalogue entry as a bare key: {said}",
                argv.join(" ")
            );
            assert!(
                said.contains(&format!("omh use {capability} {name}")),
                "the refusal spells the capability it found `{name}` in, or it                  is advice that does not work: {said}"
            );
        }
    }
    assert!(
        !sb.settings().contains("myrule"),
        "the committed file took a key nothing reads: {}",
        sb.settings()
    );
    assert!(
        !std::fs::read_to_string(sb.home.join(".omh/default.toml"))
            .unwrap_or_default()
            .contains("myrule"),
        "the file every new repo is seeded from took one"
    );
}

/// `omh settings` shows the tables it seeds as seeded, not as read by nothing.
///
/// `config::values` dropped tables, so `[use]` and `[omh]` were invisible —
/// and once they were visible they landed under *read by nothing*, which is
/// exactly backwards: they are the two things the template propagates.
#[test]
fn settings_shows_the_tables_it_seeds_as_seeded() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "[omh]\ncodegraph = false\n\n[use]\nskills = [\"tdd\"]\n",
    )
    .unwrap();

    let out = sb.omh(&["settings"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    let seeded = said
        .split("also seeded into a new repo")
        .nth(1)
        .unwrap_or_else(|| panic!("no seeded section: {said}"));
    assert!(
        seeded.contains("[omh]") && seeded.contains("[use]"),
        "the tables the template propagates are named as such: {said}"
    );
    assert!(
        !said.contains("read by nothing"),
        "and not reported as read by nothing, which is the opposite: {said}"
    );
}

/// The rename advice never destroys the file omh actually reads.
///
/// `mv old new` was printed unconditionally. With both files present that
/// overwrites a populated `default.toml` — omh printing the command that loses
/// somebody's configuration, which is worse than omh losing it, because they
/// typed it themselves and had no reason to doubt the tool.
#[test]
fn the_rename_advice_does_not_overwrite_a_template_that_exists() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/settings.toml"),
        "idle_timeout = \"9h\"\n",
    )
    .unwrap();

    // Only the old file: `mv` is safe and is what to say.
    let said = String::from_utf8_lossy(&sb.omh(&["info", "--repo"]).stderr).to_string();
    assert!(
        said.contains("mv "),
        "with nothing to overwrite, say mv: {said}"
    );
    let mv = said.lines().find(|l| l.contains("mv ")).unwrap();
    let (from, to) = mv.split_once("mv ").unwrap().1.split_once(' ').unwrap();
    assert!(
        from.ends_with("settings.toml") && to.ends_with("default.toml"),
        "the operands are the retired name then the live one, in that order: {mv}"
    );

    // Both files: `mv` would destroy the live one, so it must not be advised.
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"30m\"\n",
    )
    .unwrap();
    let said = String::from_utf8_lossy(&sb.omh(&["info", "--repo"]).stderr).to_string();
    assert!(
        !said.contains("mv "),
        "advising mv here overwrites the template omh reads: {said}"
    );
    assert!(
        said.contains("default.toml") && said.contains("settings.toml"),
        "and it still says which is which: {said}"
    );
}

/// Nothing is said when there is nothing to say.
///
/// The notice runs on every command now, so a condition that always holds
/// would put it in front of every user on every run, naming a file they do not
/// have.
#[test]
fn the_rename_is_silent_when_the_old_file_is_gone() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let out = sb.omh(&["info", "--repo"]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    // The only assertion here is a negative on stderr, and a command that
    // refused writes a *different* sentence to the same stream — so without
    // this the test passes hardest when the line does not run at all.
    assert!(out.status.success(), "the command must run: {said}");
    assert!(
        !said.contains("not read any more"),
        "no retired file exists, so there is nothing to report: {said}"
    );
}

/// `omh settings set` does not claim a repo value outranks your template.
///
/// "Outranks" is a claim about resolution and the template is not in it. Saying
/// a repo value beat your default is the exact confusion this release removed,
/// printed by the command that owns the file.
#[test]
fn setting_a_default_is_not_reported_as_losing_to_a_repo() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb.omh(&["set", "idle_timeout", "5m"]).status.success());
    let out = sb.omh(&["settings", "set", "idle_timeout", "45m"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !said.contains("outranks"),
        "the template does not lose a contest it is not in: {said}"
    );
}

/// A write says which of the three kinds of file it landed in.
///
/// `tracked()` has three arms and only one was pinned. Reverting the template
/// to "gitignored" — the answer the boolean gave — or to "committed" both
/// passed, and the second would tell somebody a file git does not carry is
/// carried by git.
#[test]
fn a_write_names_which_kind_of_file_it_landed_in() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for (argv, expected) in [
        (vec!["set", "--save", "idle_timeout", "30m"], "(committed)"),
        (
            vec!["set", "--local", "idle_timeout", "15m"],
            "(gitignored)",
        ),
        (
            vec!["settings", "set", "idle_timeout", "45m"],
            "(seeds new repos)",
        ),
    ] {
        let out = sb.omh(&argv);
        assert!(out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(expected),
            "`omh {}` must say {expected}: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// `omh settings` lists a key omh reads nothing from, and only such keys.
///
/// The *read by nothing* section is described as the point of the report —
/// "a typo looks exactly like a setting that took" — and nothing pinned it.
/// Inverting the filter, so it listed the keys omh *does* read and hid the
/// typos, passed.
#[test]
fn settings_names_a_key_read_by_nothing_and_no_other() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.home.join(".omh")).unwrap();
    std::fs::write(
        sb.home.join(".omh/default.toml"),
        "idle_timeout = \"45m\"\ncarry_ins = \"typo\"\n",
    )
    .unwrap();

    let said = String::from_utf8_lossy(&sb.omh(&["settings"]).stdout).to_string();
    let unread = said
        .split("set here, and read by nothing")
        .nth(1)
        .unwrap_or_else(|| panic!("no unread section: {said}"));
    assert!(unread.contains("carry_ins"), "the typo is named: {said}");
    assert!(
        !unread.contains("idle_timeout"),
        "and a key omh does read is not listed as unread: {said}"
    );
}

/// A feature switch omh cannot read is refused, not skipped.
///
/// `read_provision` refuses a non-boolean by name under a comment reading
/// *"two readers of one table must not disagree about strictness"*.
/// `read_table` was the third reader and it skipped one — so `omh unset`
/// reported the feature unswitched here while a layer it had not reached still
/// held `codegraph = "false"`, and the next command to read settings failed to
/// parse the file. Success, then a parse error from somewhere else.
#[test]
fn a_feature_switch_omh_cannot_read_is_named_not_skipped() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    std::fs::create_dir_all(sb.repo.join(".omh")).unwrap();
    std::fs::write(
        sb.repo.join(".omh/settings.toml"),
        "carry_in = []\n\n[omh]\ncodegraph = false\n",
    )
    .unwrap();
    // A string where a switch belongs, in the layer `--save` will not touch.
    std::fs::write(
        sb.repo.join(".omh/settings.local.toml"),
        "[omh]\ncodegraph = \"false\"\n",
    )
    .unwrap();

    let out = sb.omh(&["unset", "--save", "codegraph"]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        said.contains("codegraph") && said.contains("true or false"),
        "the entry omh cannot read has to be named where it is found, not \
         passed over: {said}"
    );
    assert!(
        said.contains("settings.local.toml"),
        "and the file it is in, since that is what has to be edited: {said}"
    );
}

/// `omh s01 attach zed` opens **that** session, in **that** editor.
///
/// The four tests this replaces asserted only that stderr lacked
/// *unrecognized subcommand*. All four produced the identical message — `no
/// adapters installed`, the first statement `attach` makes — so they proved
/// clap had routed the line somewhere and nothing else. Three mutations passed
/// them all: the arm doing nothing, the arm calling `graph`, and the arm
/// dropping `cli.session` so `omh s01 attach` opens the newest session
/// instead.
///
/// The premise that excused them was wrong. The success path needs no
/// container runtime — `fake_docker` and a seeded catalogue are enough, and
/// `tests/cli.rs` already records learning that once, one verb over, in
/// `a_launch_records_the_harness_it_started_and_a_dry_run_does_not`.
#[test]
fn attach_opens_the_session_and_the_editor_it_was_given() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.fake_docker();
    // Two, so "the one named" and "the newest" are different answers.
    sb.session("s01");
    sb.session("s02");

    let opened = sb.fake_editor("zed");

    let out = sb.omh(&["s01", "attach", "zed"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("s01"),
        "the prefix names which session to open, and s02 is the newer: {said}"
    );
    assert!(
        !said.contains("s02"),
        "and it is not the one the prefix did not name: {said}"
    );
    assert!(
        said.contains("zed"),
        "the editor named is the editor opened: {said}"
    );
    // What the editor was actually handed, not what the report claims. The
    // report is built from the same `alias` either way, so it agrees with
    // itself whether or not the launch ever happened.
    let launched = std::fs::read_to_string(&opened).unwrap_or_default();
    assert!(
        launched.contains("s01") && !launched.contains("s02"),
        "the editor opened the session the prefix named: {launched:?}"
    );
}

/// `omh s42 attach` refuses a session that does not exist, like every sibling.
///
/// It invented one. `Start::Named` returns the id unchecked and `ensure`
/// creates the worktree, so a typo built an empty session off the base branch
/// and opened your editor on it, exit 0, saying `session s42 is up` — the same
/// sentence a real rejoin prints. The comment directly above claimed the
/// opposite: *attaching an editor to a session that does not exist yet is not
/// a thing anyone asks for*.
///
/// Every other verb under `omh s` goes through `existing_session`, which
/// refuses by name. This one joined that group in 0.7.0 and did not join its
/// discipline.
#[test]
fn attach_refuses_a_session_that_does_not_exist() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.fake_docker();
    sb.session("s01");

    let out = sb.omh(&["s42", "attach", "zed"]);
    assert!(
        !out.status.success(),
        "a session that does not exist is a typo, not a request to make one: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("s42"),
        "and the refusal names it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !sb.home.join(".omh/worktrees/repo/s42").exists(),
        "nothing was built for it"
    );
}

/// With no sessions at all, attach says so rather than inventing one.
#[test]
fn attach_with_no_sessions_says_so() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.fake_docker();

    let out = sb.omh(&["s", "attach"]);
    assert!(
        !out.status.success(),
        "there is nothing to attach to: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The editor stays optional: with none given, omh prints every recipe.
#[test]
fn attach_without_an_editor_prints_the_recipes() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.seed_catalogue(&["adapters", "base", "stacks", "editors"]);
    sb.fake_docker();
    sb.session("s01");

    let out = sb.omh(&["s01", "attach"]);
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("code") && said.contains("zed"),
        "one row per editor omh knows: {said}"
    );
}

/// Every retired spelling in the table is refused, and names a line that parses.
///
/// The table replaced two hand-rolled tombstone variants. This is what a table
/// buys that variants could not: one walk asserting the property of all of
/// them, including the shapes a variant missed — the retired alias, arguments
/// after it, and `--session` on the old spelling.
#[test]
fn every_retired_spelling_is_refused_and_names_a_replacement() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for argv in [
        vec!["attach"],                            // types the retired verb on purpose
        vec!["attach", "zed"],                     // types the retired verb on purpose
        vec!["attach", "zed", "nvim"],             // types the retired verb on purpose
        vec!["a"],                                 // types the retired verb on purpose
        vec!["a", "zed"],                          // types the retired verb on purpose
        vec!["-s", "s01", "attach"],               // types the retired verb on purpose
        vec!["--session", "s01", "attach", "zed"], // types the retired verb on purpose
        vec!["s", "ls"],                           // types the retired verb on purpose
        vec!["sessions", "ls"],                    // types the retired verb on purpose
        vec!["s01", "ls"],                         // types the retired verb on purpose
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` names a retired spelling and must be refused",
            argv.join(" ")
        );
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            said.contains("omh s attach") || said.contains("omh s "),
            "`omh {}` must name the spelling that replaced it, not clap's \
             complaint: {said}",
            argv.join(" ")
        );
    }

    // And the table does not overreach: a spelling retired *under* `sessions`
    // must not answer for the top-level word, which became `omh info` in a
    // rename that deliberately kept no sentence at all.
    let out = sb.omh(&["ls"]); // types the retired verb on purpose
    assert!(!out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("is the listing"),
        "the sessions replacement is not the answer for a top-level word"
    );
}

/// The retired command is gone; everything under it answers to `omh settings`,
/// and each verb is asked for an **effect** rather than an exit code.
///
/// `set` and `unset` were already there. `edit` and `mcp` move rather than
/// retire: opening your settings in `$EDITOR` and curating your MCP servers
/// are both things you do to `~/.omh`, which is what `omh settings` now means.
///
/// Exit 0 was the whole of this test once, and it proved only that clap had
/// routed the line somewhere: replacing the entire `mcp` arm with `Ok(())`
/// left the suite green while the compiler reported `mcp_add`, `mcp_remove`,
/// `mcp_import` and `report::Servers` all dead.
#[test]
fn what_config_held_answers_to_settings() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let added = sb.omh(&["settings", "mcp", "add", "srv1", "echo"]);
    assert!(
        added.status.success(),
        "adding a server: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    let listed = sb.omh(&["settings", "mcp", "ls"]);
    assert!(listed.status.success());
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("srv1"),
        "what `mcp add` wrote is what `mcp ls` reads: {}",
        String::from_utf8_lossy(&listed.stdout)
    );
    let removed = sb.omh(&["settings", "mcp", "rm", "srv1"]);
    assert!(removed.status.success());
    assert!(
        !String::from_utf8_lossy(&sb.omh(&["settings", "mcp", "ls"]).stdout).contains("srv1"),
        "and `rm` is read by the same listing"
    );

    for argv in [
        vec!["settings", "set", "idle_timeout", "45m"],
        vec!["settings", "unset", "idle_timeout"],
    ] {
        let out = sb.omh(&argv);
        assert!(
            out.status.success(),
            "`omh {}` must work: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // **Which file**, not merely that something was opened. The layer is a
    // constant at the call site, so `Personal` → `Shared` there changes what
    // `$EDITOR` opens and nothing else — and nothing noticed.
    assert!(sb.omh(&["settings", "edit"]).status.success());
    let opened = sb.opened_in_the_editor();
    assert!(
        opened.contains("default.toml"),
        "`omh settings edit` opens your defaults, not a repo file: {opened:?}"
    );
}

/// The retired spelling says what replaced it, for every verb it held.
#[test]
fn the_retired_config_spelling_names_what_replaced_it() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for argv in [
        vec!["config"],                               // types the retired verb on purpose
        vec!["config", "set", "idle_timeout", "45m"], // types the retired verb on purpose
        vec!["config", "unset", "idle_timeout"],      // types the retired verb on purpose
        vec!["config", "edit"],                       // types the retired verb on purpose
        vec!["config", "mcp", "ls"],                  // types the retired verb on purpose
        vec!["c"],                                    // types the retired verb on purpose
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` names a retired spelling",
            argv.join(" ")
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("omh settings"),
            "and the refusal teaches the one that works: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// `--layer` goes with the command that needed it.
///
/// It existed because two commands wanted opposite write defaults. One rule
/// across four commands replaced that, and the command that carried the flag
/// went with it — so this is clap's refusal, not a deprecation notice, and
/// the docs that promised one more release have been corrected.
#[test]
fn the_layer_flag_is_gone() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    let out = sb.omh(&["settings", "set", "--layer", "shared", "idle_timeout", "1h"]);
    assert!(!out.status.success(), "--layer is not a flag any more");
}

/// `omh info --repo` answers what this checkout resolved.
///
/// `repo` was deleted, and this report is the only place that says which
/// file decided a value, which of omh's features are on here, and which
/// catalogue entries this project takes. `omh info` already means *what you
/// have*; `--repo` narrows that from the machine to the checkout.
#[test]
fn info_repo_reports_what_this_checkout_resolved() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());
    assert!(sb.omh(&["set", "codegraph", "off"]).status.success());

    let out = sb.omh(&["info", "--repo"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("30m") && said.contains("shared"),
        "a value and the file that decided it — the whole point of the \
         report: {said}"
    );
    assert!(
        said.contains("codegraph"),
        "and which of omh's features are on here: {said}"
    );
}

/// The two scopes stay apart: the machine, and this checkout.
#[test]
fn info_without_repo_is_still_about_the_machine() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    assert!(sb.omh(&["set", "idle_timeout", "30m"]).status.success());

    let said = String::from_utf8_lossy(&sb.omh(&["info"]).stdout).to_string();
    assert!(
        said.contains("harnesses"),
        "the host inventory is what bare `omh info` is: {said}"
    );
    assert!(
        !said.contains("30m"),
        "and a repo's resolved settings are the other question: {said}"
    );
}

/// The catalogue listing that `omh info` inherited is asserted here, or the
/// relocation can be undone without anything going red.
///
/// It was the half of bare `config` that was **moved** rather than
/// deleted, and moved capabilities are the ones that go missing quietly:
/// replacing the whole block with `Vec::new()` left the suite green. The JSON
/// carries it too, because that is the shape a script reads.
#[test]
fn info_lists_the_catalogue_it_inherited_from_the_deleted_command() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();
    sb.catalogue(&["skills/myskill/SKILL.md", "rules/myrule.md"]);

    let said = String::from_utf8_lossy(&sb.omh(&["info"]).stdout).to_string();
    for want in ["your catalogue", "myskill", "myrule"] {
        assert!(
            said.contains(want),
            "bare `config` listed the catalogue and `omh info` is where it \
             went — no `{want}`: {said}"
        );
    }

    let out = sb.omh(&["info", "--json"]);
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json is machine-readable");
    let catalogue = json["catalogue"]
        .as_array()
        .unwrap_or_else(|| panic!("no catalogue in {json:#}"));
    let skills = catalogue
        .iter()
        .find(|c| c["capability"] == "skills")
        .unwrap_or_else(|| panic!("no skills row in {json:#}"));
    assert_eq!(skills["count"], 1, "got {json:#}");
    assert_eq!(skills["entries"][0], "myskill", "got {json:#}");
}

/// A retired spelling is answered for where the verb goes, and nowhere else.
///
/// `retired()` runs after clap has refused, for *any* reason — so a table
/// entry that matched a word anywhere in the line answered a migration
/// question nobody asked. `config` and `repo` are ordinary names for a
/// settings key, a skill, or an MCP server, and every line below is a plain
/// typo whose honest answer clap already had. Getting *"`config` is gone — it
/// is `omh settings` now"* from `omh settings mcp add config` is a confident
/// wrong answer in place of a correct one.
#[test]
fn a_retired_spelling_somewhere_other_than_the_verb_gets_claps_own_refusal() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for (argv, expected) in [
        (vec!["settings", "set", "config"], "<VALUE>"),
        (vec!["set", "repo"], "<VALUE>"),
        (vec!["settings", "mcp", "add", "config"], "<COMMAND>"),
        // `c` and `a` are retired *verbs*, and both are one letter — the
        // shape most likely to turn up as an argument to something else.
        (vec!["s", "c"], "unrecognized subcommand"),
        (vec!["doctor", "c"], "unexpected argument"),
        (vec!["s", "attach", "--nope"], "unexpected argument"),
    ] {
        let out = sb.omh(&argv);
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            !out.status.success(),
            "`omh {}` is not a line omh accepts",
            argv.join(" ")
        );
        assert!(
            said.contains(expected),
            "`omh {}` deserves clap's own `{expected}`, not a retirement \
             notice: {said}",
            argv.join(" ")
        );
        assert!(
            !said.contains("is gone"),
            "`omh {}` was answered as a migration: {said}",
            argv.join(" ")
        );
    }
}

/// The retired spelling says what replaced it.
#[test]
fn the_retired_repo_spelling_names_what_replaced_it() {
    let sb = sandbox();
    sb.git_init();
    sb.seed_base();

    for argv in [
        vec!["repo"],                               // types the retired verb on purpose
        vec!["repo", "set", "idle_timeout", "30m"], // types the retired verb on purpose
        vec!["repo", "enable", "codegraph"],        // types the retired verb on purpose
    ] {
        let out = sb.omh(&argv);
        assert!(
            !out.status.success(),
            "`omh {}` names a retired spelling",
            argv.join(" ")
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("omh info --repo"),
            "and the refusal names the report that replaced it: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A name is checked where it is minted, so `edit` cannot be talked into
/// joining a path to the catalogue directory.
#[test]
fn edit_refuses_a_name_that_climbs_out_of_the_catalogue() {
    let sb = sandbox();
    sb.seed_base();
    let out = sb.omh(&["settings", "edit", "skills", "../../../.ssh/id_rsa"]);
    assert!(!out.status.success(), "traversal must not reach $EDITOR");
    assert!(String::from_utf8_lossy(&out.stderr).contains("never a path"));
}

/// `omh info --repo` is where the reporting this design keeps promising
/// surfaces: with a curated list the useful question stops being "what is this
/// set to" and becomes "why is this skill not here".
#[test]
fn info_repo_reports_what_is_used_what_is_not_and_what_decided_it() {
    let sb = sandbox();
    sb.seed_base();
    sb.catalogue(&["skills/review-diff/SKILL.md", "skills/refactor/SKILL.md"]);
    sb.omh(&["use", "skills", "review-diff"]);
    sb.omh(&["unuse", "skills", "refactor"]);
    sb.omh(&["set", "codegraph", "off"]);
    sb.omh(&["set", "carry_in", "[\".env\"]"]);

    let out = sb.omh(&["info", "--repo"]);
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
/// Before P4 a write to `.omh/settings.toml` was rare — `omh settings set` and
/// nothing else. Now `omh use`, `omh unuse` and `omh set` all touch it,
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

/// `[omh]` layers the same way, so `omh set` has the same hole.
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

    assert!(sb.omh(&["set", "codegraph", "on"]).status.success());
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

/// `omh s01` is one row of the dashboard, not a refusal and not a menu.
///
/// The prefix means *scope this to s01* for every other verb, so the no-verb
/// case is that same rule reaching the last place it had not. It used to be a
/// clap error, because `omh s` required a subcommand and `omh s01 ls` — the
/// only spelling that could have meant this — was refused outright.
///
/// A verb list would have been the alternative and is the wrong answer: the
/// user named a session, and replying with a menu throws that away.
#[test]
fn a_session_named_on_its_own_is_that_session_alone() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");

    let focused = sb.omh(&["s01"]);
    let printed = String::from_utf8_lossy(&focused.stdout);
    assert!(
        focused.status.success(),
        "a session named on its own is a question, not an error: {}",
        String::from_utf8_lossy(&focused.stderr)
    );
    assert!(printed.contains("s01"), "it is about s01: {printed}");
    assert!(
        !printed.contains("s02"),
        "and only about s01 — a focus that quietly widens is the thing the \
         selector exists to remove: {printed}"
    );
    // The invariant is not *no other id ever appears*: an unreadable session
    // is named in a focused view on purpose, because it is why the overlap
    // answer may be short a line. What must not appear is another session's
    // row, and that is what this fixture — two readable sessions — pins.

    // …and the unfocused command still answers about all of them.
    let all = String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();
    assert!(
        all.contains("s01") && all.contains("s02"),
        "`omh s` is still every session: {all}"
    );

    // A collision between two *other* sessions does not follow the focus in.
    // The one involving s01 does — that half is asserted where the overlap
    // section is tested, since it needs the fixture that produces one.
    let three = sb.session("s03");
    let four = sb.session("s04");
    for worktree in [&three, &four] {
        std::fs::write(worktree.join("elsewhere.rs"), "fn elsewhere() {}\n").unwrap();
    }
    let focused = String::from_utf8_lossy(&sb.omh(&["s01"]).stdout).to_string();
    assert!(
        !focused.contains("elsewhere.rs"),
        "a collision between two other sessions is not s01's business: {focused}"
    );

    // An id nothing created fails the way it fails for every other verb,
    // rather than listing nothing and looking like an answer.
    let missing = sb.omh(&["s99"]);
    let err = String::from_utf8_lossy(&missing.stderr).to_string();
    assert!(!missing.status.success(), "an unknown session is an error");
    // Checking only the exit code would accept a clap error, a panic, or a
    // failure from any layer at all — the comment in `sessions_ls` claims it
    // fails *the way every other verb fails*, so the message is the claim.
    assert!(
        err.contains("s99") && err.contains("omh s"),
        "the refusal names the id and where the real ones are listed: {err}"
    );
}

/// The focused listing's `--json` is one session, and says which.
///
/// `--json` is the scripting contract and returns before the asides, so the
/// document is the whole of what a script gets. The human filter and the JSON
/// filter share one binding today, which means a focus-drop mutation dies on
/// the human assertion — but that shared binding is an implementation detail,
/// and the contract should be pinned where it is read.
#[test]
fn the_focused_listing_is_one_session_in_the_document_too() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");

    let out = sb.omh(&["s01", "--json"]);
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`omh s01 --json` is a document");
    let sessions = doc["sessions"].as_array().expect("sessions is an array");
    assert_eq!(sessions.len(), 1, "one session was asked for: {doc}");
    assert_eq!(sessions[0]["id"], "s01", "and it is the one named: {doc}");
}

/// A wide `down` with nobody to ask stops nothing.
///
/// `omh s down` with no session named stops *every* sandbox. That is what it
/// is for, and it is also the one omh command whose blast radius grows with
/// how much work you have in flight — four sessions of an afternoon's agent
/// output, killed by a command one word shorter than the one you meant.
///
/// So it asks. And because it asks, it has to handle being unable to: in a
/// script, in CI, or behind a pipe, there is nobody to answer. `ask`'s own
/// rule is that silence declines and a closed pipe stops — the safe answer
/// has to be the one somebody gives when they are not there.
///
/// `--all` is the way to mean it without being asked. Without that, a `down`
/// in CI would either hang or stop everything, and both are worse than
/// refusing.
#[test]
fn a_wide_down_with_nobody_to_ask_stops_nothing() {
    let sb = sandbox();
    let log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");
    // What omh's probe reads: the shim's `ps` prints this file.
    std::fs::write(sb.bin.join("containers"), "omh-repo-s01\nomh-repo-s02\n").unwrap();

    // stdin is closed — `Command::output` nulls it — so this is the CI case.
    let out = sb.omh(&["s", "down"]);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !out.status.success(),
        "a question nobody answered is not a yes: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let calls = sb.docker_calls(&log);
    assert!(
        !calls.iter().any(|c| c.contains("rm")),
        "and nothing was stopped: {calls:?}"
    );
    assert!(
        said.contains("--all"),
        "it names the way to mean it: {said}"
    );

    // Naming one is unchanged — this refuses a *wide* down, not `down`.
    let one = sb.omh(&["s01", "down"]);
    assert!(
        one.status.success(),
        "a named session still stops: {}",
        String::from_utf8_lossy(&one.stderr)
    );
    assert!(
        sb.docker_calls(&log).iter().any(|c| c.contains("rm")),
        "s01 really went down"
    );

    // …and so is saying you meant all of them.
    let all = sb.omh(&["s", "down", "--all"]);
    assert!(
        all.status.success(),
        "`--all` is the answer to the question: {}",
        String::from_utf8_lossy(&all.stderr)
    );
}

/// A value the key cannot take is written, and said so.
///
/// `persistence` accepts `dtach` or `none`; anything else parses at *launch*,
/// which is minutes later and in a different command. Written either way, for
/// the same reason an unknown key is — a settings file is hand-editable and a
/// value a newer omh will accept must not be refused by this one — but the
/// typo is named where it was typed.
#[test]
fn a_value_the_key_cannot_take_is_written_and_named() {
    let sb = sandbox();

    let wrong = sb.omh(&["set", "persistence", "tmux"]);
    let said = String::from_utf8_lossy(&wrong.stderr).to_string();
    assert!(wrong.status.success(), "still written: {said}");
    assert!(
        said.contains("tmux") && said.contains("dtach"),
        "the value it cannot take, and the ones it can: {said}"
    );

    // One it can take says nothing — a warning on every write is no warning.
    let right = sb.omh(&["set", "persistence", "dtach"]);
    let quiet = String::from_utf8_lossy(&right.stderr).to_string();
    assert!(
        !quiet.contains("dtach, none"),
        "a value that fits is not lectured about: {quiet}"
    );
}

/// Committing a key that carries a secret says which key, not just which layer.
///
/// The warning has read *"the shared layer is COMMITTED — never put a secret
/// here"* since the layers existed. True, and it fires identically for
/// `account`, which is a name, and for `carry_in`, which is the one documented
/// route to a credential. A warning that cannot tell those apart is one people
/// learn to scroll past.
///
/// Now that omh can classify a key, the one write that reaches git can say
/// what is actually at stake.
#[test]
fn committing_a_key_that_carries_a_secret_names_the_key() {
    let sb = sandbox();

    let risky = sb.omh(&["set", "--save", "carry_in", "[\".env\"]"]);
    let said = String::from_utf8_lossy(&risky.stderr).to_string();
    assert!(risky.status.success(), "still written: {said}");
    assert!(
        said.contains("COMMITTED"),
        "the standing warning survives: {said}"
    );
    assert!(
        said.contains("carry_in"),
        "and it names the key that makes this the dangerous one: {said}"
    );

    // A key that carries no secret keeps the general warning and gains nothing
    // — otherwise the sharper sentence means nothing.
    // A captured login, because `omh set account` now refuses a name none
    // answers to — `account` is still the example here for the reason the
    // key registry gives it: a name, not a credential, and the one
    // value-taking key the docs show being shared on purpose.
    sb.seed_catalogue(&["adapters"]);
    sb.account("claude", "work");
    let safe = sb.omh(&["set", "--save", "account", "work"]);
    let mild = String::from_utf8_lossy(&safe.stderr).to_string();
    assert!(
        mild.contains("COMMITTED") && !mild.contains("`account` "),
        "a name is not singled out the way a secret path is: {mild}"
    );
}

/// A key omh does not read is written, and said so.
///
/// `set` accepts any key at all — it has to, since a settings file is
/// hand-editable and omh must not refuse a key a newer version will read. But
/// accepting it silently means `carry_ins` and `idle_timout` land in the file,
/// are never read by anything, and look exactly like a setting that took.
///
/// So it is written and named: the value is not lost, and the typo is not
/// discovered by wondering why nothing changed.
#[test]
fn a_setting_omh_does_not_read_is_written_and_named() {
    let sb = sandbox();

    let typo = sb.omh(&["set", "carry_ins", "[\".env\"]"]);
    let said = String::from_utf8_lossy(&typo.stderr).to_string();
    assert!(
        typo.status.success(),
        "an unknown key is still written: {said}"
    );
    assert!(
        said.contains("carry_ins"),
        "and omh says which key it knows nothing about: {said}"
    );

    // A key omh does read says nothing extra — the warning has to mean
    // something, and one that fires for every write means nothing.
    let known = sb.omh(&["set", "carry_in", "[\".env\"]"]);
    let quiet = String::from_utf8_lossy(&known.stderr).to_string();
    assert!(
        !quiet.contains("nothing reads"),
        "a key omh reads is not reported as unknown: {quiet}"
    );
}

/// A session named first is never silently dropped.
///
/// The selector's promise is that a leading `sNN` scopes what follows. For a
/// word `sessions` has no verb for, `session_prefix` falls back to the line as
/// written — and because `Cmd::Run` swallows any word, that fallback always
/// parses. So `omh s01 why codegraph` becomes `omh why codegraph` and the
/// `s01` reaches a handler that never reads it.
///
/// Nothing about that is visible: exit 0, a real answer, and the scope gone.
/// It is the same defect `omh s01 ls` had in #67, and the same one the tombstone
/// there fixed one spelling at a time. This closes the class instead: a command
/// that does not consume the session refuses a line that names one.
///
/// `doctor` is refused here too. It *should* scope — that is the `--session`
/// half of the doctor rework — but scoping it needs the session's sandbox, and
/// a refusal is the honest placeholder until that lands. Refusing and reading
/// are both correct; silently discarding is not.
#[test]
fn a_session_named_first_is_never_silently_dropped() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "editors", "stacks"]);
    sb.session("s01");

    // Both of these answer *successfully* today, which is what makes them the
    // right red: a command that failed for some other reason would not tell us
    // whether the scope was honoured or discarded.
    for line in [
        vec!["s01", "why", "codegraph"],
        vec!["--session", "s01", "info"],
        // A fresh session's id is generated, so naming one is a contradiction
        // rather than a scope to honour. This is also what lets `omh new`
        // hand `run` a bare `None`: if the refusal above ever stopped
        // covering it, that `None` would start a *different* session than the
        // one named, in silence.
        vec!["s01", "new", "claude"],
    ] {
        let out = sb.omh(&line);
        let printed = String::from_utf8_lossy(&out.stdout).to_string();
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            !out.status.success(),
            "`omh {}` answered as though the s01 were not there: {printed}",
            line.join(" ")
        );
        assert!(
            err.contains("s01"),
            "and the refusal names the scope it could not honour: {err}"
        );
    }

    // The same word without a session still works — this refuses a *pairing*,
    // not a command.
    let plain = sb.omh(&["why", "codegraph"]);
    assert!(
        plain.status.success(),
        "`omh why` is unaffected: {}",
        String::from_utf8_lossy(&plain.stderr)
    );
}

/// `omh doctor` honours the account you name, or says why it cannot.
///
/// `doctor` is the only evidence this repo has that an adapter works — AGENTS.md
/// says so under *honesty about coverage* — and credentials are the half no
/// in-process test can reach. So a `doctor` that skips the credential checks is
/// the one failure that leaves nothing behind it.
///
/// It skipped them for anyone with more than one account. `doctor_cmd` passed a
/// hardcoded `None` where `omh new` passes the user's `-a`, and then wrapped the
/// resolver in `.unwrap_or(None)` — turning a function whose own doc comment
/// reads *"Ambiguity is an error, never a guess"* into a guess of *no account*.
/// Two accounts captured, and `omh -a work doctor` printed *"no account, so
/// credentials go unchecked"*, byte-identical to naming none.
///
/// The remedy the resolver prints — *pick one with `-a <name>`* — named the very
/// flag `doctor` was discarding, and was swallowed before it could print.
#[test]
fn doctor_checks_the_credentials_of_the_account_it_was_given() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.git_init();
    sb.seed_catalogue(&["adapters", "base", "editors", "stacks"]);
    sb.account("claude", "personal");
    sb.account("claude", "work");

    // Progress goes to stderr and the answer to stdout, so both are read: the
    // claim is about which account omh chose, not about where it said so.
    // Neither run reaches a verdict — the probe needs a container the sandbox
    // does not provide — and neither needs to. What is being asked is what omh
    // decided *before* it got there.
    let both = |o: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };

    // Ambiguous and unnamed: refused, naming both, rather than proceeding as
    // though the user had no credentials at all.
    let said = both(&sb.omh(&["doctor", "--harness", "claude"]));
    assert!(
        !said.contains("no account"),
        "two accounts is not no account: {said}"
    );
    assert!(
        said.contains("work") && said.contains("personal"),
        "the refusal names the accounts it could not choose between: {said}"
    );

    // Chosen: the account the setting names is the account checked. It was a
    // flag — `-a work` — and the flag is gone: the account is one thing with
    // one spelling, and `doctor` reads it like every command that launches.
    assert!(sb.omh(&["set", "account", "work"]).status.success());
    let said = both(&sb.omh(&["doctor", "--harness", "claude"]));
    assert!(
        !said.contains("no account"),
        "the setting is not discarded on the way to doctor: {said}"
    );
    assert!(
        said.contains("as work"),
        "and the account chosen is the one checked: {said}"
    );
}

/// A named value reaches the thing it names.
///
/// The parser guard beside these proves the *grammar* changed — that `--name`
/// and `--harness` parse and the bare words are refused. It cannot prove the
/// value arrives anywhere, and six mutations that drop it on the way to the
/// handler passed the whole suite: `auth` capturing into `default` whatever you
/// asked for, `doctor` verifying the harness you did not name, and the account
/// validator deleted outright.
///
/// These two ask the binary instead. Neither needs a container: both refusals
/// happen before anything is provisioned, which is what makes them cheap enough
/// to be worth having.
#[test]
fn a_named_value_is_the_value_the_command_uses() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_adapters();

    // An account name is a single path component, because credentials mount
    // **writable** — `../../..` resolves to `~` and hands the agent the real
    // credential store. `validate_name` is well covered as a function; what was
    // not covered is that anything still calls it.
    let traversal = sb.omh(&["auth", "claude", "--name", "../../.."]);
    let err = String::from_utf8_lossy(&traversal.stderr).to_string();
    assert!(
        !traversal.status.success(),
        "an account name that escapes its directory is refused: {}",
        String::from_utf8_lossy(&traversal.stdout)
    );
    assert!(
        err.contains("single name") || err.contains("not a path"),
        "and refused as a path, not as something else that happened to fail: {err}"
    );

    // The harness named is the harness checked. Dropping the value here makes
    // `doctor` verify whichever harness the host prefers and report it green —
    // and `doctor` is the only evidence in this repo that an adapter works.
    let unknown = sb.omh(&["doctor", "--harness", "definitely-not-a-harness"]);
    let err = String::from_utf8_lossy(&unknown.stderr).to_string();
    assert!(
        !unknown.status.success(),
        "a harness omh does not have is refused rather than swapped for one it \
         does: {}",
        String::from_utf8_lossy(&unknown.stdout)
    );
    assert!(
        err.contains("definitely-not-a-harness"),
        "and the refusal names what was asked for: {err}"
    );
}

/// The inventory answers to `info`, and `ls` is gone from the top level too.
///
/// `ls` was the wide listing's verb and `omh s` took over listing sessions in
/// 2026.08, which left the old spelling meaning *everything except the thing
/// `ls` most suggests*. `info` says what it is: what you have here, not what
/// is running.
///
/// Red before the rename, in both directions: `omh info` was not a command,
/// and the retired spelling still answered. Both halves matter — a rename that
/// only adds the new spelling leaves two ways to say one thing, which is the
/// shape this repo keeps paying for in lines that go on naming a verb nobody
/// can type.
#[test]
fn the_inventory_answers_to_info_and_not_to_the_verb_it_replaced() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.seed_adapters();
    sb.session("s01");

    let named = sb.omh(&["info"]);
    let out = String::from_utf8_lossy(&named.stdout).to_string();
    assert!(
        named.status.success(),
        "`omh info` is the inventory: {out}{}",
        String::from_utf8_lossy(&named.stderr)
    );
    // Contents, not headings. `harnesses:` and `sessions:` are printed
    // unconditionally, empty list and all, so asserting on them passes over a
    // `fn info` whose harness collection has been replaced by `Vec::new()` —
    // measured, and it survived the whole suite. What each section *found* is
    // the only part that can go wrong quietly.
    //
    // `editors` is not asserted: that section is omitted entirely when the
    // catalogue is empty, which this sandbox's is.
    assert!(
        out.contains("claude"),
        "the harness this sandbox has is listed: {out}"
    );
    assert!(
        out.contains("s01"),
        "and so is the session that exists: {out}"
    );

    // The old spelling is not a second door. Asserting only that it *fails*
    // is not enough and was measured not to be: a hidden variant that printed
    // the whole inventory and then exited non-zero passed that check, which is
    // exactly the "two ways to say one thing" this test is named against. So
    // the claim is that it answered with nothing, and that the refusal came
    // from clap rather than from omh — there is no tombstone here, by explicit
    // decision, since a bare word can no longer be swallowed by a launch.
    let old = sb.omh(&["ls"]); // types the retired verb on purpose
    let said = String::from_utf8_lossy(&old.stdout).to_string();
    let why = String::from_utf8_lossy(&old.stderr).to_string();
    assert!(
        !old.status.success(),
        "the retired spelling is not a command"
    );
    assert!(
        said.is_empty(),
        "and it answers with nothing at all: {said}"
    );
    assert!(
        why.contains("unrecognized subcommand"),
        "refused by the parser, not by a tombstone omh maintains: {why}"
    );
}

/// A verb that was retired is refused by name, and never becomes another
/// command.
///
/// The `ls` verb was the documented spelling until 2026.08, so it is in muscle
/// memory and in scripts. Retiring it left two ways to get this wrong, and
/// only one of them is harmless.
///
/// Typing it bare is: clap rejects an unknown subcommand. `omh s01 ls` was
/// not. With no `ls` under `sessions` the sessions reading failed to parse,
/// the as-written reading was a live top-level `ls`, and `session_prefix`'s
/// fallback handed that reading the launch — to a command that never read
/// `cli.session`, so the scope was dropped in silence and every session was
/// listed, which is verbatim the harm the refusal removed in #67 existed to
/// prevent: *"it would list every session and look like it had listed one."*
///
/// Past tense throughout, and deliberately. `consumes_session` refuses a
/// prefix nothing consumes, and the top-level verb has since been renamed, so
/// the as-written reading no longer parses either. The tombstone survives for
/// what it *says* — clap names no replacement, and this does.
#[test]
fn the_retired_listing_verb_is_refused_by_name_rather_than_widening() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");

    // The scoped spelling must not quietly become the wide one.
    let scoped = sb.omh(&["s01", "ls"]); // types the retired verb on purpose
    let out = String::from_utf8_lossy(&scoped.stdout).to_string();
    let err = String::from_utf8_lossy(&scoped.stderr).to_string();
    assert!(
        !scoped.status.success(),
        "a retired verb is refused, not answered: {out}"
    );
    assert!(
        !out.contains("s02"),
        "and refusing means it never listed every session on the way: {out}"
    );
    assert!(
        err.contains("is the listing"),
        "the refusal names what replaced the verb: {err}"
    );

    // …and neither does the spelling people actually have in their fingers.
    let bare = sb.omh(&["s", "ls"]); // types the retired verb on purpose
    let err = String::from_utf8_lossy(&bare.stderr).to_string();
    assert!(!bare.status.success(), "the retired verb is not a command");
    assert!(
        err.contains("is the listing"),
        "a verb retired in favour of its own noun is one word away from what \
         the user meant, so the error says the word rather than leaving them \
         to read a usage line: {err}"
    );
}

/// A session omh said exists and then cannot find is an error, not `no
/// sessions`.
///
/// The focused listing checks the id up front, through the same
/// `existing_session` every other verb uses, and then filters the rows it
/// built independently. The two disagree about what a session *is*:
/// `existing_session` asks whether the path exists, `session::list` asks
/// whether it is a directory. Anything that is one and not the other — a
/// stray file, and a worktree removed between the check and the read, which
/// is a wide window full of subprocesses — passes the first and vanishes at
/// the second.
///
/// What the user then sees is `no sessions` on stdout with exit 0, which is
/// the same byte-for-byte answer a clean checkout gives. A question omh could
/// not answer must not render like an answer, least of all like the answer
/// *nothing is here*.
#[test]
fn a_session_that_vanishes_between_the_check_and_the_read_is_not_no_sessions() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    let worktree = sb.session("s01");

    // A plain file where a worktree would be: `exists()` says yes, `is_dir()`
    // says no. The race has the same shape and is not reproducible on demand.
    let stray = worktree.parent().unwrap().join("s02");
    std::fs::write(&stray, "").unwrap();

    let out = sb.omh(&["s02"]);
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !printed.contains("no sessions"),
        "omh looked, disagreed with itself, and reported an empty world: {printed}"
    );
    assert!(
        !out.status.success(),
        "and it exits non-zero, so a script cannot read the disagreement as \
         an answer: {printed}"
    );

    // The unfocused listing is unaffected — s01 is still there to report.
    let all = String::from_utf8_lossy(&sb.omh(&["s"]).stdout).to_string();
    assert!(all.contains("s01"), "`omh s` still answers: {all}");
}

/// `--session` is the same selector as the `sNN` prefix, including the
/// checking.
///
/// The prefix can only ever produce `s\d+`, so every assertion written
/// against it leaves `validate_id` — a path-traversal guard — unreached.
/// `--session` is the spelling that carries an arbitrary string into a path
/// join, and it had no test at all.
#[test]
fn the_long_spelling_of_the_selector_scopes_and_checks_the_same() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");

    let long = String::from_utf8_lossy(&sb.omh(&["s", "--session", "s01"]).stdout).to_string();
    let prefix = String::from_utf8_lossy(&sb.omh(&["s01"]).stdout).to_string();
    assert_eq!(
        long, prefix,
        "`omh s --session s01` and `omh s01` are one command spelled two ways"
    );

    // A name that is not a session id is refused rather than joined into a
    // path and listed as nothing.
    let traversal = sb.omh(&["s", "--session", "../../etc"]);
    let err = String::from_utf8_lossy(&traversal.stderr).to_string();
    assert!(
        !traversal.status.success(),
        "a selector that is not an id is refused: {}",
        String::from_utf8_lossy(&traversal.stdout)
    );
    // Refused for being a path, not for naming nothing. Both refuse here, so
    // only the message distinguishes them — and dropping `validate_id` would
    // leave the traversal to be judged by whether the joined path happens to
    // exist, which is a different question with the same answer today.
    assert!(
        err.contains("not a path"),
        "the refusal is about the shape of the name, not about what it \
         happens to point at: {err}"
    );
}

/// A launch whose probe cannot be read does not destroy the running sandbox.
///
/// The end-to-end half of the guard, and the one that is red on the commit
/// before it: with the probe collapsed to *this container cannot reach its
/// worktree*, omh reaches `docker rm -f` on a container it was told is
/// running, and the agent inside loses its turn.
///
/// Asserted on the call log rather than on the message, because `rm -f` is the
/// thing that costs somebody their work — a refusal that still removed the
/// container would read correctly and be the whole bug.
#[test]
fn a_launch_that_cannot_read_the_probe_removes_nothing() {
    let sb = sandbox();
    let log = sb.fake_docker();
    sb.seed_catalogue(&["adapters", "base", "editors", "stacks"]);
    sb.session("s01");
    // Running, so the launch takes the reuse path rather than building.
    std::fs::write(sb.bin.join("containers"), "omh-repo-s01\n").unwrap();
    // …and then will not let omh in, for a reason that is neither of the two
    // omh may act on: the daemon died between the two calls.
    //
    // The wording is docker 29.7.2's own, measured. An invented one — "Error
    // response from daemon: dial unix … connection refused" — carries the
    // prefix that means *the daemon answered*, so it read as `Probe::Gone` and
    // the container was replaced. Which is the right behaviour for that
    // sentence, and the wrong test.
    std::fs::write(
        sb.bin.join("docker-exec-refuses"),
        "failed to connect to the docker API at unix:///var/run/docker.sock; check if \
         the path is correct and if the daemon is running\n",
    )
    .unwrap();

    let out = sb.omh(&["s01", "resume", "claude"]);
    assert!(
        !out.status.success(),
        "the launch stops rather than guessing"
    );

    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("could not tell whether s01's sandbox is still usable"),
        "and says so: {said}"
    );
    assert!(
        said.contains("omh s01 down"),
        "with a way on that does not need the container entered: {said}"
    );

    let asked = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !asked.lines().any(|line| line.starts_with("rm -f")),
        "and nothing was removed — this is the half that costs work: {asked}"
    );
}

/// A runtime omh cannot reach is never reported as a sandbox that is stopped.
///
/// End to end, because the unit tests decide what each layer *says* and this
/// decides that the layers are wired to each other. The failure it guards is
/// specific and was live: with the Docker daemon down, `omh s` printed
/// `stopped` beside every session — in both formats, with nothing on stderr —
/// and `omh sNN sync` read the same false all-clear and would have written
/// over the files of a live agent.
#[test]
fn a_runtime_that_cannot_be_reached_is_not_reported_as_a_stopped_sandbox() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    std::fs::write(sb.bin.join("docker-refuses"), "").unwrap();

    let out = sb.omh(&["s"]);
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
    let json = sb.omh(&["s", "--json"]);
    let doc: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("`omh s --json` is a document");
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
fn the_listing_names_what_removed_sessions_left_behind() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();

    let launched = sb.home.join(".omh/run/repo/s02");
    std::fs::create_dir_all(&launched).unwrap();
    std::fs::write(launched.join("last-used"), "").unwrap();
    std::fs::create_dir_all(sb.home.join(".omh/run/repo/doctor")).unwrap();

    let out = sb.omh(&["s"]);
    // On **stderr**: a leftover is something wrong, not what `omh s` was asked
    // for, and `omh s > sessions.txt` must not collect it.
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

/// A focused listing does not report other sessions' leftovers.
///
/// A leftover is an id with a container or a run directory and **no
/// worktree**, so the focused id can never be one: `existing_session` proved
/// it has a worktree before the sweep runs. The overlap section earns its
/// place in a focused view because a collision is a fact about two sessions;
/// a leftover is a fact about neither, and it is guaranteed — not merely
/// likely — to be about somebody else. `omh s` is where orphans belong.
#[test]
fn a_focused_listing_leaves_other_sessions_leftovers_to_the_wide_one() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();
    let launched = sb.home.join(".omh/run/repo/s02");
    std::fs::create_dir_all(&launched).unwrap();
    std::fs::write(launched.join("last-used"), "").unwrap();

    let focused = sb.omh(&["s01"]);
    let aside = String::from_utf8_lossy(&focused.stderr).to_string();
    assert!(
        !aside.contains("s02") && !aside.contains("s03"),
        "asked about s01, told about s02 and s03: {aside}"
    );

    // …and the wide listing still reports them, so this narrowed the view
    // rather than dropping the fact.
    let wide = String::from_utf8_lossy(&sb.omh(&["s"]).stderr).to_string();
    assert!(
        wide.contains("s02") && wide.contains("s03"),
        "`omh s` is still where leftovers are named: {wide}"
    );
}

/// The promise in `docs/commands.md`, pinned against the command that broke it.
///
/// *stdout is the answer; stderr is everything else* was documented and then
/// contradicted by this exact invocation: the leftovers warning and its
/// `omh s rm` hint were appended to the table, so both landed in the file. A
/// prose rule nothing checks is a rule that drifts back — this is the check.
#[test]
fn a_redirected_listing_collects_the_sessions_and_nothing_else() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    std::fs::write(sb.bin.join("containers"), "omh-repo-s03\n").unwrap();

    let out = sb.omh(&["s"]);
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

    let out = sb.omh(&["s", "--json"]);
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
    /// A captured account, without running the harness's login.
    ///
    /// `auth::is_captured` asks the adapter what proves a login and checks each
    /// of those paths exists under the account's directory, so a fixture has to
    /// create exactly those — inventing a file would make the fixture stop
    /// resembling what `omh auth` leaves behind.
    fn account(&self, harness: &str, name: &str) {
        let dir = self.home.join(".omh/creds").join(harness).join(name);
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("adapters")
            .join(format!("{harness}.toml"));
        let toml = std::fs::read_to_string(&src).unwrap();
        let line = toml
            .lines()
            .find(|l| l.trim_start().starts_with("token"))
            .unwrap_or_else(|| panic!("{harness} declares no `token`, so nothing proves a login"));
        for tok in line.split(['[', ']']).nth(1).unwrap().split(',') {
            let tok = tok.trim().trim_matches('"');
            if tok.is_empty() {
                continue;
            }
            let rel = tok.trim_end_matches('/').trim_start_matches("$HOME/");
            let at = dir.join(rel);
            if tok.ends_with('/') {
                std::fs::create_dir_all(&at).unwrap();
            } else {
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                // Not `{}` — that is precisely the placeholder `auth::prepare`
                // writes, and `holds_content` reads it as *not captured*. A
                // fixture that wrote it would seed an account omh does not
                // believe in, and the test would pass over the bug.
                std::fs::write(&at, "{\"token\":\"seeded-by-the-fixture\"}").unwrap();
            }
        }
    }

    /// The shipped adapters, where `Paths::adapters()` looks. `init` would put
    /// them there and needs a container to finish.
    fn seed_adapters(&self) {
        self.seed_catalogue(&["adapters"]);
    }

    /// The shipped catalogue, or the parts of it a test needs.
    ///
    /// `init` stages all of this and needs a container to finish, so a test
    /// that drives a launch has to stand it up itself. Copied from the repo
    /// rather than written inline: a fixture that invents an adapter is a
    /// fixture that stops resembling the thing users get.
    fn seed_catalogue(&self, kinds: &[&str]) {
        for kind in kinds {
            let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(kind);
            let dst = self.home.join(".omh").join(kind);
            std::fs::create_dir_all(&dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
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
/// is, and re-running is a no-op — the rule `omh settings mcp import` already
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
        vec!["--json", "info"],
        vec!["--json", "info", "--repo"],
        vec!["--json", "settings"],
        vec!["--json", "use", "skills", "beta"],
        vec!["--json", "unuse", "skills", "beta"],
        vec!["--json", "set", "codegraph", "off"],
        vec!["--json", "s"],
        vec!["--json", "memory", "lint"],
    ] {
        let out = sb.omh(&args);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        // Invoked, not merely attempted. Skipping on empty stdout is how this
        // guard went quiet twice: clap writes `unrecognized subcommand` to
        // stderr, so a line naming a verb that had been retired left stdout
        // empty and read as *nothing to say*. Both `ls` entries here were dead
        // that way — one of them for the verb this very list was meant to
        // cover. A guard that skips what it cannot invoke guards nothing.
        assert!(
            out.status.success(),
            "`omh {}` did not run: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !stdout.trim().is_empty(),
            "`omh {}` answered with nothing on stdout",
            args.join(" ")
        );
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
