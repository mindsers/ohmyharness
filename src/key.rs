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
    /// One path on the host.
    ///
    /// Distinct from `Text` because `omh why` is where somebody finds out what
    /// a key wants, and "one word or phrase" is the wrong answer about a
    /// filename. `carry_in` already earned `Paths` for the plural case; this
    /// is the singular one.
    Path,
    /// `90s`, `30m`, `2h`, `1d`, or bare seconds.
    Duration,
    /// A size, as a runtime reads one: a number and a unit — `4g`, `512m`.
    Size,
    /// A number above zero, as a runtime reads a CPU count: `2`, `1.5`.
    Count,
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
    /// What applies when it is not set, in a few words — `auto`, `never
    /// stopped`. The screen draws it beside an unset key, where `(unset)`
    /// said nothing about what would happen. Each one is read off the code
    /// that falls back, not the docs.
    pub unset: &'static str,
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
/// Six, and the scan below is what keeps it six: they are string literals at
/// their call sites, so a seventh added to the code and not to this table
/// would otherwise be discovered by whoever committed a token.
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
        unset: "nothing carried",
    },
    // A *name*, selecting which captured login gets mounted. The credential
    // itself lives under `paths.creds()` and never passes through here, which
    // is why `omh set account work` writes it into the committed file, and
    // `omh init` seeds it there — a string or, named per harness, a table.
    Key {
        name: "account",
        does: "Which captured login a session is launched with, by name.",
        shape: Shape::Text,
        secret: Secret::No,
        unset: "the only login captured",
    },
    Key {
        name: "idle_timeout",
        does: "How long a session may sit unused before omh stops it.",
        shape: Shape::Duration,
        secret: Secret::No,
        unset: "never stopped",
    },
    Key {
        name: "sandbox_memory",
        does: "How much memory a session's sandbox may use, as the runtime spells \
               it (`4g`, `512m`). Unset means the runtime's default.",
        shape: Shape::Size,
        secret: Secret::No,
        unset: "the runtime's own",
    },
    Key {
        name: "sandbox_cpus",
        does: "How many CPUs a session's sandbox may use, as the runtime spells it \
               (`2`, `1.5`). Unset means the runtime's default.",
        shape: Shape::Count,
        secret: Secret::No,
        unset: "the runtime's own",
    },
    Key {
        name: "runtime",
        does: "Which runtime builds and runs the sandbox. Unset means `auto`.",
        shape: Shape::Choice(&["auto", "docker", "podman", "sbx"]),
        secret: Secret::No,
        unset: "auto",
    },
    // A path to a PEM on the host, not a credential: a CA certificate is
    // public by construction. It lands in the committed file because a team on
    // managed machines usually has the root at one path, and a teammate
    // inheriting it is the useful outcome.
    //
    // **`omh set --local` is the personal spelling, not `omh settings set`.**
    // This said the opposite, and so did both doc pages. `omh settings set`
    // writes `Layer::Personal`, which is a template for repos `init` has not
    // seen yet — it is not one of `Layer::SETTINGS`, and `init` seeds from it
    // through `write_if_absent`, which never revisits. So for anybody whose
    // repo already exists — which is everybody hitting this, because the
    // failure happens *during* `init` — the advice was a write nothing reads,
    // reported as a success.
    Key {
        name: "ca_cert",
        does: "A CA certificate the sandbox must trust, as a path to a PEM — \
               for a network that inspects TLS and signs it with its own root.",
        shape: Shape::Path,
        secret: Secret::No,
        unset: "no extra CA",
    },
    Key {
        name: "persistence",
        does: "How a session's terminal survives detaching. Unset means `dtach`.",
        shape: Shape::Choice(&["dtach", "none"]),
        secret: Secret::No,
        unset: "dtach",
    },
];

/// What omh knows about a key, if anything.
pub fn describes(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

/// The key a name nothing reads was probably meant to be: one edit or two
/// away, the way `carry_ins` is from `carry_in`.
pub fn nearest(name: &str) -> Option<&'static str> {
    KEYS.iter()
        .map(|k| (distance(name, k.name), k.name))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map(|(_, k)| k)
}

/// How many single-character edits turn `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
}

