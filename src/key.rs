//! The settings omh reads, and what omh knows about each one.
//!
//! Until this existed, a setting key was a string literal at the call site and
//! nothing else. The repo-scoped write accepted any key at all and put it in
//! the gitignored layer — safe, but only because *every* value went there,
//! which also meant a teammate cloning the repo got none of them.
//!
//! Moving that default is what needs this table. Once most keys land in the
//! committed file, "which keys must not" stops being a property of the command
//! and becomes a property of the key, and something has to hold that knowledge
//! where a test can read it.

use crate::config::Layer;

/// What a value for this key looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One word or phrase.
    Text,
    /// A TOML array of paths.
    Paths,
    /// `90s`, `30m`, `2h`, `1d`, or bare seconds.
    Duration,
    /// One of a fixed set.
    Choice(&'static [&'static str]),
}

/// Whether a value for this key can name or hold a credential.
///
/// **A judgement about the world, not something a test can derive.** No guard
/// here can tell you whether `carry_in` really reaches a secret — that is read
/// off what the value is *for*. What the guards do check is that this judgement
/// is not quietly disconnected from the file omh writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secret {
    /// Nothing a value here can hold is worth hiding from git.
    No,
    /// A value here names or contains something that must not be committed.
    Yes,
}

/// One setting, described.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub name: &'static str,
    /// What omh reads this for, in one sentence.
    ///
    /// Here rather than in the docs because `omh why <key>` promises it, in
    /// `--help` text this table's own command prints. That promise was made
    /// before anything could keep it: `why` resolved against MCP servers and
    /// hooks, so every settings key came back *nothing recorded under that
    /// name* — the failure mode `why.rs` names as its own.
    pub does: &'static str,
    pub shape: Shape,
    pub secret: Secret,
}

impl Key {
    /// Where `omh set <name> <value>` writes when nothing else decides.
    ///
    /// **Derived from `secret` rather than declared per key, deliberately.** A
    /// declared layer is a second place to be wrong, and the wrong combination
    /// — *this holds a secret, and it goes in the committed file* — is the one
    /// mistake this table exists to make unspellable. Deriving it leaves
    /// exactly one judgement to get right per key.
    pub fn default_layer(&self) -> Layer {
        match self.secret {
            Secret::Yes => Layer::Local,
            Secret::No => Layer::Shared,
        }
    }
}

/// Every key omh reads.
///
/// Five, and the scan below is what keeps it five: they are string literals at
/// their call sites, so a sixth added to the code and not to this table would
/// otherwise be discovered by whoever committed a token.
pub const KEYS: &[Key] = &[
    // The only path by which a secret reaches the agent — `src/carry.rs` says
    // so, and so does the comment `init` writes into every new settings file.
    // The value is a list of paths rather than a credential, but `.env.local`
    // in a committed file is a map to one.
    Key {
        name: "carry_in",
        does: "Untracked files a session needs — a worktree holds tracked files \
               only. The one path by which a secret reaches the agent, so keep \
               it short.",
        shape: Shape::Paths,
        secret: Secret::Yes,
    },
    // A *name*, selecting which captured login gets mounted. The credential
    // itself lives under `paths.creds()` and never passes through here, which
    // is why this is the one value-taking key the docs show being shared on
    // purpose: `omh set --shared account work`.
    Key {
        name: "account",
        does: "Which captured login a session is launched with, by name.",
        shape: Shape::Text,
        secret: Secret::No,
    },
    Key {
        name: "idle_timeout",
        does: "How long a session may sit unused before omh stops it.",
        shape: Shape::Duration,
        secret: Secret::No,
    },
    Key {
        name: "runtime",
        does: "Which runtime builds and runs the sandbox. Unset means `auto`.",
        shape: Shape::Choice(&["auto", "docker", "sbx"]),
        secret: Secret::No,
    },
    Key {
        name: "persistence",
        does: "How a session's terminal survives detaching. Unset means `dtach`.",
        shape: Shape::Choice(&["dtach", "none"]),
        secret: Secret::No,
    },
];

