//! Detection — how `omh init` decides without asking.
//!
//! Every question init would ask is hassle we promised to remove, and the
//! answers are mostly lying around already: manifests name the stack, git log
//! names what you work on, the README names the project. Deriving beats
//! interrogating twice over — no wizard, and the facts refresh themselves when
//! the repo changes instead of going stale in a config file.

use std::path::Path;

/// A stack this repo turned out to be, with the commands omh's conventional
/// hooks run for it.
///
/// A **view**, built at detection time from a `stack::Definition` — which is
/// the thing that ships, and which carries no commands. The two command fields
/// here are the join between what a project *is* and what omh's hook opinion
/// does about it, and they are owed to build-order item 7: once conventional
/// hooks ship as hook files, this type loses them and becomes a borrow of the
/// definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub name: String,
    pub marker: String,
    pub test: String,
    pub format: String,
}

/// What omh's conventional hooks run, per stack.
///
/// **Owed to build-order item 7**, and deliberately here rather than in the
/// stack file. A stack says what a project needs *installed*; a command belongs
/// to a hook, and hooks already have two homes with a defined precedence. A
/// `test =` key in `stacks/rust.toml` would be a third copy of the same string,
/// free to disagree with the other two — and contributors would then write one
/// for every stack they add.
///
/// So this stays in code until conventional hooks ship as catalogue hook files,
/// and `every_conventional_command_is_provisioned` ties it to the shipped data
/// meanwhile: a command whose program no provide installs is a hook that fails
/// on every turn.
fn conventional(stack: &str) -> Option<(&'static str, &'static str)> {
    match stack {
        "rust" => Some(("cargo test", "cargo fmt")),
        "node" => Some(("npm test", "npm run format")),
        "python" => Some(("pytest", "ruff format .")),
        "go" => Some(("go test ./...", "gofmt -w .")),
        _ => None,
    }
}

/// A derived fact, with the source that produced it. The source is not
/// decoration — `omh why` has to be able to explain every default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub source: String,
    pub fact: String,
}

/// Which executable a command needs in order to run at all.
///
/// This is the generic kernel of toolchain detection, and it is deliberately
/// ignorant of every stack omh ships: omh is not a rust tool that also knows
/// npm. Give it a command string — one of ours or one somebody wrote by hand —
/// and it names the program that has to be on PATH.
///
/// `None` means *cannot tell*, never *nothing is needed*. The two failure
/// directions are not symmetric: missing a gap costs one confusing hook error,
/// which is the status quo, while inventing one makes omh drop a working hook
/// or interrogate somebody about a toolchain they already have. Everything
/// ambiguous therefore resolves to `None`, and no caller may act on it.
pub fn program(command: &str) -> Option<&str> {
    let word = command.split_whitespace().find(|w| !is_assignment(w))?;
    is_program_name(word).then_some(word)
}

/// Is this word a program name, or is it shell?
///
/// An **allowlist**. A list of metacharacters to reject says nothing about the
/// syntax it failed to think of, and the cost of missing one is not cosmetic:
/// `$(which cargo) test` yields a candidate of `$(which`, which resolves
/// nowhere, so omh reports a missing program in a repo whose cargo is fine,
/// asks about it, and records an answer that switches a working hook off for
/// everyone who clones. That is the expensive failure direction this module
/// opens by naming.
///
/// The set is what a program name is actually made of — a bare name, a version
/// suffix (`python3.11`), a path (`./scripts/test.sh`), a `+` or `-` in the
/// name (`g++`, `cargo-nextest`). Anything else is *cannot tell*.
fn is_program_name(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+'))
}

/// `FOO=1 cargo test` runs cargo. A leading assignment is an ordinary shape for
/// a test command and names no executable, so reporting `FOO=1` missing would
/// be a gap omh invented. Recognising one word too many is safe — it costs a
/// `None` — so this stays permissive on purpose.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// A stack by name, for reading a hook filename back into the marker it
/// implies. The launcher needs this to notice a hook whose stack has gone.
pub fn known(stacks: &[crate::stack::Definition], name: &str) -> Option<Stack> {
    stacks.iter().find(|s| s.name == name).and_then(view)
}

/// Which of these stacks this repo is, as views with their commands.
///
/// Takes the definitions rather than loading them, so this stays a pure
/// function of *what omh ships* and *what is on disk here* — and so a caller
/// that has already loaded them does not read the directory twice.
pub fn stacks(defs: &[crate::stack::Definition], repo: &Path) -> Vec<Stack> {
    crate::stack::detected(defs, repo)
        .into_iter()
        .filter_map(view)
        .collect()
}

