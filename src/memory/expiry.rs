//! When a note stops being true.
//!
//! §8: `invalidated_by` takes one of a **closed set** omh can evaluate itself.
//! `omh memory stale` is a join against facts omh already holds, never a
//! judgement — there is no "old enough to be suspect" here, because that is a
//! threshold nobody calibrated pretending to be knowledge.
//!
//! Split impure-gather from pure-evaluate, so the whole decision table is a
//! unit test over hand-built facts and the part that shells out has no
//! opinions.

use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// §8's closed set. Parsed, never a free string: omh must be able to evaluate
/// every one of these itself, and a kind it cannot evaluate is a note
/// advertising an expiry it does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    File { path: String, hash: String },
    Image { digest: String },
    Base { version: String },
    Symbol { name: String },
}

/// The literal a writer pins when they mean "whatever omh would build now".
///
/// The digest is a `git hash-object` of recipe text that exists only inside
/// the binary, so there is no command anybody can run to discover it. Without
/// this, `image:` is a kind with no way to produce a value for it — every pin
/// wrong from birth, which is the note-advertising-an-expiry-it-does-not-have
/// case the module opens by forbidding. Resolved at the write path, so what
/// lands on disk is still a concrete digest.
pub const IMAGE_NOW: &str = "current";

/// The digest `image:current` resolves to, or why it cannot be known here.
///
/// **Two spellings, because "cannot tell" must not resolve to a digest.** The
/// base recipe depends on `ca_cert`, a path on the *host*. `remember_in` runs
/// inside the sandbox, where that path is not mounted — so the guest cannot
/// compute the recipe, and a guest that guesses pins the no-certificate digest
/// while every host-side `stale` computes one with the certificate. Every note
/// the agent recorded was then born stale, on exactly the machines the setting
/// exists for.
///
/// The resolution still happens at the write path, for the reason
/// [`IMAGE_NOW`] gives — storing the sentinel would compare equal to "now"
/// forever and pin nothing. What changed is *who* supplies it: omh resolves it
/// on the host and hands it to the sandbox, and a guest that was handed
/// nothing says so instead of answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipe {
    /// What omh would build, resolved where the recipe is knowable.
    Digest(String),
    /// Why it is not knowable, in the words the writer should hear.
    Unknowable(String),
}

/// Where omh hands the sandbox a digest the guest cannot compute for itself.
///
/// The same road `OMH_SESSION` travels, for the same reason: the sandbox knows
/// facts only the host holds, and an env var set at launch is how it is told.
pub const RECIPE_ENV: &str = "OMH_IMAGE_RECIPE";

impl Recipe {
    /// What the sandbox was handed at launch, or why it has nothing.
    ///
    /// Absent is the honest case, not a broken one: an `omh memory serve` run
    /// by hand outside a sandbox has no host to have told it anything.
    pub fn from_env() -> Self {
        match std::env::var(RECIPE_ENV) {
            Ok(d) if !d.trim().is_empty() => Self::Digest(d),
            _ => Self::Unknowable(format!(
                "this omh is not the one holding the image recipe — the sandbox \
                 is told the digest through ${RECIPE_ENV} at launch, and nothing \
                 set it"
            )),
        }
    }

    /// Resolve from a real `Paths` — the host, where `ca_cert` is readable.
    ///
    /// An unreadable certificate is `Unknowable`, never a digest: it is the
    /// same "the recipe is not what this would compute" case as the sandbox,
    /// arriving by a different road.
    pub fn here(paths: &crate::profile::Paths) -> Self {
        match crate::image::ca_for(paths) {
            Err(e) => Self::Unknowable(format!(
                "the image recipe depends on `ca_cert`, which could not be read: {e:#}"
            )),
            Ok(ca) => {
                match crate::image::recipe_digest(&crate::image::base_dockerfile(
                    ca.as_ref().map(crate::image::Root::pem),
                )) {
                    Ok(d) => Self::Digest(d),
                    Err(e) => Self::Unknowable(format!("could not digest the image recipe: {e:#}")),
                }
            }
        }
    }
}

