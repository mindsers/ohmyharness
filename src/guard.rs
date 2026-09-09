//! Guards — hooks whose action is `refuse`, opt-in via the catalogue.
//!
//! This module owns no runtime behaviour. The predicate a guard actually runs
//! lives in the hook's own `when`, in `hooks/*.json`, because that is what
//! ships to the sandbox — a Rust function here would be a second copy of the
//! rule, free to disagree with the first. What this module holds instead is an
//! **oracle**: an independent, pure description of the same rule (`Role`,
//! `Coverage`), used only to check the shipped predicate against, over a table
//! of paths a hand-picked example set would not have covered.
//!
//! `#[cfg(test)]`, deliberately: nothing at runtime consumes `Role` or
//! `Coverage`. A `pub fn` with no runtime caller is dead code under
//! `-D warnings`, and giving it one only to satisfy the linter would be the
//! second copy this module exists to avoid.

#![cfg(test)]

use std::path::Path;

/// What a path *is*, within one stack's convention for pairing source and
/// test. `Neither` covers everything the convention has no opinion about —
/// including, for a stack with no guard, every path there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Source,
    Test,
    Neither,
}

/// Where a `Source`'s test could live, most-likely-first. A guard allows the
/// edit if *any* candidate is dirty — one changed test is enough to say "the
/// test came first."
pub fn coverage(stack: &str, path: &Path) -> Vec<String> {
    let s = path.to_string_lossy();
    match stack {
        "go" => {
            if let Some(base) = s.strip_suffix(".go") {
                vec![format!("{base}_test.go")]
            } else {
                vec![]
            }
        }
        "python" => {
            let Some(base) = s.strip_suffix(".py") else {
                return vec![];
            };
            let (dir, name) = match base.rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), n),
                None => (String::new(), base),
            };
            vec![
                format!("{dir}test_{name}.py"),
                format!("{dir}tests/test_{name}.py"),
            ]
        }
        "node" => {
            let exts = ["ts", "tsx", "js", "jsx"];
            let Some(ext) = exts.iter().find(|e| s.ends_with(&format!(".{e}"))) else {
                return vec![];
            };
            let base = s.strip_suffix(&format!(".{ext}")).unwrap();
            let (dir, name) = match base.rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), n),
                None => (String::new(), base),
            };
            vec![
                format!("{base}.test.{ext}"),
                format!("{base}.spec.{ext}"),
                format!("{dir}__tests__/{name}.test.{ext}"),
            ]
        }
        _ => vec![],
    }
}

