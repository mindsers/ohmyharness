//! Detection — how `omh init` decides without asking.
//!
//! Every question init would ask is hassle we promised to remove, and the
//! answers are mostly lying around already: manifests name the stack, git log
//! names what you work on, the README names the project. Deriving beats
//! interrogating twice over — no wizard, and the facts refresh themselves when
//! the repo changes instead of going stale in a config file.

use std::path::Path;

/// A detected stack and the commands that go with it. The commands are what
/// make detection useful: they become the base hooks and the AGENTS.md body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub name: &'static str,
    pub marker: &'static str,
    pub test: &'static str,
    pub format: &'static str,
}

/// Every stack omh can detect. Public so `init`'s guard can seed a hook for
/// each of them rather than only for whatever stack this repo happens to be —
/// the test iterated `stacks(CARGO_MANIFEST_DIR)`, which is rust and nothing
/// else, so three quarters of what `init` writes went unexercised.
pub const KNOWN: [Stack; 4] = [
    Stack {
        name: "rust",
        marker: "Cargo.toml",
        test: "cargo test",
        format: "cargo fmt",
    },
    Stack {
        name: "node",
        marker: "package.json",
        test: "npm test",
        format: "npm run format",
    },
    Stack {
        name: "python",
        marker: "pyproject.toml",
        test: "pytest",
        format: "ruff format .",
    },
    Stack {
        name: "go",
        marker: "go.mod",
        test: "go test ./...",
        format: "gofmt -w .",
    },
];

/// A derived fact, with the source that produced it. The source is not
/// decoration — `omh why` has to be able to explain every default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub source: String,
    pub fact: String,
}

/// A stack by name, for reading a hook filename back into the marker it
/// implies. The launcher needs this to notice a hook whose stack has gone.
pub fn known(name: &str) -> Option<Stack> {
    KNOWN.into_iter().find(|s| s.name == name)
}

pub fn stacks(repo: &Path) -> Vec<Stack> {
    KNOWN
        .into_iter()
        .filter(|s| repo.join(s.marker).exists())
        .collect()
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
pub fn seeds(repo: &Path) -> Vec<Seed> {
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

    for s in stacks(repo) {
        out.push(Seed {
            source: s.marker.into(),
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

    // ── stacks ──────────────────────────────────────────────────────────────

    #[test]
    fn a_manifest_identifies_the_stack() {
        let (_d, r) = repo(&[("Cargo.toml", "[package]")]);
        let found = stacks(&r);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "rust");
    }

    #[test]
    fn a_polyglot_repo_reports_every_stack() {
        let (_d, r) = repo(&[("Cargo.toml", ""), ("package.json", "{}")]);
        let names: Vec<_> = stacks(&r).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["rust", "node"]);
    }

    /// Guessing a stack would generate wrong hooks that fail on every agent
    /// turn. Detecting nothing is the correct outcome for an unknown repo.
    #[test]
    fn no_marker_means_no_stack_rather_than_a_guess() {
        let (_d, r) = repo(&[("README.md", "hello")]);
        assert!(stacks(&r).is_empty());
    }

    /// Detection is only worth doing because it yields commands — these become
    /// the base test-on-stop and format-on-edit hooks.
    #[test]
    fn every_known_stack_supplies_commands() {
        for s in KNOWN {
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
        let s = seeds(&r);
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
        let s = seeds(&r);
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
        let s = seeds(&r);
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
        for seed in seeds(&r) {
            assert!(!seed.source.is_empty(), "unsourced seed: {seed:?}");
        }
    }

    #[test]
    fn an_empty_repo_seeds_nothing_rather_than_inventing() {
        let (_d, r) = repo(&[]);
        assert!(seeds(&r).is_empty());
    }

    #[test]
    fn existing_rules_are_seeded_so_conventions_survive() {
        let (_d, r) = repo(&[("AGENTS.md", "# Rules\n\nTDD always.\n")]);
        let s = seeds(&r);
        assert!(
            s.iter().any(|x| x.source.contains("AGENTS.md")),
            "got: {s:?}"
        );
    }
}