/// A `git hash-object` output, or a prefix of one long enough to mean it.
///
/// Seven is what `git log` abbreviates to and therefore what a writer pastes.
/// Shorter is a typo rather than an abbreviation, and anything outside lower
/// hex was never a hash — both used to be stored and then compared with `==`
/// against the full forty, so the note was stale the day it was written and
/// could never recover.
fn is_hash(s: &str, min: usize) -> bool {
    (min..=40).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// Wrong at every stage, so it can be refused before anything is known about
/// the repo. Absolute is deliberately absent: `/work/…` is what the agent
/// records and a host path inside the repo normalises away, so those are
/// `gather`'s to confine.
fn escapes(path: &str) -> bool {
    path.contains('\\') || Path::new(path).components().any(|c| c.as_os_str() == "..")
}

impl Trigger {
    pub fn parse(raw: &str) -> Result<Self> {
        let Some((kind, rest)) = raw.split_once(':') else {
            bail!("`{raw}` is not an invalidation trigger (file, image, base, symbol)");
        };
        if rest.is_empty() {
            bail!("`{raw}` names a `{kind}` trigger with nothing in it");
        }
        Ok(match kind {
            "file" => {
                // The LAST `@`: a path may legitimately contain one, and
                // splitting on the first takes the wrong half.
                let Some((path, hash)) = rest.rsplit_once('@') else {
                    bail!("`{raw}` has no hash; an expiry with nothing to compare can never fire");
                };
                if path.is_empty() || hash.is_empty() {
                    bail!("`{raw}` needs both a path and a hash");
                }
                if escapes(path) {
                    bail!("`{path}` leaves the repo; a trigger names a file omh can check");
                }
                if !is_hash(hash, 7) {
                    bail!(
                        "`{hash}` is not a git hash; `stale` compares it against \
                         `git hash-object`, so nothing else can ever match"
                    );
                }
                Self::File {
                    path: path.into(),
                    hash: hash.into(),
                }
            }
            // A recipe digest, not a container digest. `sha256:…` is what a
            // reader assumes and what the spec's own example used, and it can
            // never equal a `git hash-object` of the Dockerfile text.
            "image" if rest == IMAGE_NOW => Self::Image {
                digest: IMAGE_NOW.into(),
            },
            "image" if is_hash(rest, 40) => Self::Image {
                digest: rest.into(),
            },
            "image" => bail!(
                "`{rest}` is not a recipe digest — pin `image:{IMAGE_NOW}` and omh \
                 records what it would build now"
            ),
            // The same parser `evaluate` will compare with, asked at the door:
            // otherwise `base:banana` is stored and answers "omh cannot tell"
            // for ever, indistinguishable from having no base set installed.
            "base" if crate::base::parse_ym(rest).is_none() => {
                bail!("`{rest}` is not a base-set version omh can read")
            }
            "base" => Self::Base {
                version: rest.into(),
            },
            "symbol" => Self::Symbol { name: rest.into() },
            other => {
                bail!("`{other}` is not something omh can evaluate (file, image, base, symbol)")
            }
        })
    }

    pub fn render(&self) -> String {
        match self {
            Self::File { path, hash } => format!("file:{path}@{hash}"),
            Self::Image { digest } => format!("image:{digest}"),
            Self::Base { version } => format!("base:{version}"),
            Self::Symbol { name } => format!("symbol:{name}"),
        }
    }
}

/// What a file looks like right now.
///
/// Three states, not two. Conflating "cannot be read" with "is not there" is
/// the bug `config::read_layer` exists to prevent, and here it would report a
/// permissions problem as the world having changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileFact {
    Hash(String),
    Absent,
    Unreadable,
}

/// Something omh either knows, or can say why it does not.
///
/// The reason is not decoration. Every way of failing to read the base
/// manifest — unreadable directory, broken TOML naming its own file, no
/// version omh can parse — used to arrive as `None` and print "no base set
/// installed to compare against", which tells the reader to install what is
/// already there and throws away the error that said so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact<T> {
    Known(T),
    Unavailable(String),
}

impl<T> Default for Fact<T> {
    fn default() -> Self {
        Self::Unavailable("not gathered".into())
    }
}

/// Facts omh already holds. Gathered once, impurely; nothing here is a
/// judgement.
#[derive(Debug, Default)]
pub struct Facts {
    pub files: BTreeMap<String, FileFact>,
    /// Recipe digests omh would build right now — the base image only.
    ///
    /// A set because the harness layer is a second recipe and belongs here,
    /// but it is keyed by adapter and `gather` has no adapter to ask about.
    /// Until it does, the docs say `image:` tracks the base and nothing
    /// else, rather than this quietly reporting stale for a recipe it never
    /// looked at.
    pub images: Fact<BTreeSet<String>>,
    pub base: Fact<String>,
    /// `None` means no indexed graph was reachable — deliberately not an empty
    /// set, which would say every symbol is gone.
    pub symbols: Option<BTreeSet<String>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    NoTrigger,
    Fresh,
    /// `because` names the recorded value *and* the current one. A join
    /// reports a fact; the reader has to be able to tell whether the world
    /// moved or the note was wrong to begin with.
    Stale {
        because: String,
    },
    /// omh cannot answer. **Never collapsed into `Fresh`** — that is the one
    /// failure that makes `stale` a liar rather than merely incomplete.
    Unknown {
        because: String,
    },
}