/// What is wrong with this value for this key, if omh can tell.
///
/// `Choice`, `Path` and `Duration` are what is checkable here; `Text` and
/// `Paths` are freeform. `Path` because docker reads a relative `-v` source as
/// a named volume, so the mistake is silent and its symptom is far away.
/// `Duration` because the alternative was hearing about it at the next launch,
/// on stderr, while a session was starting.
///
/// A write refuses on one of these unless `--anyway` — the rule
/// `cmd::settings::set` states where it asks. A launch only warns:
/// `idle::parse_duration` returns `None` rather than erroring, so a value in
/// one layer that only a newer omh reads cannot stop you working.
///
/// `None` back from here means *nothing to say*, never *this is fine*.
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
        Shape::Path => {
            // Docker reads a non-absolute `-v` source as a *named volume*, so
            // a relative `ca_cert` builds a correct image and then mounts an
            // empty directory into the cross-build. And this key is committed,
            // where "the directory omh was run from" is not a shared fact.
            let bare = value.trim().trim_matches('"');
            if bare.is_empty() {
                Some(format!("`{}` was given nothing to name", key.name))
            } else if !std::path::Path::new(bare).is_absolute() {
                Some(format!(
                    "`{}` takes an absolute path — `{bare}` resolves against \
                     whatever directory omh was run from, and this setting is \
                     shared with the repo",
                    key.name
                ))
            } else if !std::path::Path::new(bare).is_file() {
                // The build refuses it there, after the write said nothing.
                Some(format!(
                    "`{}` names nothing at `{bare}` on this machine",
                    key.name
                ))
            } else if key.name == "ca_cert" {
                // What the build would refuse, asked of the one place every
                // refusal of this setting lives: `/etc/hosts` passed as a PEM.
                crate::image::Root::refusal(std::path::Path::new(bare))
            } else {
                None
            }
        }
        // Loosely, as the runtime reads it — a number, then an optional unit
        // and `i`/`b` — so a value only one runtime spells is let through for
        // that runtime to judge, and `banana` is not.
        Shape::Size => {
            let bare = value.trim().trim_matches('"');
            let digits = bare.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let unit = bare[digits.len()..].to_ascii_lowercase();
            let number = !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
                && digits.parse::<f64>().is_ok();
            // A unit, as the message says: bare, a number is bytes to Docker,
            // below the least a container runs with — `4096` was let through.
            let unit_ok = matches!(
                unit.as_str(),
                "k" | "m" | "g" | "t" | "kb" | "mb" | "gb" | "tb" | "kib" | "mib" | "gib" | "tib"
            );
            (!(number && unit_ok)).then(|| {
                format!(
                    "`{}` was given `{bare}`, which is not a size — it takes a number \
                     and a unit, as `4g` or `512m`",
                    key.name
                )
            })
        }
        Shape::Count => {
            let bare = value.trim().trim_matches('"');
            let above_zero = bare.chars().all(|c| c.is_ascii_digit() || c == '.')
                && bare.parse::<f64>().is_ok_and(|n| n > 0.0);
            (!above_zero).then(|| {
                format!(
                    "`{}` was given `{bare}`, which is not a number above zero — it \
                     takes one, as `2` or `1.5`",
                    key.name
                )
            })
        }
        Shape::Duration => {
            // Asked of the reader, never of a second pattern written here: a
            // quarrel that disagreed with `parse_duration` would either pass a
            // value the launch goes on to ignore, or complain about one it
            // reads perfectly well. There is one definition of "a duration omh
            // can read" and this is a call to it.
            //
            // That also covers the value that is not a typo at all — digits
            // that fit `u64` while their seconds do not. `94368760191893771d`
            // is a plain enough "never reap this", and before `parse_duration`
            // checked its multiply it meant 128 seconds.
            match crate::idle::parse_duration(value) {
                Some(_) => None,
                None => Some(format!(
                    "`{}` was given `{}`, which is not a duration omh can read \
                     — it takes `90s`, `30m`, `2h`, `1d`, or bare seconds",
                    key.name,
                    value.trim().trim_matches('"')
                )),
            }
        }
        // A list, as the file holds one: `.env` alone was written as a string,
        // which the launch's reader refuses as not a list of strings.
        Shape::Paths => {
            let listed: Option<Vec<String>> =
                toml::from_str::<toml::Table>(&format!("v = {value}"))
                    .ok()
                    .and_then(|t| t.get("v").cloned())
                    .and_then(|v| {
                        v.as_array()?
                            .iter()
                            .map(|p| p.as_str().map(str::to_string))
                            .collect()
                    });
            let Some(paths) = listed else {
                return Some(format!(
                    "`{}` takes a list of paths, as `[\".env\", \".env.local\"]` — `{}` is not one",
                    key.name,
                    value.trim()
                ));
            };
            // As every launch checks them: `["../.ssh"]` was written, seeded
            // into a committed file, and refused at each launch after.
            paths
                .iter()
                .find_map(|p| crate::carry::validate_pattern(p).err())
                .map(|e| format!("{e:#}"))
        }
        Shape::Text => None,
    }
}

