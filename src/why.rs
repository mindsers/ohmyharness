//! `omh why` — who put this here, and on what grounds.
//!
//! An opinionated tool's characteristic failure is opacity: "without the hassle
//! of understanding" curdling into "unable to understand". `omh info --repo`
//! answers *where* a value came from; this answers *why*, and for whose reasons.
//!
//! The distinction that matters is authorship. omh's own choices carry a
//! rationale, a measured cost, what was considered instead, and a way out. Your
//! choices carry provenance and **nothing else** — a tool that answers "because
//! it is in the base set" about something you added yourself is lying about its
//! own authorship, and being able to tell the two apart is the whole feature.
//!
//! omh's choices are **features**, and a feature is answered for whole. Its
//! parts — a server, hooks, a rules section — are generated at launch, switched
//! together, and none is a file of yours, so no part has an answer of its own:
//! `omh why graph-first` is `omh why codegraph`. What is yours is what is in
//! your catalogue, and that is everything else.

use crate::base::{Entry, Manifest, Rejected};
use crate::config::{Layer, Setting};
use std::collections::{BTreeMap, BTreeSet};

/// What omh knows, assembled from the manifest and the resolved profile.
pub struct Catalog<'a> {
    pub manifest: &'a Manifest,
    /// What is actually installed, with the layer it won in — your catalogue's
    /// servers and hooks, never omh's.
    pub installed: Vec<Setting>,
    /// What sits in your catalogue under a name of omh's, by kind: hook files,
    /// and the copies an earlier omh seeded into `mcp.json`. A leftover is one
    /// matching a part *of the same kind* — a hook of yours named like a
    /// server is a hook, and runs.
    pub copies: Vec<(crate::adapter::Capability, Setting)>,
    /// Name → what `init` would have written for a detected stack: omh's
    /// writing, but not omh's opinion.
    ///
    /// Carries the command and layer, not just a label, because the name alone
    /// proves nothing — anyone can create `rust-test.json`.
    pub derived: BTreeMap<String, Derived>,
    /// Features this repo has switched off. Not a property of the entry —
    /// `omh why` answers about the base set and about *here*, and an answer
    /// that explains why something is installed while it is disabled three
    /// feet away is only half true.
    pub off: BTreeSet<String>,
}

/// What `init` writes for a detected stack, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    /// e.g. "rust, detected from Cargo.toml"
    pub from: String,
    /// The command `init` would have written — `stack.test` or `stack.format`.
    pub command: String,
    /// Always the shared layer. `init` writes nowhere else, so a hook in
    /// `local` was not written by `init` whatever it is called.
    pub layer: Layer,
}