/// Every stack omh ships, loaded from the repository's own `stacks/`.
///
/// The replacement for the `KNOWN` constant the tests used to iterate, and it
/// reads exactly what production reads — so a guard cannot pass against a
/// hardcoded list while the shipped data says something else. Two registries
/// free to disagree is the failure this whole step removes.
#[cfg(test)]
pub(crate) fn shipped() -> Vec<crate::stack::Definition> {
    crate::stack::load_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/stacks")))
        .expect("the shipped stacks must load")
}

/// Those of them omh has a hook opinion about, as views.
#[cfg(test)]
pub(crate) fn all_shipped() -> Vec<Stack> {
    shipped().iter().filter_map(view).collect()
}

/// A definition omh has a hook opinion about becomes a detected stack; one it
/// does not is detected and simply gets no conventional hooks.
///
/// That is the honest outcome for a contributed stack today: omh can provision
/// an ecosystem it was taught about in four lines of TOML, and has nothing to
/// say about how that ecosystem tests itself until item 5 derives it or item 6
/// asks. Silently inventing `elixir test` would be the wrong kind of helpful.
fn view(def: &crate::stack::Definition) -> Option<Stack> {
    let (test, format) = conventional(&def.name)?;
    Some(Stack {
        name: def.name.clone(),
        marker: def.marker.clone(),
        test: test.to_string(),
        format: format.to_string(),
    })
}

/// Which harness to default to. Host evidence is only a *hint*: the harness
/// itself runs in the sandbox, so this picks a preference, never an install.
pub fn preferred_harness(
    candidates: &[String],
    installed_on_host: &dyn Fn(&str) -> bool,
) -> Option<String> {
    candidates
        .iter()
        .find(|c| installed_on_host(c))
        .or_else(|| candidates.first())
        .cloned()
}

