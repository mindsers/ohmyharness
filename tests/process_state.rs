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
//! That is how `hook::tests::every_accepted_inject_reaches_the_agent_intact`,
//! which asserts an empty stderr, failed once under a full parallel run and
//! passed in isolation (#102). Starting the test binary from a deleted
//! directory reproduces the failure message exactly. The same move made
//! `image::tests::a_corporate_ca_reaches_the_image_and_moves_its_tag` fail
//! intermittently: git exits 128 with `Unable to read current working
//! directory`.
//!
//! `src/image.rs` already states the rule beside its digest test. This gives it
//! a test that goes red without it: nothing in this crate moves the working
//! directory.

use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `dir`. Walked rather than listed, so a new module is
/// covered the moment it exists.
///
/// A directory it cannot read is a failure, not an empty one: a scan that read
/// nothing agrees with anything.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for e in entries {
        let p = e
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn nothing_moves_the_process_working_directory() {
    let root = repo();
    let mut files = vec![root.join("build.rs")];
    for dir in ["src", "tests"] {
        rust_files(&root.join(dir), &mut files);
    }
    // By name, because a scan that stopped reading where the call was, or
    // where the test it broke lives, would still pass.
    for read in [
        "build.rs",
        "src/image.rs",
        "src/hook.rs",
        "src/memory/tools.rs",
        "tests/cli.rs",
    ] {
        assert!(
            files.iter().any(|f| f.ends_with(read) && f.is_file()),
            "the scan no longer reads {read}"
        );
    }
    // This file names the call in order to forbid it.
    let this = root.join(file!());

    let mut offenders = Vec::new();
    for file in files.iter().filter(|f| **f != this) {
        let text =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            // Prose about the rule is not a breach of it.
            if code.starts_with("//") {
                continue;
            }
            if code.contains("set_current_dir(") {
                let rel = file.strip_prefix(&root).unwrap_or(file);
                offenders.push(format!("{}:{}: {code}", rel.display(), n + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`set_current_dir` moves the working directory of the whole test process, \
         so every child any other test spawns can start in this test's directory — \
         or in one that has already been deleted. Pass a directory to the code \
         under test, or give a spawned command its own `current_dir`, instead:\n{}",
        offenders.join("\n")
    );
}
