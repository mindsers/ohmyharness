//! What a built image turned out to contain, remembered between runs.
//!
//! One question, asked once per image: *does this program resolve in here?*
//! The answer is a fact about the image and nothing else — not about the repo,
//! not about the host — so it is cached under the image's **tag**, and a repo
//! that switches harness or gains a provide simply asks about a tag it has not
//! seen.
//!
//! ## Why the tag, and not a digest of the recipe
//!
//! `docs/design/adoption.md` gestured at `image::recipe_digest`, which shells
//! out to `git hash-object`. git does not work inside an omh sandbox by design
//! — the worktree's `.git` is a pointer at a host directory omh does not mount
//! — so keying this cache on it makes the cache unbuildable in the environment
//! omh ships. The tag is already content-addressed on the recipe (`tag_for`
//! hashes the adapter's install; `stack_tag` hashes the whole layered
//! Dockerfile), which is the property the digest was wanted for, and it costs
//! no subprocess at all.
//!
//! ## The asymmetry this file exists to keep
//!
//! An **unknown** program is [`None`], never `Some(false)`. Suppression acts on
//! `Some(false)` alone, so a cache that is missing, empty, corrupt or truncated
//! suppresses nothing and every hook ships — the same direction
//! `detect::program` and `triage_for` already fall in. The other way round, one
//! unreadable file would silently switch off every hook in every repo, and the
//! user would see a working session with no automation in it and no reason
//! given.

use crate::profile::Paths;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};

/// `{tag: {program: resolves}}`, as read from and written to `~/.omh/facts.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Facts(BTreeMap<String, BTreeMap<String, bool>>);

impl Paths {
    /// Measurements about images, beside the catalogue rather than in the repo.
    ///
    /// An image is shared by every repo that renders the same recipe, so a
    /// per-repo cache would ask the same question once per checkout and answer
    /// it four times.
    pub fn facts(&self) -> std::path::PathBuf {
        self.root.join("facts.json")
    }
}