/// Facts derived for memory. No questions asked.
pub fn seeds(defs: &[crate::stack::Definition], repo: &Path) -> Vec<Seed> {
    let mut out = Vec::new();

    // First non-heading, non-empty line of the README is the project in one line.
    if let Ok(readme) = std::fs::read_to_string(repo.join("README.md")) {
        // A tagline under the title is the commonest README shape, and it is
        // usually a blockquote. Badges sit in the same place and say nothing.
        if let Some(line) = readme
            .lines()
            .map(|l| l.trim().trim_start_matches('>').trim())
            .find(|l| {
                !l.is_empty()
                    && !l.starts_with('#')
                    && !l.starts_with("[!")
                    && !l.starts_with("![")
                    && !l.starts_with("---")
            })
        {
            out.push(Seed {
                source: "README.md".into(),
                fact: line.to_string(),
            });
        }
    }

    for s in stacks(defs, repo) {
        out.push(Seed {
            source: s.marker.clone(),
            fact: format!(
                "stack: {} (test `{}`, format `{}`)",
                s.name, s.test, s.format
            ),
        });
    }

    // Conventions the project already wrote down outlive any interview.
    for rules in ["AGENTS.md", "CLAUDE.md"] {
        if let Ok(body) = std::fs::read_to_string(repo.join(rules)) {
            if !body.trim().is_empty() {
                out.push(Seed {
                    source: rules.into(),
                    fact: format!("existing conventions ({} lines)", body.lines().count()),
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for (name, body) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (dir, root)
    }

    // ── the program a command needs ──────────────────────────────────────────

    /// The generic kernel. omh is not a rust tool that also knows npm: it takes
    /// a command string it was given and reports which executable has to exist
    /// for that string to run. Every shipped stack goes through here, and so
    /// does a command somebody wrote by hand.
    #[test]
    fn the_program_a_command_needs_is_the_word_that_runs() {
        for (command, want) in [
            ("cargo test", "cargo"),
            ("npm run format", "npm"),
            ("pytest", "pytest"),
            ("ruff format .", "ruff"),
            ("go test ./...", "go"),
            ("gofmt -w .", "gofmt"),
        ] {
            assert_eq!(program(command), Some(want), "for {command:?}");
        }
    }

    /// The asymmetry that governs this whole feature. Failing to spot a
    /// missing program costs one confusing hook error — the status quo. Naming
    /// a program missing when it is not is worse: omh drops a hook that works,
    /// or interrogates somebody about a toolchain they already have. So when a
    /// command is not something we can read with confidence the answer is
    /// `None` — *cannot tell* — and no caller is entitled to act on it.
    #[test]
    fn a_command_we_cannot_read_names_no_program_rather_than_a_wrong_one() {
        assert_eq!(program(""), None, "an empty command runs nothing");
        assert_eq!(program("   "), None, "and neither does whitespace");
        // A leading environment assignment is an ordinary shape for a test
        // command, and it names no executable. Reporting `RUST_LOG=debug`
        // missing would be a gap omh invented.
        assert_eq!(program("RUST_LOG=debug cargo test"), Some("cargo"));
        assert_eq!(program("FOO=1 BAR=2 pytest"), Some("pytest"));
        assert_eq!(program("FOO=1"), None, "assignments alone run nothing");
    }

    /// A word carrying shell syntax is not a program name, and guessing one out
    /// of it fabricates a gap. `$(which cargo) test` otherwise reports a missing
    /// program called `$(which` — in a repo whose cargo is fine — and the
    /// recorded answer switches a working hook off for everyone who clones it.
    ///
    /// An allowlist, not a list of metacharacters to reject. A denylist says
    /// nothing about whatever syntax it failed to think of, which is the lesson
    /// `image::probe_args`' guard had to learn twice.
    #[test]
    fn a_word_carrying_shell_syntax_names_no_program() {
        for command in [
            "$(which cargo) test",
            "${CARGO:-cargo} test",
            "`which cargo` test",
            // Not a valid shell variable name, so `is_assignment` correctly
            // declines to skip it — which leaves it as the candidate, and it is
            // not a program either.
            "2FOO=1 cargo test",
            "cargo|tee test",
            "cargo&&true",
        ] {
            assert_eq!(program(command), None, "for {command:?}");
        }
    }

    /// And the ordinary shapes still resolve. An allowlist that refused real
    /// program names would be worse than the guess it replaced — it would
    /// invent a *silence* instead of a gap, and silence is what stops a real
    /// missing toolchain being reported at all.
    #[test]
    fn an_ordinary_program_name_still_resolves() {
        for (command, want) in [
            ("./scripts/test.sh", "./scripts/test.sh"),
            ("python3.11 -m pytest", "python3.11"),
            ("g++ -o x x.cc", "g++"),
            ("/usr/local/bin/cargo test", "/usr/local/bin/cargo"),
            ("cargo-nextest run", "cargo-nextest"),
        ] {
            assert_eq!(program(command), Some(want), "for {command:?}");
        }
    }

    // ── stacks ──────────────────────────────────────────────────────────────

    #[test]
    fn a_manifest_identifies_the_stack() {
        let (_d, r) = repo(&[("Cargo.toml", "[package]")]);
        let found = stacks(&shipped(), &r);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "rust");
    }

    #[test]
    fn a_polyglot_repo_reports_every_stack() {
        let (_d, r) = repo(&[("Cargo.toml", ""), ("package.json", "{}")]);
        let names: std::collections::BTreeSet<_> =
            stacks(&shipped(), &r).into_iter().map(|s| s.name).collect();

        // A set, not a sequence. Order *within* a stack is load-bearing —
        // `corepack enable pnpm` needs node first — but order *across* stacks
        // is whatever `load_dir` sorted by, and nothing may come to depend on
        // it. Asserting the old declaration order would have pinned an
        // accident.
        assert_eq!(
            names,
            ["rust", "node"].map(String::from).into_iter().collect()
        );
    }

    /// The join between omh's hook opinion and the data it ships.
    ///
    /// `conventional` lives in code and the provides live in TOML, so until
    /// item 7 removes the split they are two registries free to disagree — and
    /// a disagreement is silent and expensive: omh seeds a `gofmt -w .` hook
    /// and probes for `gofmt`, while no provide installs it, so the image
    /// builds, the probe fails, and somebody is asked a question no recipe can
    /// answer.
    ///
    /// Iterated over every shipped stack, because the pair that drifts will not
    /// be the one this repo happens to be.
    #[test]
    fn every_conventional_command_is_provisioned() {
        for def in shipped() {
            let Some((test, format)) = conventional(&def.name) else {
                continue;
            };
            let provisioned: Vec<&str> = def
                .provides
                .iter()
                .flat_map(|p| p.needs.iter().map(String::as_str))
                .collect();

            for command in [test, format] {
                let Some(needed) = program(command) else {
                    panic!("{}: `{command}` names no program", def.name);
                };
                assert!(
                    provisioned.contains(&needed),
                    "{}: the conventional hook runs `{command}`, and no provide \
                     installs `{needed}` — the hook would fail on every turn. \
                     provisioned: {provisioned:?}",
                    def.name
                );
            }
        }
    }

    /// Guessing a stack would generate wrong hooks that fail on every agent
    /// turn. Detecting nothing is the correct outcome for an unknown repo.
    #[test]
    fn no_marker_means_no_stack_rather_than_a_guess() {
        let (_d, r) = repo(&[("README.md", "hello")]);
        assert!(stacks(&shipped(), &r).is_empty());
    }

    /// Detection is only worth doing because it yields commands — these become
    /// the base test-on-stop and format-on-edit hooks.
    #[test]
    fn every_known_stack_supplies_commands() {
        for s in all_shipped() {
            assert!(!s.test.is_empty(), "{} has no test command", s.name);
            assert!(!s.format.is_empty(), "{} has no format command", s.name);
        }
    }

    // ── harness preference ──────────────────────────────────────────────────

    fn candidates() -> Vec<String> {
        vec!["claude".into(), "opencode".into()]
    }

    #[test]
    fn host_evidence_picks_the_default_harness() {
        let pick = preferred_harness(&candidates(), &|h| h == "opencode");
        assert_eq!(pick.as_deref(), Some("opencode"));
    }

    /// The harness runs in the sandbox, so an empty host is normal, not an
    /// error — init still has to choose something and say so.
    #[test]
    fn nothing_installed_still_yields_a_default() {
        let pick = preferred_harness(&candidates(), &|_| false);
        assert_eq!(pick.as_deref(), Some("claude"));
    }

    #[test]
    fn no_adapters_means_no_preference() {
        assert_eq!(preferred_harness(&[], &|_| true), None);
    }

    // ── memory seeds ────────────────────────────────────────────────────────

    #[test]
    fn seeds_are_derived_from_what_the_repo_already_says() {
        let (_d, r) = repo(&[
            ("README.md", "# omh\n\noh-my-zsh for agentic coding.\n"),
            ("Cargo.toml", "[package]\nname = \"omh\""),
        ]);
        let s = seeds(&shipped(), &r);
        assert!(
            s.iter().any(|x| x.fact.contains("oh-my-zsh")),
            "README should seed the project description: {s:?}"
        );
        assert!(
            s.iter().any(|x| x.fact.contains("rust")),
            "stack should be seeded: {s:?}"
        );
    }

    /// Every seed must name where it came from, or `omh why` cannot explain it
    /// and the memory becomes unfalsifiable folklore.
    /// A tagline under the title is the commonest README shape there is, and a
    /// derived fact should be the sentence, not the markdown around it.
    #[test]
    fn a_blockquote_tagline_is_read_as_prose() {
        let (_d, r) = repo(&[("README.md", "# omh\n\n> Launch any coding harness.\n")]);
        let s = seeds(&shipped(), &r);
        let fact = &s
            .iter()
            .find(|x| x.source == "README.md")
            .expect("README seed")
            .fact;
        assert_eq!(
            fact, "Launch any coding harness.",
            "markdown syntax is not the fact"
        );
    }

    /// Badges are the other thing that sits under a title, and they say nothing
    /// about the project.
    #[test]
    fn a_badge_line_is_not_mistaken_for_a_description() {
        let (_d, r) = repo(&[(
            "README.md",
            "# p\n\n[![CI](https://img.shields.io/x)](https://ci)\n\nA real description.\n",
        )]);
        let s = seeds(&shipped(), &r);
        let fact = &s
            .iter()
            .find(|x| x.source == "README.md")
            .expect("README seed")
            .fact;
        assert_eq!(fact, "A real description.");
    }

    #[test]
    fn every_seed_cites_its_source() {
        let (_d, r) = repo(&[("README.md", "# p\n\nA thing.\n"), ("go.mod", "module x")]);
        for seed in seeds(&shipped(), &r) {
            assert!(!seed.source.is_empty(), "unsourced seed: {seed:?}");
        }
    }

    #[test]
    fn an_empty_repo_seeds_nothing_rather_than_inventing() {
        let (_d, r) = repo(&[]);
        assert!(seeds(&shipped(), &r).is_empty());
    }

    #[test]
    fn existing_rules_are_seeded_so_conventions_survive() {
        let (_d, r) = repo(&[("AGENTS.md", "# Rules\n\nTDD always.\n")]);
        let s = seeds(&shipped(), &r);
        assert!(
            s.iter().any(|x| x.source.contains("AGENTS.md")),
            "got: {s:?}"
        );
    }
}