#[derive(Debug)]
pub enum Verdict<'a> {
    /// One of omh's features, asked about by its name, a part's, or a name a
    /// part used to have — one answer for all of them.
    ///
    /// `stale` is files in your catalogue named like one of its parts: copies
    /// an older omh seeded, never read now. Carried rather than ignored:
    /// reporting one as what runs would be a confident wrong answer, and not
    /// mentioning it leaves somebody editing a file with no effect.
    Feature {
        name: String,
        parts: Vec<&'a Entry>,
        stale: Vec<&'a Setting>,
    },
    /// omh wrote it, from your repo rather than from its opinion. Nothing to
    /// argue about and nothing curated — `cargo fmt` is just what formats Rust.
    Derived { yours: &'a Setting, from: String },
    /// Yours. omh has no rationale for this and will not invent one.
    Yours { yours: &'a Setting },
    /// Considered and turned down. Recorded so the same candidate is not
    /// re-litigated every time somebody rediscovers it.
    Rejected { rejection: &'a Rejected },
    /// Nothing known. Lists what is, rather than guessing.
    Unknown { known: Vec<String> },
}

impl<'a> Catalog<'a> {
    pub fn why(&'a self, name: &str) -> Verdict<'a> {
        if let Some(feature) = self.feature_of(name) {
            let parts: Vec<&Entry> = self
                .manifest
                .entries
                .iter()
                .filter(|e| e.feature == feature)
                .collect();
            let stale = self
                .copies
                .iter()
                .filter(|(cap, s)| {
                    parts.iter().any(|p| {
                        p.kind.capability() == *cap
                            && (p.name == s.key || p.was.iter().any(|w| *w == s.key))
                    })
                })
                .map(|(_, s)| s)
                .collect();
            return Verdict::Feature {
                name: feature.to_string(),
                parts,
                stale,
            };
        }
        match self.installed.iter().find(|s| s.key == name) {
            // A name match alone is not evidence omh wrote this. `init` writes
            // stack hooks only into the *shared* layer and only with the
            // command detection produced, so both are checkable — and a
            // hand-written `rust-test.json` in `local` was being reported as
            // "written by omh init", which is the same lie about authorship
            // this module exists to prevent, pointing the other way.
            Some(yours) => match self.derived.get(name) {
                Some(d) if d.layer == yours.layer && d.command == yours.value => Verdict::Derived {
                    yours,
                    from: d.from.clone(),
                },
                _ => Verdict::Yours { yours },
            },
            None => match self.manifest.rejection(name) {
                Some(rejection) => Verdict::Rejected { rejection },
                None => Verdict::Unknown {
                    known: self.known(),
                },
            },
        }
    }

    /// The feature `name` names: the feature itself, one of its parts, or a
    /// name a part used to have.
    fn feature_of(&self, name: &str) -> Option<&'a str> {
        self.manifest
            .entries
            .iter()
            .find(|e| e.feature == name || e.name == name || e.was.iter().any(|w| w == name))
            .map(|e| e.feature.as_str())
    }

    /// Everything answerable, for when a name matches nothing. Guessing what
    /// somebody meant would send them to the wrong explanation, which is worse
    /// than admitting ignorance.
    fn known(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .manifest
            .entries
            .iter()
            .map(|e| e.name.clone())
            // Features too: they are the `[omh]` key space and the manifest
            // instructs people to type them, so a list that omits them sends
            // somebody looking for a name omh itself printed.
            .chain(self.manifest.entries.iter().map(|e| e.feature.clone()))
            .chain(self.installed.iter().map(|s| s.key.clone()))
            .chain(self.manifest.rejected.iter().map(|r| r.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// Whether a measurement predates the base set it is shipped in.
///
/// Compared against the **manifest version**, not the entry's `since`. Against
/// `since` this could essentially never fire: a measurement is taken at or
/// after the entry was added, and `since` never moves, so no shipped number was
/// flaggable in 2027 or 2035. The proof it was inert is in this repo's history
/// — a byte count went wrong within a day of being written and staleness said
/// nothing, because it was structurally incapable of saying anything.
///
/// The manifest version moves every time the base set is re-cut, which is
/// exactly when measurements should be re-taken or re-affirmed.
fn is_stale(measured_on: &str, manifest_version: &str) -> bool {
    use crate::base::parse_ym as ym;
    match (ym(measured_on), ym(manifest_version)) {
        // Unparseable dates are not evidence of staleness. Saying nothing beats
        // labelling a good measurement stale because a format changed — and the
        // curation test rejects an unreadable date at load, so this arm is a
        // fallback rather than the guard.
        (Some(on), Some(version)) => on < version,
        _ => false,
    }
}

/// Every cost of every part, each under the part it is a cost of.
///
/// Per part, because a feature's cost is its parts' together and the reader
/// has to see which part pays which. The date and the method ride on every
/// line: a measurement without them reads as a fact about right now, which is
/// the fabricated authority this whole command exists to avoid.
fn costs(parts: &[&Entry], version: &str, out: &mut String) {
    let measured: Vec<(&str, String, &crate::base::Measured)> = parts
        .iter()
        .flat_map(|p| {
            p.measured
                .iter()
                .map(move |m| (p.name.as_str(), format!("{} {}", m.value, m.what), m))
        })
        .collect();
    // Pad the value and its subject as one unit. Padding only the subject makes
    // the `measured` column wander, which reads as sloppiness in the exact place
    // the output is asking to be trusted.
    let width = measured
        .iter()
        .map(|(_, c, _)| c.chars().count())
        .max()
        .unwrap_or(0);
    let names = part_width(parts);
    let mut label = "costs";
    let mut last = "";
    for (part, claim, m) in &measured {
        let stale = if is_stale(&m.on, version) {
            "  (stale)"
        } else {
            ""
        };
        let shown = if *part == last { "" } else { part };
        out.push_str(&format!(
            "  {label:<11} {shown:<names$}   {claim:<width$}   measured {}{stale}\n",
            m.on
        ));
        out.push_str(&format!("  {:<11} {:<names$}   {}\n", "", "", m.how));
        label = "";
        last = part;
    }
    // `how` prints on every line, so a stale measurement needs only to say
    // that the base set has been re-cut since it was taken.
    if measured.iter().any(|(_, _, m)| is_stale(&m.on, version)) {
        out.push_str(&format!(
            "              (stale: taken before base set {version} — re-measure or re-affirm)\n"
        ));
    }
}

/// What was considered instead of each part, under the part.
fn alternatives(parts: &[&Entry], out: &mut String) {
    let width = parts
        .iter()
        .flat_map(|p| &p.instead_of)
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    let names = part_width(parts);
    let mut label = "instead of";
    for part in parts {
        let mut shown = part.name.as_str();
        for a in &part.instead_of {
            out.push_str(&format!(
                "  {label:<11} {shown:<names$}   {:<width$}   {}\n",
                a.name, a.why
            ));
            label = "";
            shown = "";
        }
    }
}

fn part_width(parts: &[&Entry]) -> usize {
    parts
        .iter()
        .map(|p| p.name.chars().count())
        .max()
        .unwrap_or(0)
}

/// Every answer names the manifest that produced it.
///
/// Four separate wrong answers — a stray file becoming the base set, omh
/// disowning its own entries after an upgrade, an untouched hook read as an
/// edit, a permissions error read as "not installed" — were all invisible for
/// the same reason: nothing said which manifest, at which version, answered.
/// One line turns each of them from a confident wrong answer into a visible one.
pub fn render_with_source(
    catalog: &Catalog,
    verdict: &Verdict,
    version: &str,
    source: &str,
) -> String {
    let mut out = render(catalog, verdict, version);
    out.push_str(&format!("\n  answered from {source}\n"));
    out
}

pub fn render(catalog: &Catalog, verdict: &Verdict, version: &str) -> String {
    let mut out = String::new();
    match verdict {
        Verdict::Feature { name, parts, stale } => {
            let off = if catalog.off.contains(name) {
                "  —  off in this repo"
            } else {
                ""
            };
            // As versions, not strings: `2027.2` sorts after `2027.10` as text.
            let since = parts
                .iter()
                .min_by_key(|p| crate::base::parse_ym(&p.since))
                .map_or("", |p| p.since.as_str());
            out.push_str(&format!(
                "{name} — an omh feature, in the base set since {since}{off}\n\n"
            ));
            let names = part_width(parts);
            let mut label = "gathers";
            for part in parts {
                out.push_str(&format!(
                    "  {label:<11} {:<names$}   {}\n",
                    part.name, part.because
                ));
                label = "";
            }
            costs(parts, version, &mut out);
            alternatives(parts, &mut out);
            // One switch, but each part says what switching it off leaves
            // behind — `git-turn` its ref, `git-note` its note — so a part's
            // own text is shown when it differs from the rest.
            let first = parts.first().map(|p| p.remove.as_str()).unwrap_or("");
            if parts.iter().all(|p| p.remove == first) {
                out.push_str(&format!("  {:<11} {first}\n", "remove"));
            } else {
                let mut label = "remove";
                for part in parts {
                    out.push_str(&format!(
                        "  {label:<11} {:<names$}   {}\n",
                        part.name, part.remove
                    ));
                    label = "";
                }
            }
            // Line by line rather than one continued literal. The `\` escape
            // does strip the next line's leading whitespace, so the source
            // reads correctly and prints correctly — until `cargo fmt` joins
            // the literal onto one line and keeps that indentation as literal
            // spaces. Checked twice here, both times by reading the output of
            // a real `omh why` rather than the source.
            out.push_str("\n  A feature is all or nothing: every part above is generated at\n");
            out.push_str("  launch and switched together, and none of them is a file of yours.\n");
            for file in stale {
                out.push_str(&format!(
                    "\n  `{}` in {} is a copy an older omh seeded, and is no longer read.\n",
                    file.key,
                    file.layer.whose()
                ));
            }
        }
        // omh wrote it, so disowning it would be as false as claiming your own
        // entry. But there is no argument to make for it either.
        Verdict::Derived { yours, from } => {
            out.push_str(&format!(
                "{} — written by omh init, from your repo\n\n",
                yours.key
            ));
            out.push_str(&format!("  {:<11} {from}\n", "derived from"));
            out.push_str(&format!("  {:<11} {}\n", "installed", yours.layer.whose()));
            out.push_str(
                "\n  Not a curated choice — it follows from what your repo is,\n  \
                 so there is nothing to argue about. Edit or delete it freely;\n  \
                 init will not write over your version.\n",
            );
        }
        // No rationale, and no claim of base-set membership. omh did not choose
        // this and must not lend it reasoning it does not have.
        Verdict::Yours { yours } => {
            out.push_str(&format!("{} — your choice, not omh's\n\n", yours.key));
            out.push_str(&format!("  {:<11} {}\n", "added in", yours.layer.whose()));
            let shadowed = if yours.shadows.is_empty() {
                "nothing".to_string()
            } else {
                yours
                    .shadows
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str(&format!("  {:<11} {shadowed}\n", "overrides"));
            out.push_str("\n  omh has no rationale for this one — it is yours.\n");
        }
        Verdict::Rejected { rejection } => {
            out.push_str(&format!(
                "{} — considered {}, not in the base set\n\n",
                rejection.name, rejection.considered
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", rejection.because));
        }
        Verdict::Unknown { known } => {
            out.push_str("omh has nothing recorded under that name.\n\n");
            out.push_str("  known:\n");
            for name in known {
                out.push_str(&format!("    {name}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Layer;
    use std::path::Path;

    const BUNDLED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    fn manifest() -> Manifest {
        Manifest::load_dir(Path::new(BUNDLED)).unwrap()
    }

    fn setting(key: &str, value: &str, layer: Layer) -> Setting {
        Setting {
            key: key.into(),
            value: value.into(),
            layer,
            shadows: Vec::new(),
        }
    }

    fn catalog<'a>(m: &'a Manifest, installed: Vec<Setting>) -> Catalog<'a> {
        Catalog {
            manifest: m,
            installed,
            copies: Vec::new(),
            derived: BTreeMap::new(),
            off: BTreeSet::new(),
        }
    }

    /// The feature answer with one part in it, for the cost tests: they are
    /// about how one entry's measurements print, whatever feature it is in.
    fn one(entry: &Entry) -> Verdict<'_> {
        Verdict::Feature {
            name: entry.feature.clone(),
            parts: vec![entry],
            stale: Vec::new(),
        }
    }

    /// `graph-first` is not a hook that happens to mention the graph; it is
    /// part of the graph, and the answer for it is the graph's — headed by the
    /// feature, whichever name was asked, and with a leftover file of that
    /// name in the catalogue or not.
    #[test]
    fn every_part_answers_under_its_features_name() {
        let m = manifest();
        for c in [
            catalog(&m, vec![setting("graph-first", "nudge", Layer::Shared)]),
            catalog(&m, vec![]),
        ] {
            for name in ["graph-first", "omh-codegraph", "codegraph"] {
                let out = render(&c, &c.why(name), "2026.08");
                assert!(
                    out.starts_with("codegraph — an omh feature"),
                    "`omh why {name}`: {out}"
                );
            }
        }
    }

    /// A part of a feature is generated from the manifest at launch and is
    /// not a file anywhere, so the answer for it is the feature's, with no
    /// file of yours in it.
    ///
    /// The leftover case is the one that matters. A repo initialised before
    /// generation can still have hook files under omh's names in its profile;
    /// they are not what runs, so reporting one as what omh ships would be a
    /// confident wrong answer about the thing this command exists to be right
    /// about.
    #[test]
    fn a_generated_entry_is_generated_not_missing_and_not_yours() {
        let m = manifest();

        for name in ["graph-refresh", "git-rules"] {
            assert!(
                matches!(
                    catalog(&m, vec![]).why(name),
                    Verdict::Feature { ref stale, .. } if stale.is_empty()
                ),
                "{name} with no file"
            );
        }

        let leftover = setting("graph-refresh", "what an older omh wrote", Layer::Shared);
        let mut c = catalog(&m, vec![]);
        c.copies = vec![(crate::adapter::Capability::Hooks, leftover)];
        let verdict = c.why("graph-refresh");
        assert!(
            matches!(verdict, Verdict::Feature { ref stale, .. } if stale.len() == 1),
            "a leftover file does not decide, so it cannot be the answer"
        );
        let out = render(&c, &verdict, "2026.08");
        assert!(
            out.contains("this repo") && out.contains("no longer read"),
            "the dead file is named rather than ignored: {out}"
        );
    }

    /// Off here is a fact about this repo, and the answer has to carry it —
    /// otherwise `omh why` explains why something is installed while it is
    /// switched off three feet away.
    #[test]
    fn a_verdict_says_whether_the_feature_is_on_here() {
        let m = manifest();
        let mut c = catalog(&m, vec![]);
        c.off = ["codegraph".to_string()].into();

        let out = render(&c, &c.why("graph-refresh"), "2026.08");
        assert!(out.contains("off in this repo"), "got: {out}");
    }

    /// `git-notice` is a feature with no entry of its own, and the manifest
    /// prints `git-notice = false` as the way out of `git-rules`. Asked
    /// about it, omh answered "nothing recorded under that name" and listed
    /// names that did not include it.
    ///
    /// Feature names are user-facing vocabulary now — they are the whole
    /// `[omh]` key space, and `settings::validate` errors in terms of them —
    /// so the command built to explain omh's choices has to know them. An
    /// instruction printed as the way out and then not recognised is the shape
    /// CONTRIBUTING singles out: it worked, it said so, and it was wrong.
    /// Every name the shipped manifest records as rejected has to be
    /// answerable, because `rejected` is where a *retired* entry goes and the
    /// manifest promises `omh why` still answers for it.
    ///
    /// `git-unavailable` is the case that made this matter: it shipped as a
    /// hook through 2026.08, and a user with it in their settings from before
    /// then types the name omh told them to type. Deleting the whole
    /// `[[rejected]]` block for it left the suite green — the `Rejected` arm is
    /// exercised only through a hand-built fixture, never against what omh
    /// actually ships.
    #[test]
    fn every_rejected_name_the_manifest_ships_is_answerable() {
        let m = manifest();
        let names: Vec<String> = m.rejected.iter().map(|r| r.name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "git-unavailable"),
            "the retired hook is the one users still have written down: {names:?}"
        );

        let c = catalog(&m, vec![]);
        for name in &names {
            assert!(
                matches!(c.why(name), Verdict::Rejected { .. }),
                "`omh why {name}` has to answer, not shrug"
            );
        }
    }

    #[test]
    fn a_feature_is_explained_even_when_no_entry_shares_its_name() {
        let m = manifest();
        let c = catalog(&m, vec![]);

        // The feature, whole — not the first entry of it: `git-rules` alone
        // would be a part named as the whole.
        match c.why("git-notice") {
            Verdict::Feature { parts, .. } => {
                let names: Vec<&str> = parts.iter().map(|e| e.name.as_str()).collect();
                assert_eq!(names, ["git-rules", "git-note", "git-turn"]);
            }
            other => panic!("expected the feature, got {other:?}"),
        }
        let out = render(&c, &c.why("git-notice"), "2026.08");
        assert!(
            out.contains("git-rules"),
            "must list what the feature gathers: {out}"
        );

        match c.why("teleport") {
            Verdict::Unknown { known } => assert!(
                known.contains(&"git-notice".to_string()),
                "a name omh tells you to type has to be discoverable: {known:?}"
            ),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// The other half of the grouping: what a feature brought with it. Nobody
    /// can answer "what does removing codegraph take" from a comment header.
    #[test]
    fn why_a_feature_lists_what_it_brings() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c, &c.why("codegraph"), "2026.08");
        for name in ["graph-orient", "graph-first", "graph-read", "graph-refresh"] {
            assert!(out.contains(name), "{name} is part of codegraph: {out}");
        }
    }

    /// A feature is one thing, so it gets one answer whichever of its names is
    /// typed — the feature's, any part's, or a name a part used to have. No
    /// part is answered for on its own. The one answer carries what each part
    /// is for and what each costs, since justifying costs is what `omh why`
    /// is for, and a part's cost is the feature's.
    #[test]
    fn every_name_in_a_feature_gets_the_features_answer() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        for feature in ["codegraph", "memory", "git-notice"] {
            let answer = render(&c, &c.why(feature), "2026.08");
            let parts: Vec<&crate::base::Entry> =
                m.entries.iter().filter(|e| e.feature == feature).collect();
            assert!(!parts.is_empty(), "{feature}");
            for part in &parts {
                for name in std::iter::once(&part.name).chain(&part.was) {
                    assert_eq!(
                        render(&c, &c.why(name), "2026.08"),
                        answer,
                        "`omh why {name}` answers apart from `omh why {feature}`"
                    );
                }
                assert!(
                    answer.contains(&part.name),
                    "{feature} names {}: {answer}",
                    part.name
                );
                for m in &part.measured {
                    assert!(
                        answer.contains(&m.value),
                        "{feature} carries {}'s cost {}: {answer}",
                        part.name,
                        m.value
                    );
                }
                for a in &part.instead_of {
                    assert!(
                        answer.contains(&a.name),
                        "{feature} carries {}: {answer}",
                        a.name
                    );
                }
                // The way out, each part's own: `git-turn` and `git-note` say
                // what they leave behind, and the first part's text alone
                // dropped both.
                assert!(
                    answer.contains(&part.remove),
                    "{feature} carries {}'s way out: {answer}",
                    part.name
                );
            }
        }
    }

    /// A copy an older omh seeded into your catalogue — under the server's
    /// old name as much as its new one — is named in the feature's answer as
    /// not read. Leftovers are matched by kind: a hook of yours that shares a
    /// server's name is a hook, runs, and is not anybody's leftover.
    #[test]
    fn a_leftover_is_matched_by_kind_and_by_every_name_a_part_had() {
        let m = manifest();
        let mut c = catalog(&m, vec![]);
        c.copies = vec![
            (
                crate::adapter::Capability::Mcp,
                setting("codegraph", "codebase-memory-mcp", Layer::Personal),
            ),
            (
                crate::adapter::Capability::Hooks,
                setting("omh-memory", "a hook of yours", Layer::Personal),
            ),
        ];
        match c.why("codegraph") {
            Verdict::Feature { stale, .. } => assert_eq!(
                stale.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
                ["codegraph"]
            ),
            other => panic!("expected the feature, got {other:?}"),
        }
        match c.why("memory") {
            Verdict::Feature { stale, .. } => assert!(stale.is_empty(), "{stale:?}"),
            other => panic!("expected the feature, got {other:?}"),
        }
    }

    /// The load-bearing case: omh must not claim authorship of your choices.
    #[test]
    fn your_own_entry_gets_no_rationale() {
        let m = manifest();
        let c = catalog(&m, vec![setting("linear", "npx", Layer::Local)]);
        match c.why("linear") {
            Verdict::Yours { yours } => assert_eq!(yours.layer, Layer::Local),
            other => panic!("expected Yours, got {other:?}"),
        }
    }

    /// A rejection is a product artifact, not a "not found".
    #[test]
    fn a_rejected_candidate_explains_its_rejection() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        match c.why("gitnexus") {
            Verdict::Rejected { rejection } => {
                assert!(
                    rejection.because.contains("Noncommercial"),
                    "{}",
                    rejection.because
                );
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_name_lists_what_is_known_instead_of_guessing() {
        let m = manifest();
        let c = catalog(&m, vec![setting("linear", "npx", Layer::Local)]);
        match c.why("lienar") {
            Verdict::Unknown { known } => {
                assert!(known.contains(&"codegraph".to_string()), "{known:?}");
                assert!(known.contains(&"linear".to_string()), "{known:?}");
                assert!(known.contains(&"gitnexus".to_string()), "{known:?}");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    // ── rendering ────────────────────────────────────────────────────────────

    #[test]
    fn omhs_choice_reports_its_argument_and_its_cost() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c, &c.why("codegraph"), "2026.08");

        assert!(out.contains("an omh feature"), "{out}");
        assert!(
            out.contains("re-grepping"),
            "the argument is missing:\n{out}"
        );
        assert!(out.contains("0.46s"), "the cost is missing:\n{out}");
        assert!(
            out.contains("gitnexus"),
            "the alternatives are missing:\n{out}"
        );
        assert!(out.contains("omh set codegraph off"), "no way out:\n{out}");
    }

    /// Cost is measured and benefit is argued: two different kinds of claim.
    /// A measurement printed without its date reads as a fact about right now,
    /// which is exactly the fabricated authority this command exists to avoid.
    #[test]
    fn every_measured_cost_carries_the_date_it_was_taken() {
        let m = manifest();
        for entry in &m.entries {
            let out = render(&catalog(&m, vec![]), &one(entry), "2026.08");
            for measured in &entry.measured {
                let line = out
                    .lines()
                    .find(|l| l.contains(&measured.value) && l.contains(&measured.what))
                    .unwrap_or_else(|| panic!("{}: no line for {}", entry.name, measured.what));
                assert!(
                    line.contains(&measured.on),
                    "{}: cost printed without its date: {line}",
                    entry.name
                );
            }
        }
    }

    /// A measurement taken before the entry's own version describes an older
    /// thing. Still worth showing — it is the only number there is — but not
    /// worth presenting as current.
    #[test]
    fn a_measurement_older_than_the_entry_is_marked_stale() {
        let m: Manifest = toml::from_str(
            r#"
version = "2026.08"
[[entry]]
name = "x"
kind = "mcp"
feature = "x"
since = "2026.08"
because = "b"
remove = "r"
command = "c"
[[entry.measured]]
what = "per turn"
value = "1s"
how = "h"
on = "2026-01-01"
[[entry.instead_of]]
name = "a"
why = "w"
"#,
        )
        .unwrap();
        // Measured in 2026-01, shipped in a base set cut in 2026.08: the number
        // predates the version it is being presented as evidence for.
        let c = catalog(&m, vec![]);
        let out = render(&c, &one(&m.entries[0]), "2026.08");
        assert!(
            out.contains("stale"),
            "an outdated measurement must say so:\n{out}"
        );

        // The same measurement, in the base set it was actually taken for, is
        // not stale. Without this the check could be `=> true` and stay green —
        // which it was, and did.
        let fresh = render(&c, &one(&m.entries[0]), "2026.01");
        assert!(
            !fresh.contains("stale"),
            "a current measurement must not be flagged:\n{fresh}"
        );
    }

    /// Staleness used to compare against the entry's own `since`, which never
    /// moves — so no shipped measurement could ever be flagged, in 2027 or in
    /// 2035. The proof it was inert is in this repo: a byte count went wrong
    /// within a day of being written and this said nothing.
    ///
    /// Against the manifest version it fires exactly when the base set is
    /// re-cut, which is when numbers should be re-taken or re-affirmed.
    #[test]
    fn staleness_is_measured_against_the_base_set_not_the_entrys_own_age() {
        let m = manifest();
        let entry = m.entry("omh-codegraph").unwrap();

        let current = render(&catalog(&m, vec![]), &one(entry), &m.version);
        assert!(
            !current.contains("stale"),
            "shipped numbers are current:\n{current}"
        );

        // A later cut of the base set, with the same measurements carried over.
        let later = render(&catalog(&m, vec![]), &one(entry), "2027.01");

        // Assert the marker on the cost line itself, not merely the word
        // "stale" somewhere in the output. There are two call sites — the
        // per-measurement marker and the trailing summary — and a bare
        // `contains` is satisfied by either, so it cannot tell which one is
        // wired to the wrong comparison. Mutating one call site left this test
        // green until the assertion was tightened.
        let first = entry.measured.first().expect("codegraph has measurements");
        let cost_line = later
            .lines()
            .find(|l| l.contains(&first.value) && l.contains("measured"))
            .unwrap_or_else(|| panic!("no cost line in:\n{later}"));
        assert!(
            cost_line.contains("(stale)"),
            "re-cutting the base set must flag every carried-over number:\n{cost_line}"
        );
    }

    /// `how` is the entire evidence for a number. It used to print only when a
    /// measurement was stale — which, given the bug above, meant never. A bare
    /// figure with no method is the shape of claim this command replaced.
    #[test]
    fn the_method_is_shown_for_every_cost_not_only_stale_ones() {
        let m = manifest();
        let entry = m.entry("omh-codegraph").unwrap();
        let out = render(&catalog(&m, vec![]), &one(entry), &m.version);

        for measured in &entry.measured {
            assert!(
                out.contains(&measured.how),
                "cost `{}` printed without how it was taken:\n{out}",
                measured.value
            );
        }
    }

    /// `init` generates `<stack>-test` and `<stack>-format` hooks from what it
    /// detected. Those are omh's writing but not omh's *opinion* — there is
    /// nothing curated to argue about, `cargo fmt` is simply what formats Rust.
    ///
    /// Calling them "your choice" is the same false claim of authorship as
    /// calling your own entry omh's, just pointing the other way, and it was the
    /// answer this command gave until running it revealed otherwise.
    fn rust_format() -> Derived {
        Derived {
            from: "rust, detected from Cargo.toml".into(),
            command: "cargo fmt".into(),
            layer: Layer::Shared,
        }
    }

    /// `init` writes stack hooks only into the shared layer, so one sitting in
    /// `local` was not written by `init` whatever it is called. Claiming
    /// otherwise is the same authorship lie this module exists to prevent,
    /// aimed the other way — reproduced with a hand-written `rust-test.json`.
    #[test]
    fn a_hook_in_a_layer_init_never_writes_to_is_yours() {
        let m = manifest();
        let mut c = catalog(&m, vec![setting("rust-format", "cargo fmt", Layer::Local)]);
        c.derived.insert("rust-format".into(), rust_format());
        assert!(
            matches!(c.why("rust-format"), Verdict::Yours { .. }),
            "init does not write to local, so this is not init's"
        );
    }

    /// Right name, right layer, different command: you rewrote it, and omh must
    /// not print "there is nothing to argue about" over your own work.
    #[test]
    fn a_rewritten_stack_hook_is_yours() {
        let m = manifest();
        let mut c = catalog(
            &m,
            vec![setting(
                "rust-format",
                "cargo +nightly fmt --all",
                Layer::Shared,
            )],
        );
        c.derived.insert("rust-format".into(), rust_format());
        assert!(matches!(c.why("rust-format"), Verdict::Yours { .. }));
    }

    #[test]
    fn a_hook_derived_from_your_stack_is_neither_omhs_opinion_nor_yours() {
        let m = manifest();
        let mut c = catalog(&m, vec![setting("rust-format", "cargo fmt", Layer::Shared)]);
        c.derived.insert("rust-format".into(), rust_format());

        match c.why("rust-format") {
            Verdict::Derived { from, .. } => assert!(from.contains("Cargo.toml"), "{from}"),
            other => panic!("expected Derived, got {other:?}"),
        }

        let out = render(&c, &c.why("rust-format"), "2026.08");
        assert!(!out.contains("base set"), "claims it is curated:\n{out}");
        assert!(
            !out.contains("your choice"),
            "disowns something omh wrote:\n{out}"
        );
        assert!(
            out.contains("Cargo.toml"),
            "does not say what it was derived from:\n{out}"
        );
    }

    /// The load-bearing negative. omh must not lend its reasoning to a choice
    /// it did not make — no rationale, and no claim of base-set membership.
    #[test]
    fn your_own_choice_never_borrows_omhs_authority() {
        let m = manifest();
        let c = catalog(&m, vec![setting("linear", "npx", Layer::Local)]);
        let out = render(&c, &c.why("linear"), "2026.08");

        assert!(out.contains("your choice"), "{out}");
        assert!(!out.contains("base set"), "claims omh installed it:\n{out}");
        assert!(!out.contains("because"), "invents a rationale:\n{out}");
        assert!(
            out.contains("local"),
            "provenance is the one thing it can say:\n{out}"
        );
    }

    /// The line that converts four silent wrong answers into visible ones. A
    /// stray manifest, a post-upgrade drift, an unreadable layer — each of them
    /// produces a confident answer, and the only way a reader can tell is by
    /// seeing which manifest was consulted.
    #[test]
    fn every_answer_names_the_manifest_that_produced_it() {
        let m = manifest();
        let c = catalog(
            &m,
            vec![setting(
                "omh-codegraph",
                "codebase-memory-mcp",
                Layer::Shared,
            )],
        );
        let out = render_with_source(
            &c,
            &c.why("omh-codegraph"),
            "2026.08",
            "/home/x/.omh/base/2026.08.toml · 2026.08",
        );
        assert!(out.contains("answered from"), "{out}");
        assert!(out.contains("2026.08.toml"), "{out}");
    }

    /// The whole `Rejected` arm could be emptied and the suite stayed green —
    /// and it carries the highest-consequence string the command emits. The
    /// gitnexus entry is a *licence* warning: a user who installs it at work is
    /// in violation of a dependency they never chose.
    #[test]
    fn a_rejection_prints_its_reasoning_and_when_it_was_considered() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c, &c.why("gitnexus"), &m.version);

        let r = m.rejection("gitnexus").unwrap();
        assert!(
            out.contains(&r.because),
            "the reasoning is the whole point:\n{out}"
        );
        assert!(
            out.contains(&r.considered),
            "when it was considered:\n{out}"
        );
        assert!(
            out.contains("Noncommercial"),
            "the licence problem must survive:\n{out}"
        );
    }

    #[test]
    fn an_unknown_name_prints_the_alternatives_it_does_know() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c, &c.why("lienar"), "2026.08");
        assert!(out.contains("codegraph"), "{out}");
        assert!(
            !out.contains("an omh feature"),
            "guessed at a match:\n{out}"
        );
    }
}