/// Repo-relative, always.
///
/// The MCP server runs where the repo is `/work`, so an agent naturally
/// records an absolute sandbox path. Host-side that file does not exist, and
/// every `file:` trigger would report stale on day one — turning the command
/// into noise nobody reads.
pub fn normalise_path(raw: &str, repo: &Path) -> String {
    let sandbox = format!("{}/", crate::container_workdir());
    let trimmed = raw
        .strip_prefix(&sandbox)
        .or_else(|| raw.strip_prefix("./"))
        .unwrap_or(raw);
    Path::new(trimmed)
        .strip_prefix(repo)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

/// Pure. The entire decision table is a unit test over hand-built facts.
pub fn evaluate(trigger: Option<&Trigger>, facts: &Facts) -> Verdict {
    let Some(trigger) = trigger else {
        // §8's honest residue: derived from experience, carrying a date and
        // nothing else. Never "old enough to be suspect" — that is a threshold
        // nobody calibrated dressed as knowledge.
        return Verdict::NoTrigger;
    };
    match trigger {
        Trigger::File { path, hash } => match facts.files.get(path) {
            None | Some(FileFact::Unreadable) => Verdict::Unknown {
                because: format!("`{path}` could not be read"),
            },
            Some(FileFact::Absent) => Verdict::Stale {
                because: format!("`{path}` is gone; it was pinned at {hash}"),
            },
            // A prefix, because `git log` abbreviates and so does everyone
            // reading it. `parse` has already refused anything too short to
            // mean one hash.
            Some(FileFact::Hash(now)) if now.starts_with(hash.as_str()) => Verdict::Fresh,
            Some(FileFact::Hash(now)) => Verdict::Stale {
                because: format!("`{path}` was {hash}, is now {now}"),
            },
        },
        Trigger::Image { digest } => match &facts.images {
            Fact::Unavailable(why) => Verdict::Unknown {
                because: why.clone(),
            },
            Fact::Known(recipes) if recipes.contains(digest) => Verdict::Fresh,
            Fact::Known(_) => Verdict::Stale {
                because: format!("the sandbox image recipe changed; this note pinned {digest}"),
            },
        },
        Trigger::Base { version } => {
            let current = match &facts.base {
                Fact::Unavailable(why) => {
                    return Verdict::Unknown {
                        because: why.clone(),
                    }
                }
                Fact::Known(v) => v,
            };
            // Reuses the parser the manifest loader uses, so a version `stale`
            // cannot read is the same version `load_dir` refuses. Compared as
            // numbers: `2027.2` beating `2027.10` is a bug this repo already
            // shipped once.
            let (Some(now), Some(pinned)) = (
                crate::base::parse_ym(current),
                crate::base::parse_ym(version),
            ) else {
                return Verdict::Unknown {
                    because: format!("`{current}` or `{version}` is not a version omh can read"),
                };
            };
            match now > pinned {
                true => Verdict::Stale {
                    because: format!("the base set is {current}; this note pinned {version}"),
                },
                false => Verdict::Fresh,
            }
        }
        Trigger::Symbol { name } => {
            let Some(symbols) = &facts.symbols else {
                return Verdict::Unknown {
                    because: "no indexed code graph reachable from the host".into(),
                };
            };
            match symbols.contains(name) {
                true => Verdict::Fresh,
                false => Verdict::Stale {
                    because: format!("the code graph no longer contains `{name}`"),
                },
            }
        }
    }
}

/// Facts about the world right now, gathered once.
///
/// Only the files the notes actually name. Walking the repo would make `stale`
/// O(repo) and slow enough that nobody runs it, and a check nobody runs is a
/// check that does not exist.
pub fn gather(paths: &crate::profile::Paths, triggers: &[Trigger]) -> Facts {
    let mut facts = Facts::default();

    for trigger in triggers {
        if let Trigger::File { path, .. } = trigger {
            if facts.files.contains_key(path) {
                continue;
            }
            // Folded here as well as on the write path. A note that arrived any
            // other way — hand-authored, edited, promoted from a teammate, or
            // written before normalisation shipped — still carries the sandbox
            // prefix, and reported a deletion for a file sitting right there.
            let relative = normalise_path(path, &paths.repo);
            // `Path::join` with an absolute argument *replaces*, so a pin
            // normalisation could not fold into the repo would have `stale`
            // stat an arbitrary host path and report on what it found.
            let fact = match escapes(&relative) || Path::new(&relative).is_absolute() {
                true => FileFact::Unreadable,
                false => {
                    let full = paths.repo.join(&relative);
                    match std::fs::metadata(&full) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileFact::Absent,
                        Err(_) => FileFact::Unreadable,
                        Ok(_) => match hash_file(&paths.repo, &relative) {
                            Ok(h) => FileFact::Hash(h),
                            // Present but unhashable is not the same as gone.
                            Err(_) => FileFact::Unreadable,
                        },
                    }
                }
            };
            facts.files.insert(path.clone(), fact);
        }
    }

    // The recipe omh would build right now. No longer pure: it reads the PEM
    // `ca_cert` names, so it can fail — and a failure here must reach
    // `Unavailable` rather than a digest computed as though nobody had set a
    // certificate, which would mark every image-pinned note stale on the one
    // machine that has one.
    facts.images = match Recipe::here(paths) {
        Recipe::Digest(d) => Fact::Known(BTreeSet::from([d])),
        Recipe::Unknowable(why) => Fact::Unavailable(why),
    };

    facts.base = match crate::base::Manifest::load_dir(&paths.base()) {
        Ok(m) => Fact::Known(m.version),
        Err(e) => Fact::Unavailable(format!("{e:#}")),
    };

    // `symbols` stays `None`. The code graph lives in a container volume and is
    // queried per session, through a running sandbox, under a project name that
    // is per-session too. A host-side `stale` has none of those, and asking for
    // one would put a container exec in the middle of a command whose entire
    // spec sentence is "a join against facts omh already holds". `evaluate`
    // already fires correctly the day a set can be supplied.
    facts
}