#[cfg(test)]
mod tests {
    /// **A relative `ca_cert` builds a correct image and mounts an empty
    /// directory.** `Shape::Path` validated nothing, so
    /// `omh set --local ca_cert corp.pem` was accepted in silence. `ca_for`
    /// then reads it relative to the process CWD and gets the right bytes, so
    /// the recipe and the tag are both correct — but `memory::deliver` passes
    /// the same raw string to `docker run -v {path}:/omh-ca.crt:ro`, and
    /// docker reads a non-absolute source as a **named volume**. The guest
    /// gets an empty directory where the certificate should be, and
    /// `SSL_CERT_FILE` points at it.
    ///
    /// It is also the one setting where relative is wrong on its face:
    /// `ca_cert` is `Secret::No`, so it lands in the *committed* layer, where
    /// the directory omh happened to be run from is not a shared fact.
    ///
    /// Refused at the write unless `--anyway`, which is what `quarrel` is
    /// for. The refusal that stops a build still lives in `Root::read`.
    #[test]
    fn a_path_key_says_so_when_the_path_is_not_absolute() {
        let ca = KEYS
            .iter()
            .find(|k| k.name == "ca_cert")
            .expect("ca_cert is a key");
        assert!(
            matches!(ca.shape, Shape::Path),
            "this test is about the `Path` shape"
        );

        for relative in ["corp.pem", "./corp.pem", "certs/corp.pem", ""] {
            let said = quarrel(ca, relative).unwrap_or_else(|| {
                panic!("`{relative}` is not absolute and must be quarrelled with")
            });
            assert!(
                said.contains("ca_cert"),
                "the quarrel must name the key: {said}"
            );
        }

        // Absolute and there is the shape that works, and a quoted one is the
        // same string to a person — `Choice` already learned that lesson.
        let dir = tempfile::tempdir().unwrap();
        let pem = dir.path().join("corp.pem");
        std::fs::write(
            &pem,
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let at = pem.display().to_string();
        for absolute in [at.clone(), format!("\"{at}\"")] {
            assert_eq!(
                quarrel(ca, &absolute),
                None,
                "`{absolute}` is absolute and must pass"
            );
        }
        // Absolute and not there: the build refuses it, so the write says so
        // first. `/nope/ca.pem` was written with exit 0.
        let missing = dir.path().join("nope.pem").display().to_string();
        let said = quarrel(ca, &missing).expect("a missing certificate is said");
        assert!(said.contains("nothing at"), "{said}");
        // There, and not a certificate: `/etc/hosts` was taken as a PEM.
        let hosts = dir.path().join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
        let said = quarrel(ca, &hosts.display().to_string()).expect("not a certificate");
        assert!(said.contains("certificate"), "{said}");
    }

    /// A list of paths is checked as every launch checks it: `["../.ssh"]`
    /// was written with exit 0 — on the screen, with a ✓ — seeded into a new
    /// repo's committed file, and refused by every launch after. It is the
    /// one key that carries secrets.
    #[test]
    fn a_list_of_paths_is_checked_as_a_launch_checks_it() {
        let carry = KEYS.iter().find(|k| k.name == "carry_in").unwrap();
        assert_eq!(quarrel(carry, "[\".env\", \"certs/\"]"), None);
        for bad in [
            "[\"../.ssh\"]",
            "[\".env\", \"/etc/passwd\"]",
            "[\"~/.aws\"]",
            "[\"\"]",
        ] {
            let said = quarrel(carry, bad).unwrap_or_else(|| panic!("`{bad}` passed"));
            assert!(said.contains("carry_in"), "{said}");
        }
    }

    /// A size and a CPU count are checked as loosely as the runtime reads
    /// them, and no looser: `sandbox_memory banana` and `sandbox_cpus lots`
    /// were written with exit 0, and found at the next launch. A value no
    /// runtime reads as one is refused; anything a runtime might is let
    /// through for it to judge.
    #[test]
    fn a_size_and_a_cpu_count_are_checked_as_a_runtime_reads_them() {
        let key = |name: &str| KEYS.iter().find(|k| k.name == name).unwrap();
        let (memory, cpus) = (key("sandbox_memory"), key("sandbox_cpus"));
        for ok in ["4g", "512m", "1.5g", "2GiB", "512mb", "4G", "\"4g\""] {
            assert_eq!(quarrel(memory, ok), None, "`{ok}` is a size");
        }
        // A bare number is bytes to Docker, below the least it runs with:
        // the message said "a number and a unit" and let `4096` through.
        for bad in ["banana", "g4", "", "4x", "-1g", "4 gigs", "4096", "4096b"] {
            let said = quarrel(memory, bad).unwrap_or_else(|| panic!("`{bad}` is not a size"));
            assert!(said.contains("sandbox_memory"), "{said}");
        }
        for ok in ["2", "1.5", "0.5", "\"2\""] {
            assert_eq!(quarrel(cpus, ok), None, "`{ok}` is a count");
        }
        for bad in ["lots", "0", "-1", "", "nan", "inf", "2 cores"] {
            let said = quarrel(cpus, bad).unwrap_or_else(|| panic!("`{bad}` is not a count"));
            assert!(said.contains("sandbox_cpus"), "{said}");
        }
    }

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

    /// Every key says what applies when it is not set, and the answer agrees
    /// with the rest of what the table says: a choice falls back to one of its
    /// choices, and a `does` that already says "Unset means `x`" says `x` here.
    ///
    /// The screen draws this beside every unset key, where `(unset)` told
    /// nobody what would happen.
    #[test]
    fn every_key_says_what_applies_when_it_is_unset() {
        for k in KEYS {
            assert!(!k.unset.trim().is_empty(), "{} says nothing", k.name);
            if let Shape::Choice(allowed) = k.shape {
                assert!(
                    allowed.contains(&k.unset),
                    "{} falls back to `{}`, which is not one of {allowed:?}",
                    k.name,
                    k.unset
                );
            }
            if let Some((_, rest)) = k.does.split_once("Unset means `") {
                let said = rest.split('`').next().unwrap_or_default();
                assert_eq!(k.unset, said, "{}: `does` and `unset` disagree", k.name);
            }
        }
    }

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
        let mut files = 0;
        // Production text only, by the one rule `testsrc` keeps: a test
        // that reads a made-up key is not a setting omh reads.
        {
            for (_path, body) in crate::testsrc::production() {
                files += 1;
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
        // Tied to the table rather than typed. A floor of `5` sat here while
        // the table held six, so a key whose call site went dark was one the
        // floor could absorb — and a number nobody updates is a number that
        // stops meaning anything.
        //
        // `- 2` because the scan reads *literals* at the two call shapes above,
        // so a key resolved another way is legitimately invisible to it:
        // `ca_cert`, matched on a `Setting`'s own field, and `account`, which
        // `config::account_for` reads as a string or a per-harness table.
        //
        // Written without naming either call shape: this comment sits inside
        // the file the scan reads, and spelling one out here made the scan
        // match its own prose and report a key called `name`.
        assert!(
            read.len() >= KEYS.len() - 2,
            "the scan found only {read:?} of {} keys — it is no longer finding \
             the call sites it was written to read",
            KEYS.len()
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
    /// It protects one key. A seventh key misclassified on the day it is added
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
    /// It is the executable half of the rule the configuration reference states as
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