/// What omh knows about a key, if anything.
pub fn describes(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

/// What is wrong with this value for this key, if omh can tell.
///
/// Only `Choice` is checkable here, and that is the whole of the claim:
/// `Text` and `Paths` are freeform, and a `Duration` omh cannot parse is
/// already reported where it is read, deliberately — `idle::parse_duration`
/// returns `None` rather than erroring so a typo in one layer cannot stop you
/// working. `None` back from here means *nothing to say*, never *this is fine*.
pub fn quarrel(key: &Key, value: &str) -> Option<String> {
    match key.shape {
        Shape::Choice(allowed) => {
            // The value arrives as it was typed; a quoted one is the same
            // string to a person and a different one to `contains`.
            let bare = value.trim().trim_matches('"');
            (!allowed.contains(&bare)).then(|| {
                format!(
                    "`{}` does not take `{bare}` — only {}",
                    key.name,
                    allowed.join(", ")
                )
            })
        }
        Shape::Text | Shape::Paths | Shape::Duration => None,
    }
}

#[cfg(test)]
mod tests {
    /// A key's description is written for the person who typed `omh settings`.
    ///
    /// One of them read *"Files a session gets that git does not carry — see
    /// `src/carry.rs`."* and shipped that to the terminal. A source path is a
    /// note between maintainers; it tells somebody configuring a tool to go and
    /// read a file they do not have, in a language they may not write.
    ///
    /// The rule is about the audience, not that one path, so it is stated as
    /// the audience: nothing in a description names a file in this repository.
    #[test]
    fn no_key_description_sends_the_reader_to_the_source() {
        for key in super::KEYS {
            for pointer in ["src/", ".rs`", "crate::"] {
                assert!(
                    !key.does.contains(pointer),
                    "`{}` tells whoever ran `omh settings` to read `{pointer}`: {}",
                    key.name,
                    key.does
                );
            }
        }
    }

    use super::*;

    /// Every key says what omh reads it for.
    ///
    /// `--help` and the docs both tell a person to run `omh why <key>` to find
    /// out, and an empty sentence there is that promise broken quietly. A
    /// length floor rather than a non-empty check: `does: ""` and `does: "."`
    /// are the two ways a hurried addition satisfies "not empty".
    #[test]
    fn every_key_says_what_omh_reads_it_for() {
        assert!(!KEYS.is_empty(), "an empty table satisfies every rule");
        for k in KEYS {
            assert!(
                k.does.len() > 20,
                "`{}` does not say what omh reads it for: {:?}",
                k.name,
                k.does
            );
        }
    }

    /// Every key omh reads is a key omh can describe.
    ///
    /// The keys are string literals at their call sites, so the table and the
    /// code that reads settings can drift apart silently — and the drift is
    /// one-directional and dangerous: a key added to the code and not to the
    /// table gets whatever default `omh set` hands an unknown key, which is
    /// the committed file.
    ///
    /// So the scan reads the call sites rather than trusting a list. It walks
    /// `src/` **recursively** — `src/memory/` is five thousand lines that the
    /// older scans in this repo never opened, because they used `read_dir` and
    /// stopped at the top.
    #[test]
    fn every_setting_omh_reads_is_a_key_omh_can_classify() {
        let mut read = std::collections::BTreeSet::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut files = 0;
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                files += 1;
                let body = std::fs::read_to_string(&path).unwrap();
                for call in ["policy_value(", "policy_list("] {
                    for (at, _) in body.match_indices(call) {
                        // `policy_value(&paths, "account")` — the literal is
                        // the second argument, so take what is between the
                        // first pair of quotes after the call.
                        let rest = &body[at..];
                        let Some(open) = rest.find('"') else { continue };
                        let Some(shut) = rest[open + 1..].find('"') else {
                            continue;
                        };
                        let name = &rest[open + 1..open + 1 + shut];
                        // A definition, not a call: `fn policy_value(` has no
                        // literal before the next statement.
                        if !name.is_empty() && !name.contains(char::is_whitespace) {
                            read.insert(name.to_string());
                        }
                    }
                }
            }
        }

        assert!(
            files > 20,
            "the scan read {files} sources — it stopped early, and a scan that \
             stopped early agrees with anything"
        );
        assert!(
            read.len() >= 5,
            "the scan found only {read:?} — it is no longer finding the call \
             sites it was written to read"
        );

        let unclassified: Vec<&String> = read.iter().filter(|k| describes(k).is_none()).collect();
        assert!(
            unclassified.is_empty(),
            "omh reads these settings and can say nothing about them, so \
             nothing can decide where they are safe to write: {unclassified:?}"
        );
    }

    /// `carry_in` is classified as what the rest of the codebase says it is.
    ///
    /// Written because the mutation that matters most survived everything
    /// else: flipping `carry_in` to `Secret::No` sends the one documented
    /// route to a credential into the committed file, and the table-wide
    /// guards all stayed green — they check that a *stated* judgement reaches
    /// the right file, and a wrong judgement is still self-consistent.
    ///
    /// So this pins the judgement itself against what the code already
    /// asserts in prose: `src/carry.rs` calls it the only path by which a
    /// secret reaches the agent, `init` writes that same sentence into every
    /// new settings file, and the repo-scoped write defaulted away from the
    /// committed layer *because* of this key.
    ///
    /// It protects one key. A sixth key misclassified on the day it is added
    /// is not caught by anything here, and cannot be — that is a judgement
    /// about what a value is for. What can be checked is that the judgement
    /// omh already made, in writing, is the one the table holds.
    #[test]
    fn the_documented_secret_path_is_classified_as_one() {
        let carry = describes("carry_in").expect("`carry_in` is a key omh reads");
        assert_eq!(
            carry.secret,
            Secret::Yes,
            "`src/carry.rs` calls this the only path by which a secret reaches \
             the agent; the table disagrees"
        );
        assert!(
            !carry.default_layer().is_committed(),
            "and so it cannot default to a file git tracks"
        );
    }

    /// The choices omh offers are the choices omh parses.
    ///
    /// Two copies of one list, and the table's copy is the one nobody runs.
    /// `runtime` carries `auto` on top of the backends because that is the
    /// default when the key is unset and `select` handles it by name.
    #[test]
    fn every_choice_is_one_the_code_behind_it_accepts() {
        let choices = |name: &str| match describes(name).map(|k| k.shape) {
            Some(Shape::Choice(c)) => c,
            other => panic!("`{name}` is {other:?}, not a choice"),
        };
        for backend in crate::runtime::NAMES {
            assert!(
                choices("runtime").contains(&backend),
                "`runtime` does not offer `{backend}`, which `select` accepts"
            );
        }
        assert!(
            choices("runtime").contains(&"auto"),
            "`auto` is what an unset `runtime` means, so it is sayable"
        );
        assert_eq!(
            choices("persistence"),
            crate::persist::Mode::NAMES,
            "the modes offered and the modes parsed have diverged"
        );
    }

    /// A key that can carry a secret never defaults to the committed file.
    ///
    /// This cannot check the judgement — whether `carry_in` really reaches a
    /// credential is read off what the value is *for*, and no test knows that.
    /// What it pins is the step from the judgement to the file: that
    /// `default_layer` has not been inverted, simplified away, or given a
    /// per-key override that disagrees with it.
    ///
    /// It is the executable half of the rule `docs/configuration.md` states as
    /// prose — *the protection moved from the command to the key*. The prose it
    /// used to quote said *the committed file is never reached by accident,
    /// only on purpose*, which was a property of a command that sent every
    /// value to the gitignored file. `omh set` defaults to the committed one,
    /// so the table below is what carries the guarantee now.
    ///
    /// Two guards in `main.rs` cover the steps this one cannot see, and the
    /// three are a chain rather than three spellings of one claim:
    /// `no_unqualified_write_can_reach_version_control` reads `key_layer`, the
    /// table lookup plus the fallback for a key the table never saw; and
    /// `the_gitignored_file_is_the_only_direction_rule_two_moves_a_write`
    /// reads `set_layer`, where the one layer override in the design lives.
    /// This one pins the step from the judgement to `default_layer`.
    #[test]
    fn no_key_that_carries_a_secret_defaults_to_the_committed_layer() {
        assert!(!KEYS.is_empty(), "an empty table satisfies every rule");
        for k in KEYS {
            if k.secret == Secret::Yes {
                assert!(
                    !k.default_layer().is_committed(),
                    "`{}` can carry a secret and defaults to a file git tracks",
                    k.name
                );
            }
        }
        // …and the safe ones are shared, or the table would be protecting
        // nothing by making everything local.
        assert!(
            KEYS.iter().any(|k| k.default_layer().is_committed()),
            "every key defaults to the gitignored layer, which is the old \
             behaviour wearing a table"
        );
    }
}