impl Facts {
    /// Read what is remembered. **Never fatal, and never `Some(false)`.**
    ///
    /// A missing file is a first run. An unreadable or malformed one is a
    /// defect somewhere, and the honest answer to *does `cargo` resolve* is
    /// still "nothing here knows" — so both come back empty and every hook
    /// ships. Reported to stderr rather than swallowed: silence here is a
    /// sandbox that quietly stops caching and re-probes on every launch, which
    /// looks exactly like working.
    pub fn load(paths: &Paths) -> Self {
        let path = paths.facts();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("omh: could not read {} — {e}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_str(&raw) {
            Ok(facts) => facts,
            Err(e) => {
                eprintln!(
                    "omh: {} is not readable as measurements ({e}), so nothing is \
                     assumed about this image",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Everything known about one image, in the shape suppression reads.
    ///
    /// A program **absent from the returned map** is one nobody probed, and
    /// that is the whole asymmetry: `render::suppressed_by_probe` acts on
    /// `Some(false)`, so an unknown program suppresses nothing. A tag nobody
    /// has probed returns an empty map, which says exactly that about every
    /// program at once.
    pub fn about(&self, tag: &str) -> BTreeMap<String, bool> {
        self.0.get(tag).cloned().unwrap_or_default()
    }

    /// Which of these have never been asked about in this image.
    ///
    /// The whole point of the cache: a launch probes what it has not seen, not
    /// what it has. A repo whose hooks and stacks have not changed since `init`
    /// asks the container nothing.
    pub fn unseen(&self, tag: &str, wanted: &BTreeSet<String>) -> Vec<String> {
        let known = self.0.get(tag);
        wanted
            .iter()
            .filter(|p| !known.is_some_and(|k| k.contains_key(*p)))
            .cloned()
            .collect()
    }

    /// Record what a probe reported about one image.
    ///
    /// Outcomes rather than a map, so this reads the one wire format
    /// `doctor::parse` produces and there is no second shape to keep in step.
    pub fn learn(&mut self, tag: &str, outcomes: &[crate::doctor::Outcome]) {
        let entry = self.0.entry(tag.to_string()).or_default();
        for o in outcomes {
            entry.insert(o.name.clone(), o.ok);
        }
    }

    /// Write it back, creating `~/.omh` if this is a first run.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.facts();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        (dir, paths)
    }

    /// What suppression will ask: `Some(false)` blocks a hook, and everything
    /// else — including a program nobody probed — leaves it alone.
    fn resolves(facts: &Facts, tag: &str, program: &str) -> Option<bool> {
        facts.about(tag).get(program).copied()
    }

    fn outcome(name: &str, ok: bool) -> crate::doctor::Outcome {
        crate::doctor::Outcome {
            name: name.to_string(),
            ok,
            detail: if ok {
                "resolves".into()
            } else {
                "not installed in the sandbox".into()
            },
        }
    }

    /// **A program nobody probed is `None`, never `Some(false)`**, and every
    /// other guarantee in this file rests on it.
    ///
    /// Suppression acts on `Some(false)`. Collapsing *unknown* into *absent*
    /// would make a first run, a new hook, or a cache somebody deleted look
    /// exactly like a sandbox with nothing installed — and omh would ship a
    /// session with every hook switched off, silently, for a repo whose
    /// toolchain is fine.
    #[test]
    fn a_program_nobody_probed_is_unknown_rather_than_missing() {
        let mut facts = Facts::default();
        facts.learn("omh/claude:abc", &[outcome("cargo", true)]);

        assert_eq!(resolves(&facts, "omh/claude:abc", "cargo"), Some(true));
        assert_eq!(
            resolves(&facts, "omh/claude:abc", "shellcheck"),
            None,
            "a program in no probe is unknown, and unknown suppresses nothing"
        );
        assert_eq!(
            resolves(&Facts::default(), "omh/claude:abc", "cargo"),
            None,
            "and an empty cache knows nothing about anything"
        );
    }

    /// Facts are about an **image**, not a repo — which is the whole reason
    /// they can be cached at all.
    ///
    /// Two repos rendering the same recipe get the same tag and share the
    /// answer; a repo that gains a provide gets a different tag and asks again.
    /// A cache that leaked across tags would answer `cargo` *resolves* about a
    /// stack layer that was never built, and the hook would ship into a sandbox
    /// that cannot run it.
    #[test]
    fn what_is_known_about_one_image_is_not_known_about_another() {
        let mut facts = Facts::default();
        facts.learn("omh/claude:with-rust", &[outcome("cargo", true)]);

        assert_eq!(resolves(&facts, "omh/claude:plain", "cargo"), None);
        assert_eq!(
            facts.about("omh/claude:plain"),
            BTreeMap::new(),
            "a tag nobody probed knows nothing"
        );
        assert_eq!(
            facts.about("omh/claude:with-rust"),
            BTreeMap::from([("cargo".to_string(), true)])
        );
    }

    /// A tag is `omh/<adapter>:<hash>`, so both `/` and `:` are in the key.
    ///
    /// Written down because the obvious storage — one file per image — makes
    /// that a path with a directory separator and a Windows drive letter in it.
    /// A JSON object key holds either happily, and this is what stops somebody
    /// "simplifying" it into a filename later.
    #[test]
    fn a_tag_with_a_slash_and_a_colon_survives_the_round_trip() {
        let (_d, paths) = fixture();
        let tag = "omh/claude-code:9f2ab1c0";

        let mut facts = Facts::default();
        facts.learn(tag, &[outcome("cargo", true), outcome("cc", false)]);
        facts.save(&paths).unwrap();

        let read = Facts::load(&paths);
        assert_eq!(read, facts, "what was written is what comes back");
        assert_eq!(resolves(&read, tag, "cargo"), Some(true));
        assert_eq!(resolves(&read, tag, "cc"), Some(false));
    }

    /// Only what has never been asked is asked again — including the programs
    /// that came back **false**.
    ///
    /// A `false` is an answer. Re-probing it would spend a container run per
    /// launch to be told the same thing, and re-probing it *because* it was
    /// false is the shape that quietly turns the cache into a no-op.
    #[test]
    fn only_programs_nobody_has_asked_about_are_probed_again() {
        let mut facts = Facts::default();
        facts.learn(
            "omh/claude:abc",
            &[outcome("cargo", true), outcome("cc", false)],
        );

        let wanted = BTreeSet::from([
            "cargo".to_string(),
            "cc".to_string(),
            "shellcheck".to_string(),
        ]);
        assert_eq!(
            facts.unseen("omh/claude:abc", &wanted),
            vec!["shellcheck".to_string()],
            "a recorded `false` is an answer, not a reason to ask again"
        );
        assert_eq!(
            facts.unseen("omh/claude:other", &wanted),
            vec![
                "cargo".to_string(),
                "cc".to_string(),
                "shellcheck".to_string()
            ],
            "and an image nobody has probed owes every question"
        );
    }

    /// A cache omh cannot read suppresses **nothing**.
    ///
    /// The failure has to fall this way round. A corrupt `facts.json` that read
    /// as "no program resolves" would switch off every hook in every repo on
    /// the machine, in a session that otherwise looks completely normal — and
    /// the user has no reason to suspect a cache file they never knew existed.
    #[test]
    fn a_cache_omh_cannot_read_is_not_a_sandbox_with_nothing_in_it() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(&paths.root).unwrap();
        std::fs::write(paths.facts(), "{ this is not json").unwrap();

        let facts = Facts::load(&paths);
        assert_eq!(
            resolves(&facts, "omh/claude:abc", "cargo"),
            None,
            "unreadable is cannot-tell, and cannot-tell suppresses nothing"
        );
        assert_eq!(facts, Facts::default());
    }

    /// A later probe replaces an earlier answer about the same image.
    ///
    /// The image can genuinely change under one tag — `docker rmi` and a
    /// rebuild, a base the registry moved — and the newest measurement is the
    /// one taken against what is actually there. Merging by keeping the old
    /// value would make a fixed toolchain permanently invisible.
    #[test]
    fn a_new_measurement_replaces_the_one_before_it() {
        let mut facts = Facts::default();
        facts.learn("omh/claude:abc", &[outcome("cargo", false)]);
        facts.learn("omh/claude:abc", &[outcome("cargo", true)]);
        assert_eq!(resolves(&facts, "omh/claude:abc", "cargo"), Some(true));
    }
}
