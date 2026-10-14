//! Process-global state is shared by every test in a binary.
//!
//! `cargo test` runs tests as threads of one process, and a child any of them
//! spawns inherits that process's working directory. A test that moves it with
//! `set_current_dir` moves it for every test beside it. When the directory it
//! moved to is a `TempDir` that is then dropped, a shell spawned in that window
//! starts somewhere that no longer exists — and bash (`shell-init: error
//! retrieving current directory`) and dash (`sh: 0: getcwd() failed`) both say
//! so on stderr while still exiting 0.
//!
//! This is the cause found for #102.
//! `hook::tests::every_accepted_inject_reaches_the_agent_intact`, which asserts
//! an empty stderr, failed once under a full parallel run and passed in
//! isolation. Started from a deleted directory, it fails the same way: `sh`
//! exits 0 with that line on stderr. The same move made
//! `image::tests::a_corporate_ca_reaches_the_image_and_moves_its_tag` fail
//! intermittently: git exits 128 with `Unable to read current working
//! directory`.
//!
//! `src/image.rs` already states the rule beside its digest test. This gives it
//! a test that goes red without it: no line of code under `src/` or `tests/`,
//! nor in `build.rs`, names `set_current_dir` or calls `chdir`. It reads text,
//! not the compiled crate, so a comment is skipped only when it is the whole
//! line, and a module declared from outside those directories with `#[path]`
//! is not read.

use std::fs;
use std::path::{Path, PathBuf};

/// What moves the working directory: the std function by name — called,
/// imported or passed — and the libc call under it. Spelled in halves so this
/// file is read like any other.
const MOVES: [&str; 2] = [concat!("set_current", "_dir"), concat!("ch", "dir(")];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `dir`. Walked rather than listed, so a new module is
/// covered the moment it exists.
///
/// A directory or entry it cannot read is a failure, not an empty one: a scan
/// that read nothing agrees with anything.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for e in entries {
        let p = e
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .path();
        // `is_dir()` reads a failed stat as "not a directory", which would drop
        // a directory it could not look into.
        let meta = fs::metadata(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()));
        if meta.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn nothing_moves_the_process_working_directory() {
    let root = repo();
    // Pushed by hand rather than walked; reading it below fails if it is gone.
    let mut files = vec![root.join("build.rs")];
    for dir in ["src", "tests"] {
        rust_files(&root.join(dir), &mut files);
    }
    // By name, because a scan that read less than it should agrees with
    // anything:
    // - `src/image.rs` is where the call was, and `src/hook.rs` holds the test
    //   it broke;
    // - `src/memory/tools.rs` is two levels down, so a walk that stops
    //   recursing is caught;
    // - `tests/cli.rs` is under the second root.
    for read in [
        "src/image.rs",
        "src/hook.rs",
        "src/memory/tools.rs",
        "tests/cli.rs",
    ] {
        assert!(
            files.contains(&root.join(read)),
            "the scan never reached {read}: it moved, and this list should follow \
             it, or the walk stopped short"
        );
    }

    let mut offenders = Vec::new();
    for file in &files {
        let text =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            // Prose about the rule is not a breach of it.
            if code.starts_with("//") {
                continue;
            }
            if MOVES.iter().any(|m| code.contains(m)) {
                let rel = file.strip_prefix(&root).unwrap_or(file);
                offenders.push(format!("{}:{}: {code}", rel.display(), n + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "moving the working directory moves it for the whole test process, so \
         every child any other test spawns can start in this test's directory — \
         or in one that has already been deleted. Pass a directory to the code \
         under test, or give a spawned command its own `current_dir`, instead:\n{}",
        offenders.join("\n")
    );
}