/// `Role::of` is the one place "is this a test file" is decided, so a stack
/// whose test convention is not source-file suffix has to say `Neither` here
/// rather than let `coverage` guess — `coverage` is never asked about a `Test`
/// or `Neither` path in the first place.
pub fn role(stack: &str, path: &Path) -> Role {
    let s = path.to_string_lossy();
    match stack {
        "go" => {
            if s.ends_with("_test.go") {
                Role::Test
            } else if s.ends_with(".go") {
                Role::Source
            } else {
                Role::Neither
            }
        }
        "python" => {
            let name = path.file_name().map(|f| f.to_string_lossy().to_string());
            let is_test_name = name.is_some_and(|n| n.starts_with("test_") || n == "conftest.py");
            if !s.ends_with(".py") {
                Role::Neither
            } else if is_test_name || s.contains("/tests/") || s.starts_with("tests/") {
                Role::Test
            } else {
                Role::Source
            }
        }
        "node" => {
            let is_dts = s.ends_with(".d.ts");
            let is_test = s.contains(".test.") || s.contains(".spec.") || s.contains("__tests__/");
            let is_source = ["ts", "tsx", "js", "jsx"]
                .iter()
                .any(|e| s.ends_with(&format!(".{e}")));
            if is_dts {
                Role::Neither
            } else if is_test {
                Role::Test
            } else if is_source {
                Role::Source
            } else {
                Role::Neither
            }
        }
        _ => Role::Neither,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_file_is_never_a_source_file() {
        let cases = [
            ("go", "foo_test.go"),
            ("go", "pkg/bar_test.go"),
            ("python", "test_foo.py"),
            ("python", "tests/test_foo.py"),
            ("python", "src/tests/test_bar.py"),
            ("python", "conftest.py"),
            ("node", "foo.test.ts"),
            ("node", "foo.spec.tsx"),
            ("node", "__tests__/foo.test.js"),
            ("node", "foo.d.ts"),
        ];
        for (stack, path) in cases {
            assert_ne!(
                role(stack, Path::new(path)),
                Role::Source,
                "{stack}: {path} must not be Source"
            );
        }
    }

    #[test]
    fn coverage_names_every_place_the_test_could_be() {
        assert_eq!(
            coverage("go", Path::new("pkg/foo.go")),
            vec!["pkg/foo_test.go"]
        );
        assert_eq!(
            coverage("python", Path::new("src/a/b.py")),
            vec!["src/a/test_b.py", "src/a/tests/test_b.py"]
        );
        assert_eq!(
            coverage("node", Path::new("src/a.ts")),
            vec!["src/a.test.ts", "src/a.spec.ts", "src/__tests__/a.test.ts"]
        );
    }

    #[test]
    fn a_path_outside_the_stack_has_no_role() {
        for (stack, path) in [
            ("go", "README.md"),
            ("go", "go.mod"),
            ("python", "pyproject.toml"),
            ("node", "package.json"),
            ("node", "README.md"),
        ] {
            assert_eq!(
                role(stack, Path::new(path)),
                Role::Neither,
                "{stack}: {path}"
            );
        }
    }
}

/// The rendered guards — run against the real `claude` adapter, in a real
/// git repository, exactly the way `no_shipped_hook_refuses_a_git_command`
/// (`src/base.rs`) proves a hook by firing it rather than by reading it.
#[cfg(test)]
mod rendered {
    use super::*;
    use crate::adapter::{Adapter, Capability};
    use crate::hook::{Hook, Outcome};
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const HOOKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hooks");

    /// One catalogue hook, rendered for claude — the harness every guard has
    /// to work on. `hook::render` is what a hand-written predicate has to
    /// survive: claude's own field-extraction preamble, not the raw JSON.
    fn claude_command(hook_name: &str) -> String {
        let path = PathBuf::from(HOOKS).join(format!("{hook_name}.json"));
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let hook = Hook::parse(&raw, hook_name).unwrap();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let binding = adapter
            .supports(Capability::Hooks)
            .expect("claude has hooks");
        match crate::hook::render(hook_name, &hook, binding, &adapter.tools).unwrap() {
            Outcome::Rendered(r) => r.command,
            Outcome::Dropped(d) => panic!("claude cannot express {hook_name}: {d}"),
        }
    }

    /// Feeds `file_path` to the rendered command as Claude's own payload
    /// shape and reads back whether it refused. Claude's protocol is
    /// silence-means-proceed: a hook that does not print a
    /// `permissionDecision` allows the call, exactly as one that exits
    /// before ever reading stdin does — `when` declining and a `refuse`
    /// firing are the only two shapes this hook can produce, so this
    /// returns a plain `bool` rather than an `Option`.
    ///
    /// Deliberately spawned from a directory that has nothing to do with
    /// `file_path` — every guard resolves its own directory from
    /// `$OMH_TOOL_FILE` and passes it to `git -C` explicitly, so the process
    /// `cwd` must not matter. A caller passing the file's own repo as `cwd`
    /// would make a predicate that silently depended on cwd anyway (e.g. a
    /// bare `git status` with no `-C`) pass every test by coincidence, since
    /// cwd and dirname would always agree.
    fn refused(command: &str, file_path: &str) -> bool {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh must run");
        let payload = serde_json::json!({ "tool_input": { "file_path": file_path } });
        if let Err(e) = child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
        {
            // Every guard mentions `$OMH_TOOL_FILE`, so `hook::render`
            // prepends `p=$(cat)` ahead of `when` — stdin is always read in
            // full before a guard can decide anything. This branch cannot
            // fire for a guard; it is kept only because the base.rs pattern
            // this helper follows needs it for hooks with no fields at all
            // (`graph-refresh`, `git-turn`), where `when` can exit before
            // reading stdin.
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "{e}");
        }
        let out = child.wait_with_output().unwrap();
        // Exit status first, and asserted rather than swallowed: none of
        // these hooks use Claude's *other* block mechanism (exiting 2 with a
        // reason on stderr, which prints no JSON) — `no_shipped_hook_refuses_a_git_command`
        // in `src/base.rs` is where that path is legitimately checked. Here a
        // nonzero exit means the rendered command itself is broken, and
        // reading that as "did not refuse" would report a crashed guard as a
        // working one.
        assert!(
            out.status.success(),
            "guard exited {:?} on `{command}`: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
            return false;
        };
        parsed
            .get("hookSpecificOutput")
            .and_then(|h| h.get("permissionDecision"))
            .and_then(|d| d.as_str())
            == Some("deny")
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A repo with `source` and its paired test both committed clean — the
    /// state a guard must allow, since nothing has been touched yet.
    fn repo_with(source: &str, test_file: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        for f in [source, test_file] {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    /// One representative source/test pair per stack, all run through the
    /// same hook — `tdd-guard` covers every stack itself, dispatching on
    /// extension the way `config-guard` dispatches on path. Table-driven
    /// rather than three copies of the same five tests, so a fourth stack is
    /// one more row and never a fourth catalogue entry: a monorepo with go
    /// and node turns on one name, `tdd-guard`, not two.
    const STACKS: &[(&str, &str, &str)] = &[
        ("go", "foo.go", "foo_test.go"),
        ("python", "foo.py", "test_foo.py"),
        ("node", "foo.ts", "foo.test.ts"),
    ];

    #[test]
    fn a_clean_test_file_refuses_the_source_edit() {
        let command = claude_command("tdd-guard");
        for (stack, source, test) in STACKS {
            let dir = repo_with(source, test);
            let file = dir.path().join(source);
            assert!(
                refused(&command, &file.to_string_lossy()),
                "{stack}: the test file has not moved since the last commit"
            );
        }
    }

    #[test]
    fn a_dirty_test_file_allows_it() {
        let command = claude_command("tdd-guard");
        for (stack, source, test) in STACKS {
            let dir = repo_with(source, test);
            std::fs::write(dir.path().join(test), "changed").unwrap();
            let file = dir.path().join(source);
            assert!(!refused(&command, &file.to_string_lossy()), "{stack}");
        }
    }

    #[test]
    fn editing_the_test_itself_is_never_refused() {
        let command = claude_command("tdd-guard");
        for (stack, source, test) in STACKS {
            let dir = repo_with(source, test);
            let file = dir.path().join(test);
            assert!(!refused(&command, &file.to_string_lossy()), "{stack}");
        }
    }

    #[test]
    fn a_file_with_no_role_is_never_refused() {
        let command = claude_command("tdd-guard");
        for (stack, source, test) in STACKS {
            let dir = repo_with(source, test);
            let file = dir.path().join("README.md");
            assert!(!refused(&command, &file.to_string_lossy()), "{stack}");
        }
    }

    #[test]
    fn a_guard_that_cannot_ask_git_allows() {
        let command = claude_command("tdd-guard");
        for (stack, source, _test) in STACKS {
            // Not a git repository at all — `git status` fails outright, and
            // the guard has to read failure as "cannot tell" rather than as
            // "refuse". This is the case the invariant exists for.
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(source), "x").unwrap();
            let file = dir.path().join(source);
            assert!(
                !refused(&command, &file.to_string_lossy()),
                "{stack}: no git repository at all — a hard failure, not a refusal"
            );
        }
    }

    /// Distinct from the case above: git answers successfully, but the test
    /// file has never existed — that is not "cannot tell", it is the
    /// ordinary TDD state "no test written yet", and a guard is allowed to
    /// refuse it exactly as it refuses an untouched-but-existing test.
    #[test]
    fn no_test_file_at_all_still_refuses() {
        let command = claude_command("tdd-guard");
        for (stack, source, _test) in STACKS {
            let dir = tempfile::tempdir().unwrap();
            git(dir.path(), &["init", "-q"]);
            std::fs::write(dir.path().join(source), "x").unwrap();
            git(dir.path(), &["add", "-A"]);
            git(dir.path(), &["commit", "-q", "-m", "init"]);
            let file = dir.path().join(source);
            assert!(
                refused(&command, &file.to_string_lossy()),
                "{stack}: git answered fine; there is simply no test yet"
            );
        }
    }

    /// python and node each name more than one place the test could be —
    /// `coverage()`'s whole reason to return a `Vec`. Driven off `coverage()`
    /// itself rather than one hand-picked candidate: a predicate that
    /// dropped a candidate, or spelled `__tests__` wrong, fails against the
    /// model instead of against a literal this test also typed.
    #[test]
    fn any_dirty_candidate_is_enough() {
        let command = claude_command("tdd-guard");
        for (stack, source) in [("python", "src/foo.py"), ("node", "src/foo.ts")] {
            let candidates = coverage(stack, Path::new(source));
            assert!(!candidates.is_empty(), "{stack}: no candidates to check");
            for candidate in &candidates {
                // Every *other* candidate exists too, clean — a real repo
                // would have all of them committed; only `candidate` moves.
                let dir = tempfile::tempdir().unwrap();
                git(dir.path(), &["init", "-q"]);
                for f in std::iter::once(source).chain(candidates.iter().map(String::as_str)) {
                    let p = dir.path().join(f);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, "x").unwrap();
                }
                git(dir.path(), &["add", "-A"]);
                git(dir.path(), &["commit", "-q", "-m", "init"]);
                std::fs::write(dir.path().join(candidate), "changed").unwrap();
                let file = dir.path().join(source);
                assert!(
                    !refused(&command, &file.to_string_lossy()),
                    "{stack}: dirtying {candidate} alone should be enough"
                );
            }
        }
    }

    /// The oracle in `super::{role, coverage}` and the shipped predicate are
    /// two independent descriptions of the same rule. This is the test that
    /// makes them answer to each other, over paths a hand-picked example set
    /// would not have hit.
    #[test]
    fn every_shipped_guard_agrees_with_the_model() {
        let cases: &[(&str, &str)] = &[
            ("go", "pkg/thing.go"),
            ("go", "pkg/thing_test.go"),
            ("go", "README.md"),
            ("python", "pkg/thing.py"),
            ("python", "pkg/test_thing.py"),
            ("python", "pkg/tests/thing.py"),
            ("python", "pyproject.toml"),
            // A `*/test_*.py` glob spans `/`, so it also matches a *source*
            // file that merely sits under a directory starting with
            // `test_` — `role()` only checks the basename. The drift this
            // catches: the shipped predicate reading `src/test_utils/x.py`
            // as a test file it may never touch, when the oracle says it is
            // an ordinary source file with no test of its own.
            ("python", "src/test_utils/helper.py"),
            // The merged hook's node arm used to check `*.test.*` as a
            // top-level pattern, ahead of the python fallback — so a python
            // file whose name merely *contains* `.test.` (an unusual name,
            // but a plain source file) was read as an already-tested node
            // file and never refused, while the oracle — which only reads
            // `test_`-prefix and `/tests/` for python — says Source.
            ("python", "foo.test.py"),
            ("node", "src/thing.ts"),
            ("node", "src/thing.test.ts"),
            ("node", "src/thing.d.ts"),
            ("node", "deep/__tests__/x.ts"),
            ("node", "package.json"),
        ];
        let command = claude_command("tdd-guard");
        for (stack, rel) in cases {
            let source_role = role(stack, Path::new(rel));
            // A guard oracle only has an opinion about `Source` paths — for
            // `Test`/`Neither` the invariant is "always allow", already
            // covered by the tests above. What this checks is the boundary
            // itself: the model must not silently call something `Source`
            // that the shipped predicate's `case` arms do not recognise
            // either, or the two have drifted.
            let dir = repo_with(rel, "unrelated_test_file_that_never_matches.never");
            let file = dir.path().join(rel);
            let claude_refuses = refused(&command, &file.to_string_lossy());
            match source_role {
                Role::Source => assert!(
                    claude_refuses,
                    "{stack}/{rel}: oracle says Source, no matching test exists, \
                     the shipped predicate must refuse"
                ),
                Role::Test | Role::Neither => assert!(
                    !claude_refuses,
                    "{stack}/{rel}: oracle says {source_role:?}, the shipped \
                     predicate must never refuse it"
                ),
            }
        }
    }

    /// A monorepo names one guard, and it has to cover every stack in it at
    /// once — the whole reason `tdd-guard` is one file rather than one per
    /// stack. A go edit with a dirty test and a python edit with a clean one,
    /// against the *same rendered command*, in the *same repo*.
    #[test]
    fn one_guard_covers_every_stack_in_a_monorepo() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        for f in ["go/foo.go", "go/foo_test.go", "py/bar.py", "py/test_bar.py"] {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("py/test_bar.py"), "changed").unwrap();

        let command = claude_command("tdd-guard");
        assert!(
            refused(&command, &dir.path().join("go/foo.go").to_string_lossy()),
            "go's test has not moved"
        );
        assert!(
            !refused(&command, &dir.path().join("py/bar.py").to_string_lossy()),
            "python's test just did, in the same repo, under the same guard"
        );
    }

    /// A decision, named rather than left implicit: `git status` compares
    /// against `HEAD`, so once the agent commits, the test file it just
    /// touched reads as clean again — the next source edit is refused until
    /// a test is touched *again*. That is per-cycle TDD (every red/green
    /// round starts with a test edit), not a bug, but it is a real behaviour
    /// nobody wrote down until this test.
    #[test]
    fn a_committed_test_no_longer_counts_as_touched() {
        let dir = repo_with("foo.go", "foo_test.go");
        std::fs::write(dir.path().join("foo_test.go"), "changed").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "test change"]);
        let command = claude_command("tdd-guard");
        let file = dir.path().join("foo.go");
        assert!(
            refused(&command, &file.to_string_lossy()),
            "the test change is now in HEAD, so `git status` reports it clean again"
        );
    }

    #[test]
    fn config_guard_refuses_edits_under_omh() {
        let command = claude_command("config-guard");
        assert!(refused(&command, "/work/.omh/settings.toml"));
        assert!(refused(&command, "/work/.omh/hooks/x.json"));
    }

    #[test]
    fn config_guard_allows_the_memory_store() {
        let command = claude_command("config-guard");
        assert!(!refused(&command, "/work/.omh/notes/2026-09-08-x.md"));
    }

    #[test]
    fn config_guard_allows_everything_outside_omh() {
        let command = claude_command("config-guard");
        assert!(!refused(&command, "/work/src/main.rs"));
        assert!(!refused(&command, "/work/AGENTS.md"));
    }

    /// omp's `edit` tool takes one `input` string with the path embedded in a
    /// `[PATH#TAG]` payload, not a `path` field — `adapters/omp.toml:159`
    /// records the gap, and the check for it lives in `render::omp_plugin`
    /// (`render.rs:829`), not in the shared `hook::render` every harness
    /// calls into. So this has to go through `render::document` — the
    /// function a real launch actually calls — rather than `hook::render`
    /// directly, or it would silently miss the one special case it exists to
    /// prove. Until §1b closes it, every guard has to be dropped *by name*
    /// there, never silently absent.
    #[test]
    fn every_guard_is_dropped_by_name_on_omp() {
        let adapter = Adapter::find(Path::new(ADAPTERS), "omp").unwrap();
        let binding = adapter.supports(Capability::Hooks).expect("omp has hooks");
        // `sources` is layer *directories*, scanned for every `*.json` in
        // them — matching how `profile::sources` hands `render::document`
        // the whole catalogue dir, never one file at a time.
        let sources = vec![PathBuf::from(HOOKS)];
        let doc = crate::render::document(
            Capability::Hooks,
            binding,
            &sources,
            &crate::base::Own::default(),
            &crate::settings::RepoPolicy::default(),
            &adapter.tools,
            &Default::default(),
        )
        .unwrap();
        for name in ["tdd-guard", "config-guard"] {
            assert!(
                doc.dropped.iter().any(|d| d.name == name),
                "{name}: expected in `dropped`, got: {:?} / body: {}",
                doc.dropped,
                doc.body
            );
        }
    }

    /// codex has no hooks capability at all (`adapters/codex.toml`), so the
    /// launcher gives up the whole capability rather than naming one hook —
    /// that path is exercised in `container.rs`. What belongs to this guard
    /// specifically is that codex is not silently offered the option: there
    /// is no `[capabilities.hooks]` table to render against.
    #[test]
    fn codex_has_no_hooks_capability_to_render_against() {
        let adapter = Adapter::find(Path::new(ADAPTERS), "codex").unwrap();
        assert!(adapter.supports(Capability::Hooks).is_none());
    }
}
