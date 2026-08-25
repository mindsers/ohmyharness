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
                 if [ \"$1\" = inspect ]; then echo true; fi\n\
                 if [ \"$1\" = ps ]; then cat {containers} 2>/dev/null; fi\nexit 0\n",
                log = log.display(),
                refuses = self.bin.join("docker-refuses").display(),
                exec_refuses = self.bin.join("docker-exec-refuses").display(),
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
/// `omh ls` with the session dropped.
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
    assert!(sb
        .omh(&["repo", "set", "runtime", "bogus"])
        .status
        .success());
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
/// `omh attach` created for an editor without ever running a harness in it.
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

    let wrong = sb.omh(&["repo", "set", "persistence", "tmux"]);
    let said = String::from_utf8_lossy(&wrong.stderr).to_string();
    assert!(wrong.status.success(), "still written: {said}");
    assert!(
        said.contains("tmux") && said.contains("dtach"),
        "the value it cannot take, and the ones it can: {said}"
    );

    // One it can take says nothing — a warning on every write is no warning.
    let right = sb.omh(&["repo", "set", "persistence", "dtach"]);
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

    let risky = sb.omh(&["repo", "set", "--shared", "carry_in", "[\".env\"]"]);
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
    let safe = sb.omh(&["repo", "set", "--shared", "account", "work"]);
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

    let typo = sb.omh(&["repo", "set", "carry_ins", "[\".env\"]"]);
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
    let known = sb.omh(&["repo", "set", "carry_in", "[\".env\"]"]);
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
        vec!["--session", "s01", "ls"],
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

/// A verb that was retired is refused by name, and never becomes another
/// command.
///
/// The `ls` verb was the documented spelling until 2026.08, so it is in muscle
/// memory and in scripts. Retiring it left two ways to get this wrong, and
/// only one of them is harmless.
///
/// Typing it bare is: clap rejects an unknown subcommand. `omh s01 ls` is
/// not. With no `ls` under `sessions` the sessions reading fails to parse,
/// the as-written reading `omh ls` parses as the **top-level inventory**, and
/// `session_prefix`'s fallback hands that reading the launch because it is
/// not a `Cmd::Run`. `Cmd::Ls` never reads `cli.session`, so the session is
/// dropped in silence and every session is listed — which is verbatim the
/// harm the refusal removed in #67 existed to prevent: *"it would list every
/// session and look like it had listed one."*
///
/// So the verb survives as a tombstone rather than as a hole. Deleting it
/// from the parser did not make the line unspellable, only unrefusable.
#[test]
fn the_retired_listing_verb_is_refused_by_name_rather_than_widening() {
    let sb = sandbox();
    let _log = sb.fake_docker();
    sb.session("s01");
    sb.session("s02");

    // The scoped spelling must not quietly become the wide one.
    let scoped = sb.omh(&["s01", "ls"]);
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
        vec!["--json", "s"],
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