/// Says which way it failed. `git` missing, `git` not executable, `-C repo`
/// not a repository (exit 128), a path git refuses, and a path that is a
/// directory all used to arrive as one `None` and print "could not be read" —
/// six causes behind one string, and the stderr that distinguished them thrown
/// away unread.
fn hash_file(repo: &Path, path: &str) -> Result<String> {
    use anyhow::Context;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", "--"])
        .arg(path)
        .output()
        .with_context(|| format!("running git hash-object in {}", repo.display()))?;
    if !out.status.success() {
        bail!(
            "git hash-object: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        bail!("git hash-object printed nothing for `{path}`");
    }
    Ok(hash)
}

/// One note's verdict, with everything needed to print it.
#[derive(Debug)]
pub struct Judged {
    pub key: String,
    pub layer: crate::memory::Layer,
    pub recorded: String,
    pub verdict: Verdict,
}

/// `omh memory stale` — a join, in one pass.
pub fn judge(paths: &crate::profile::Paths, notes: &[crate::memory::Note]) -> Result<Vec<Judged>> {
    let triggers: Vec<Trigger> = notes
        .iter()
        .filter_map(|n| n.invalidated_by.as_deref())
        .filter_map(|raw| Trigger::parse(raw).ok())
        .collect();
    let facts = gather(paths, &triggers);

    Ok(notes
        .iter()
        .map(|n| {
            let parsed = n.invalidated_by.as_deref().map(Trigger::parse);
            let verdict = match parsed {
                None => Verdict::NoTrigger,
                // A trigger that will not parse is a note omh cannot judge,
                // said out loud rather than treated as having none.
                Some(Err(e)) => Verdict::Unknown {
                    because: format!("{e}"),
                },
                Some(Ok(t)) => evaluate(Some(&t), &facts),
            };
            Judged {
                key: n.key.clone(),
                layer: n.layer,
                recorded: n.recorded.clone(),
                verdict,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts::default()
    }

    // ── gathering ───────────────────────────────────────────────────────────

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, crate::profile::Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for (name, body) in files {
            let p = paths.repo.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (dir, paths)
    }

    /// A note whose key is a real note, for the bridges that go through `judge`
    /// rather than calling `evaluate` with hand-built facts.
    fn note_pinning(trigger: Option<&str>) -> crate::memory::Note {
        crate::memory::Note {
            key: "k".into(),
            kind: crate::memory::Kind::Surprise,
            source: "session s01, claude".into(),
            recorded: "2026-08-07".into(),
            invalidated_by: trigger.map(|t| t.into()),
            body: String::new(),
            layer: crate::memory::Layer::Local,
            path: std::path::PathBuf::from("k.md"),
        }
    }

    fn git_init(repo: &Path) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "-q", "-b", "main"])
            .output()
            .expect("git must be installed");
        assert!(out.status.success());
    }

    // ── what the door must refuse ───────────────────────────────────────────

    /// **An expiry that can never fire is worse than none**, which is this
    /// module's own doctrine — and every one of these was accepted. `git log`
    /// prints abbreviated hashes, so a short pin is what a writer produces;
    /// `parse_ym` sits unused two arms away; and the spec's own `image:` example
    /// is a *container* digest, which no recipe hash can ever equal.
    #[test]
    fn a_trigger_omh_could_never_evaluate_is_refused_at_the_door() {
        for raw in [
            "file:src/main.rs@zzzz",
            "file:src/main.rs@abc",
            "file:src/main.rs@ABC123DEF456A",
            "base:banana",
            "image:sha256-4f2a",
            "image:",
            "base:",
            "symbol:",
        ] {
            assert!(
                Trigger::parse(raw).is_err(),
                "`{raw}` names an expiry omh cannot evaluate"
            );
        }
    }

    /// The pins a writer actually produces must still be accepted, or the guard
    /// above is just a way to refuse the whole feature.
    #[test]
    fn the_pins_a_writer_produces_are_accepted() {
        for raw in [
            "file:src/main.rs@ce013625030ba8dba906f756967f9e9ca394464a",
            "file:src/main.rs@ce01362",
            "base:2026.08",
            "image:ce013625030ba8dba906f756967f9e9ca394464a",
            "image:current",
            "symbol:GUEST_HOME",
        ] {
            assert!(
                Trigger::parse(raw).is_ok(),
                "`{raw}` is a pin omh can honour"
            );
        }
    }

    /// `validate_key` exists because a path read back off disk becomes a write.
    /// A trigger path is read back off disk and becomes a `metadata` call —
    /// and notes are promoted, so it arrives from another machine.
    ///
    /// Only the forms that are wrong at every stage are refused here. An
    /// absolute path cannot be: `/work/…` is what the agent legitimately
    /// records and a host path inside the repo is normalised away, so `gather`
    /// is where an absolute path that survives normalisation has to stop.
    #[test]
    fn a_trigger_path_that_leaves_the_repo_is_refused() {
        for raw in [
            "file:../../../../etc/passwd@ce01362",
            "file:a/../../b@ce01362",
            "file:a\\b@ce01362",
        ] {
            assert!(Trigger::parse(raw).is_err(), "`{raw}` leaves the store");
        }
    }

    /// The existence oracle. `Path::join` with an absolute argument *replaces*,
    /// so an absolute pin normalisation could not fold into the repo used to
    /// `metadata` an arbitrary host path and report on what it found.
    #[test]
    fn gather_never_stats_a_path_outside_the_repo() {
        let (_d, paths) = repo_with(&[("in.txt", "x")]);
        let outside = Trigger::File {
            path: "/etc/passwd".into(),
            hash: "ce01362".into(),
        };
        let facts = gather(&paths, std::slice::from_ref(&outside));

        assert_eq!(
            facts.files.get("/etc/passwd"),
            Some(&FileFact::Unreadable),
            "a path omh cannot confine to the repo is not a fact about the repo"
        );
        assert!(
            matches!(evaluate(Some(&outside), &facts), Verdict::Unknown { .. }),
            "and it is never reported as gone"
        );
    }

    /// Abbreviated pins have to actually match, or refusing the unmatchable
    /// ones above just moves the permanently-stale note to a different arm.
    #[test]
    fn an_abbreviated_hash_matches_the_hash_it_abbreviates() {
        let mut facts = facts();
        facts.files.insert(
            "t.txt".into(),
            FileFact::Hash("ce013625030ba8dba906f756967f9e9ca394464a".into()),
        );
        let short = Trigger::parse("file:t.txt@ce01362").unwrap();
        assert_eq!(evaluate(Some(&short), &facts), Verdict::Fresh);

        let wrong = Trigger::parse("file:t.txt@ce01363").unwrap();
        assert!(matches!(
            evaluate(Some(&wrong), &facts),
            Verdict::Stale { .. }
        ));
    }

    // ── the bridges: everything `main` actually calls ───────────────────────

    /// **`judge` is the only thing the command calls, and nothing went through
    /// it.** Every other test here hand-builds `Facts` and calls `evaluate`, so
    /// `judge` could return `Fresh` for every note and the suite stayed green.
    #[test]
    fn judge_reports_a_file_that_really_changed_as_stale() {
        let (_d, paths) = repo_with(&[("tracked.txt", "before\n")]);
        git_init(&paths.repo);
        let pinned = hash_file(&paths.repo, "tracked.txt").unwrap();

        let fresh = note_pinning(Some(&format!("file:tracked.txt@{pinned}")));
        assert_eq!(
            judge(&paths, std::slice::from_ref(&fresh)).unwrap()[0].verdict,
            Verdict::Fresh,
            "the hash omh computes must be the hash a note would have pinned"
        );

        std::fs::write(paths.repo.join("tracked.txt"), "after\n").unwrap();
        assert!(
            matches!(
                judge(&paths, std::slice::from_ref(&fresh)).unwrap()[0].verdict,
                Verdict::Stale { .. }
            ),
            "and the change has to reach the verdict"
        );
    }

    /// **The milestone's gate, wired end to end.** `gather` could have been
    /// digesting the wrong string forever: the false direction was covered by a
    /// hand-built `Facts`, and the true direction — that the digest omh
    /// computes is the digest a note would pin — by nothing.
    #[test]
    fn judge_agrees_with_the_recipe_digest_a_note_would_pin() {
        let (_d, paths) = repo_with(&[]);
        let now = crate::image::recipe_digest(&crate::image::base_dockerfile(None)).unwrap();

        let pinned = note_pinning(Some(&format!("image:{now}")));
        assert_eq!(
            judge(&paths, std::slice::from_ref(&pinned)).unwrap()[0].verdict,
            Verdict::Fresh,
            "a note pinning today's recipe is current"
        );

        let stale = note_pinning(Some(&format!("image:{}", "0".repeat(40))));
        assert!(
            matches!(
                judge(&paths, std::slice::from_ref(&stale)).unwrap()[0].verdict,
                Verdict::Stale { .. }
            ),
            "and one pinning a recipe omh would not build now is not"
        );
    }

    /// **A trap, disarmed rather than a feature deferred.**
    ///
    /// `image:` covers the base recipe only, and the obvious extension — digest
    /// `harness_dockerfile` as well — quietly reintroduces the bug
    /// `recipe_digest` was written to prevent. The harness recipe opens
    /// `FROM omh/base:<base_tag>`, and `base_tag` is a `DefaultHasher` of the base
    /// recipe: a value std does not guarantee across releases. Pinning a digest
    /// computed over that text marks every harness-triggered note stale for
    /// everyone the day somebody upgrades Rust — the mass false positive with no
    /// cause anybody could find, which is the sentence `recipe_digest`'s own
    /// docstring uses.
    ///
    /// There are two honest ways forward: render the recipe stably first,
    /// substituting the base's *recipe* digest for its tag, or leave `image:`
    /// base-only as the docs say. This fails on the third — `gather` growing a
    /// harness digest while the recipe still carries an unstable tag — so the
    /// dependency is discovered here rather than in a store full of notes that
    /// went stale on a toolchain bump.
    #[test]
    fn a_harness_recipe_is_never_pinned_while_it_carries_an_unstable_tag() {
        let shipped = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/adapters"));
        let harnesses = crate::adapter::Adapter::load_dir(shipped).unwrap();
        assert!(!harnesses.is_empty(), "no adapters to check: {shipped:?}");

        // Into the profile `gather` would actually read, or this guards a
        // directory the code under test never looks at — the fixture would be
        // empty, `load_dir` would return nothing, and the mistake this exists
        // to catch would sail through.
        let (_d, paths) = repo_with(&[]);
        std::fs::create_dir_all(paths.adapters()).unwrap();
        for entry in std::fs::read_dir(shipped).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "toml") {
                std::fs::copy(entry.path(), paths.adapters().join(entry.file_name())).unwrap();
            }
        }

        let pinned = match gather(&paths, &[]).images {
            Fact::Known(set) => set,
            Fact::Unavailable(why) => panic!("the base recipe must be digestible: {why}"),
        };

        for adapter in &harnesses {
            let recipe = crate::image::harness_dockerfile(adapter, None);
            let carries_unstable_tag = recipe.contains(&crate::image::base_tag(None));
            let digest = crate::image::recipe_digest(&recipe).unwrap();

            // Pinned as well as guarded, so this cannot go quietly vacuous. If
            // it fails, the recipe has become stable and the news is good:
            // `image:` may now cover harnesses, and the guard below has nothing
            // left to protect. Read it and delete it rather than relaxing it.
            assert!(
                carries_unstable_tag,
                "`{}`'s recipe no longer embeds the base tag — a harness digest \
                 is now safe to pin, and this test has become the obstacle",
                adapter.name
            );
            assert!(
                !pinned.contains(&digest),
                "`{}`'s recipe digest is pinned while the recipe still embeds \
                 the base tag, a DefaultHasher value std does not guarantee \
                 across releases. Render it stably first — substitute the base's \
                 recipe digest for its tag — or leave `image:` base-only.",
                adapter.name
            );
        }
    }

    /// `symbols` is `None` by never being assigned, so an `= Some(empty)` slip
    /// would report every symbol gone — under the `stale` heading, which is the
    /// bold claim `commands.md` makes.
    #[test]
    fn gather_leaves_the_symbol_set_unknown_rather_than_empty() {
        let (_d, paths) = repo_with(&[]);
        assert_eq!(gather(&paths, &[]).symbols, None);
    }

    /// The sandbox prefix was folded in on the write path only. Any note that
    /// did not come through `remember` — hand-authored, edited, promoted from a
    /// teammate, or written before this shipped — reported a deletion that
    /// never happened, for a file sitting right there.
    #[test]
    fn gather_folds_a_sandbox_path_the_way_the_write_path_would() {
        let (_d, paths) = repo_with(&[("tracked.txt", "x\n")]);
        git_init(&paths.repo);
        // Pinned exactly as the writer would have: the hash of the file as it
        // is, recorded against the path the agent sees.
        let pinned = hash_file(&paths.repo, "tracked.txt").unwrap();
        let sandbox = Trigger::File {
            path: format!("{}/tracked.txt", crate::container_workdir()),
            hash: pinned,
        };
        let facts = gather(&paths, std::slice::from_ref(&sandbox));

        assert_eq!(
            evaluate(Some(&sandbox), &facts),
            Verdict::Fresh,
            "the file is present and unchanged"
        );
    }

    /// Half the `Absent`/`Unreadable` distinction was unenforced: flipping
    /// `Err(_) => Unreadable` to `Absent` kept the suite green, which reports a
    /// permissions problem as the world having changed.
    #[cfg(unix)]
    #[test]
    fn a_file_omh_may_not_read_is_not_reported_as_gone() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths) = repo_with(&[("locked/secret.txt", "x\n")]);
        git_init(&paths.repo);
        let dir = paths.repo.join("locked");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let t = Trigger::File {
            path: "locked/secret.txt".into(),
            hash: "ce01362".into(),
        };
        let facts = gather(&paths, std::slice::from_ref(&t));
        let verdict = evaluate(Some(&t), &facts);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(verdict, Verdict::Unknown { .. }),
            "a file omh cannot read has not been deleted: {verdict:?}"
        );
    }

    /// `load_dir` builds five distinct errors, including a parse failure naming
    /// the offending file, and every one of them became "no base set
    /// installed" — telling the reader to install what is already there.
    #[test]
    fn an_unreadable_base_manifest_says_so_rather_than_claiming_none_is_installed() {
        let (_d, paths) = repo_with(&[]);
        std::fs::create_dir_all(paths.base()).unwrap();
        std::fs::write(paths.base().join("2026.09.toml"), "this is not toml {{{").unwrap();

        let t = Trigger::Base {
            version: "2026.08".into(),
        };
        let facts = gather(&paths, std::slice::from_ref(&t));
        let Verdict::Unknown { because } = evaluate(Some(&t), &facts) else {
            panic!("a manifest omh cannot read is not an answer");
        };
        assert!(
            because.contains("2026.09"),
            "name what could not be read: {because}"
        );
    }

    /// Walking the repo would make `stale` O(repo) and slow enough that nobody
    /// runs it — and a check nobody runs is a check that does not exist.
    #[test]
    fn gathering_facts_hashes_only_the_files_notes_name() {
        let (_d, paths) = repo_with(&[
            ("named.rs", "x"),
            ("unnamed.rs", "y"),
            ("deep/also-unnamed.rs", "z"),
        ]);
        let triggers = vec![Trigger::parse("file:named.rs@01d0111").unwrap()];
        let facts = gather(&paths, &triggers);

        assert_eq!(facts.files.len(), 1, "only what was asked about");
        assert!(facts.files.contains_key("named.rs"));
    }

    /// A file that is there and one that is not are different facts, and both
    /// are different from one omh could not read.
    #[test]
    fn a_present_file_hashes_and_a_missing_one_is_absent() {
        let (_d, paths) = repo_with(&[("here.rs", "content")]);
        let triggers = vec![
            Trigger::parse("file:here.rs@01d0111").unwrap(),
            Trigger::parse("file:gone.rs@01d0111").unwrap(),
        ];
        let facts = gather(&paths, &triggers);

        assert!(matches!(facts.files["here.rs"], FileFact::Hash(_)));
        assert_eq!(facts.files["gone.rs"], FileFact::Absent);
    }

    /// The hash has to be the same one `remember` would have recorded, or a
    /// note is stale from the moment it is written.
    #[test]
    fn a_files_hash_matches_what_git_would_record_for_it() {
        let (_d, paths) = repo_with(&[("f.txt", "hello\n")]);
        let facts = gather(&paths, &[Trigger::parse("file:f.txt@0000001").unwrap()]);
        assert_eq!(
            facts.files["f.txt"],
            FileFact::Hash("ce013625030ba8dba906f756967f9e9ca394464a".into()),
            "git's own blob hash for `hello\\n`"
        );
    }

    /// A trigger omh cannot parse is a note it cannot judge, and saying so is
    /// not the same as saying the note has no expiry.
    #[test]
    fn a_note_whose_trigger_will_not_parse_is_unknown_not_untriggered() {
        let (_d, paths) = repo_with(&[]);
        let mut note = crate::memory::Note {
            key: "k".into(),
            kind: crate::memory::Kind::Surprise,
            source: "session s01, claude".into(),
            recorded: "2026-08-07".into(),
            invalidated_by: Some("vibes:soon".into()),
            body: String::new(),
            layer: crate::memory::Layer::Local,
            path: std::path::PathBuf::from("k.md"),
        };
        let judged = judge(&paths, std::slice::from_ref(&note)).unwrap();
        assert!(matches!(judged[0].verdict, Verdict::Unknown { .. }));

        note.invalidated_by = None;
        let judged = judge(&paths, std::slice::from_ref(&note)).unwrap();
        assert_eq!(judged[0].verdict, Verdict::NoTrigger);
    }

    // ── the closed set ──────────────────────────────────────────────────────

    /// A parser that accepts a kind the evaluator cannot handle produces a
    /// note advertising an expiry it does not have — worse than no expiry,
    /// because somebody trusts it.
    #[test]
    fn every_invalidation_kind_in_the_spec_round_trips() {
        for raw in [
            "file:src/main.rs@9f2c1a4e",
            "image:4f2a000000000000000000000000000000000000",
            "base:2026.08",
            "symbol:GUEST_HOME",
        ] {
            let parsed = Trigger::parse(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(parsed.render(), raw);
        }
    }

    #[test]
    fn an_unknown_invalidation_kind_is_refused_rather_than_stored() {
        for raw in ["vibes:soon", "2026-08-07", "file", "", "when:tuesday"] {
            assert!(Trigger::parse(raw).is_err(), "{raw:?} is not a trigger");
        }
        let err = Trigger::parse("vibes:soon").unwrap_err().to_string();
        assert!(err.contains("vibes"), "name what was not understood: {err}");
        for kind in ["file", "image", "base", "symbol"] {
            assert!(err.contains(kind), "list what is accepted: {err}");
        }
    }

    /// `split('@').next()` on a path containing one takes the wrong half. The
    /// hash is after the **last** `@`.
    #[test]
    fn a_file_path_containing_an_at_sign_still_parses() {
        let t = Trigger::parse("file:vendor/@scope/pkg/x.ts@abc1230").unwrap();
        assert_eq!(
            t,
            Trigger::File {
                path: "vendor/@scope/pkg/x.ts".into(),
                hash: "abc1230".into()
            }
        );
    }

    /// A `file:` with no hash is an expiry structurally incapable of firing —
    /// which is the bug `why::is_stale` shipped with, in a new costume.
    #[test]
    fn a_file_trigger_without_a_hash_is_refused() {
        assert!(Trigger::parse("file:src/main.rs").is_err());
        assert!(Trigger::parse("file:src/main.rs@").is_err());
        assert!(Trigger::parse("file:@abc").is_err());
    }

    /// The server runs where the repo is `/work`, so an agent naturally writes
    /// an absolute sandbox path. Host-side `stale` would then see a missing
    /// file and report **every** file trigger stale on day one, which turns the
    /// whole command into noise nobody reads.
    #[test]
    fn a_path_recorded_inside_the_sandbox_is_stored_repo_relative() {
        let repo = std::path::Path::new("/Users/x/proj");
        for raw in ["/work/src/main.rs", "src/main.rs", "./src/main.rs"] {
            assert_eq!(normalise_path(raw, repo), "src/main.rs", "from {raw:?}");
        }
        assert_eq!(
            normalise_path("/Users/x/proj/src/main.rs", repo),
            "src/main.rs",
            "a host absolute path inside the repo is relative too"
        );
    }

    // ── evaluation, as a table ──────────────────────────────────────────────

    #[test]
    fn a_note_with_no_trigger_is_never_stale() {
        // Not "stale after N days". §8 forbids a judgement, and a day count is
        // a threshold nobody calibrated wearing the clothes of a fact.
        assert_eq!(evaluate(None, &facts()), Verdict::NoTrigger);
    }

    #[test]
    fn a_note_is_stale_when_the_file_it_pinned_changed() {
        let mut f = facts();
        f.files
            .insert("src/main.rs".into(), FileFact::Hash("d1ff333".into()));
        let t = Trigger::parse("file:src/main.rs@0a1b2c3").unwrap();
        let v = evaluate(Some(&t), &f);
        assert!(matches!(v, Verdict::Stale { .. }), "{v:?}");
        // The reason names both values: a join reports a fact, and the reader
        // has to be able to tell whether the world moved or the note was wrong.
        let Verdict::Stale { because } = v else {
            unreachable!()
        };
        assert!(
            because.contains("0a1b2c3") && because.contains("d1ff333"),
            "{because}"
        );
    }

    /// Without this the test above passes on `=> Stale`, which this repo has
    /// shipped: a staleness check that could have been `=> true`.
    #[test]
    fn an_unchanged_file_is_not_stale() {
        let mut f = facts();
        f.files.insert(
            "src/main.rs".into(),
            FileFact::Hash("5a3e5a3f00000000000000000000000000000000".into()),
        );
        let t = Trigger::parse("file:src/main.rs@5a3e5a3").unwrap();
        assert_eq!(evaluate(Some(&t), &f), Verdict::Fresh);
    }

    /// Deleting the evidence must not make the note permanently true.
    #[test]
    fn a_note_is_stale_when_the_file_it_pinned_was_deleted() {
        let mut f = facts();
        f.files.insert("gone.rs".into(), FileFact::Absent);
        let t = Trigger::parse("file:gone.rs@abc0001").unwrap();
        assert!(matches!(evaluate(Some(&t), &f), Verdict::Stale { .. }));
    }

    /// `config.rs` carries this bug's own docstring: folding "cannot read" into
    /// "is not there" reports a permissions problem as the world having changed.
    #[test]
    fn an_unreadable_file_is_unknown_rather_than_stale() {
        let mut f = facts();
        f.files.insert("locked.rs".into(), FileFact::Unreadable);
        let t = Trigger::parse("file:locked.rs@abc0001").unwrap();
        assert!(
            matches!(evaluate(Some(&t), &f), Verdict::Unknown { .. }),
            "a file omh could not read says nothing about the note"
        );
    }

    #[test]
    fn a_note_is_stale_when_the_image_recipe_changed() {
        let mut f = facts();
        f.images = Fact::Known(BTreeSet::from([
            "abcdef0000000000000000000000000000000000".to_string()
        ]));
        assert!(matches!(
            evaluate(
                Some(&Trigger::parse("image:01d0000000000000000000000000000000000000").unwrap()),
                &f
            ),
            Verdict::Stale { .. }
        ));
        assert_eq!(
            evaluate(
                Some(&Trigger::parse("image:abcdef0000000000000000000000000000000000").unwrap()),
                &f
            ),
            Verdict::Fresh
        );
    }

    /// With no image built, omh knows nothing about images — and knowing
    /// nothing is not the same as the image having changed.
    #[test]
    fn an_image_trigger_is_unknown_when_no_recipe_is_available() {
        assert!(matches!(
            evaluate(
                Some(&Trigger::parse("image:0000000000000000000000000000000000000001").unwrap()),
                &facts()
            ),
            Verdict::Unknown { .. }
        ));
    }

    #[test]
    fn a_note_is_stale_when_the_base_set_was_re_cut() {
        let mut f = facts();
        f.base = Fact::Known("2026.09".into());
        assert!(matches!(
            evaluate(Some(&Trigger::parse("base:2026.08").unwrap()), &f),
            Verdict::Stale { .. }
        ));
        assert_eq!(
            evaluate(Some(&Trigger::parse("base:2026.09").unwrap()), &f),
            Verdict::Fresh
        );
    }

    /// **This exact bug already shipped in this repo**, where `2027.2` beat
    /// `2027.10` because the comparison was a string sort. It is the best
    /// mutation available and it costs nothing to reuse.
    #[test]
    fn base_versions_are_compared_numerically_not_lexicographically() {
        let mut f = facts();
        f.base = Fact::Known("2027.10".into());
        assert!(
            matches!(
                evaluate(Some(&Trigger::parse("base:2027.2").unwrap()), &f),
                Verdict::Stale { .. }
            ),
            "2027.10 is newer than 2027.2, which a string sort denies"
        );
        // And the converse, so the check cannot be `=> Stale`.
        f.base = Fact::Known("2027.2".into());
        assert_eq!(
            evaluate(Some(&Trigger::parse("base:2027.10").unwrap()), &f),
            Verdict::Fresh
        );
    }

    /// A version omh cannot read is a fact it does not have. Treating it as
    /// stale flags every good note the day a format changes.
    #[test]
    fn a_base_version_omh_cannot_parse_is_unknown_rather_than_stale() {
        let mut f = facts();
        f.base = Fact::Known("not-a-version".into());
        assert!(matches!(
            evaluate(Some(&Trigger::parse("base:2026.08").unwrap()), &f),
            Verdict::Unknown { .. }
        ));
        f.base = Fact::Unavailable("no base set installed".into());
        assert!(matches!(
            evaluate(Some(&Trigger::parse("base:2026.08").unwrap()), &f),
            Verdict::Unknown { .. }
        ));
    }

    /// The code graph lives in a container volume and is queried per session.
    /// A host-side `stale` has neither, so it must say so rather than guess —
    /// and `Unknown` must never quietly become `Fresh`, which would promise an
    /// expiry omh has not implemented.
    #[test]
    fn a_symbol_trigger_is_unknown_when_no_graph_is_reachable() {
        let v = evaluate(
            Some(&Trigger::parse("symbol:GUEST_HOME").unwrap()),
            &facts(),
        );
        assert!(matches!(v, Verdict::Unknown { .. }), "{v:?}");
        let Verdict::Unknown { because } = v else {
            unreachable!()
        };
        assert!(because.contains("graph"), "say why: {because}");
    }

    /// Without this, the arm above could be hardwired to `Unknown` for ever and
    /// nothing would notice the day a symbol set can be supplied.
    #[test]
    fn a_symbol_trigger_fires_when_a_graph_says_the_symbol_is_gone() {
        let mut f = facts();
        f.symbols = Some(["STILL_HERE".to_string()].into_iter().collect());
        assert!(matches!(
            evaluate(Some(&Trigger::parse("symbol:GONE").unwrap()), &f),
            Verdict::Stale { .. }
        ));
        assert_eq!(
            evaluate(Some(&Trigger::parse("symbol:STILL_HERE").unwrap()), &f),
            Verdict::Fresh
        );
    }

    /// Every arm, in one place: nothing may return `Fresh` from an absence.
    /// That is the single failure mode that makes `stale` a liar rather than
    /// merely incomplete.
    #[test]
    fn nothing_is_ever_fresh_because_omh_knows_nothing() {
        for raw in [
            "file:x.rs@abc0001",
            "image:1000000000000000000000000000000000000000",
            "base:2026.08",
            "symbol:X",
        ] {
            let t = Trigger::parse(raw).unwrap();
            assert_ne!(
                evaluate(Some(&t), &facts()),
                Verdict::Fresh,
                "{raw} reported fresh against facts omh does not have"
            );
        }
    }
}
