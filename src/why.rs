//! `omh why` — who put this here, and on what grounds.
//!
//! An opinionated tool's characteristic failure is opacity: "without the hassle
//! of understanding" curdling into "unable to understand". `omh config` answers
//! *where* a value came from; this answers *why*, and for whose reasons.
//!
//! The distinction that matters is authorship. omh's own choices carry a
//! rationale, a measured cost, what was considered instead, and a way out. Your
//! choices carry provenance and **nothing else** — a tool that answers "because
//! it is in the base set" about something you added yourself is lying about its
//! own authorship, and being able to tell the two apart is the whole feature.
//!
//! Authorship is *derived*, never recorded. The base set is seeded into your
//! profile at `init` and then lives as ordinary config, so an omh entry and one
//! of yours are byte-identical in the same file. Comparing against the manifest
//! recovers the distinction without a marker that could go stale — and comparing
//! the value as well as the name yields the state people actually want to know
//! about: that they are running a modified default.

use crate::base::{Entry, Manifest, Rejected};
use crate::config::{Layer, Setting};
use std::collections::BTreeMap;

/// What omh knows, assembled from the manifest and the resolved profile.
pub struct Catalog<'a> {
    pub manifest: &'a Manifest,
    /// Name → exactly what omh ships, for deciding whether your copy is
    /// modified. Built by the caller because the baseline for an MCP server
    /// lives in the manifest while a hook's lives in code.
    pub baselines: BTreeMap<String, String>,
    /// What is actually installed, with the layer it won in.
    pub installed: Vec<Setting>,
    /// Name → what `init` would have written for a detected stack: omh's
    /// writing, but not omh's opinion.
    ///
    /// Carries the command and layer, not just a label, because the name alone
    /// proves nothing — anyone can create `rust-test.json`.
    pub derived: BTreeMap<String, Derived>,
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
    /// omh chose it and your copy matches what omh ships.
    Omh {
        entry: &'a Entry,
        yours: &'a Setting,
    },
    /// omh chose it, and the copy on disk is not what omh ships **now**.
    ///
    /// Deliberately not called "modified by you". omh cannot tell who changed
    /// it: `init` seeds the profile with `write_if_absent` and never revisits
    /// it, while the shipped baseline moves with every release. So the first
    /// omh upgrade that touches a hook command makes every existing profile
    /// differ — and the previous version of this verdict told all those users
    /// they had edited a file they never opened.
    ///
    /// Naming the difference is honest and useful. Naming a culprit is neither.
    Differs {
        entry: &'a Entry,
        ships: String,
        yours: &'a Setting,
    },
    /// omh chose it and it is not in your profile — removed, or `init` has not
    /// run. Not an error: leaving is supposed to be easy.
    Removed { entry: &'a Entry },
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
        let entry = self.manifest.entry(name);
        let yours = self.installed.iter().find(|s| s.key == name);

        match (entry, yours) {
            (Some(entry), Some(yours)) => match self.baselines.get(name) {
                // A baseline that matches means untouched. A baseline omh does
                // not have means it cannot claim you changed anything, so the
                // quiet answer is the honest one.
                Some(ships) if ships != &yours.value => Verdict::Differs {
                    entry,
                    ships: ships.clone(),
                    yours,
                },
                _ => Verdict::Omh { entry, yours },
            },
            (Some(entry), None) => Verdict::Removed { entry },
            // A name match alone is not evidence omh wrote this. `init` writes
            // stack hooks only into the *shared* layer and only with the
            // command detection produced, so both are checkable — and a
            // hand-written `rust-test.json` in `local` was being reported as
            // "written by omh init", which is the same lie about authorship
            // this module exists to prevent, pointing the other way.
            (None, Some(yours)) => match self.derived.get(name) {
                Some(d) if d.layer == yours.layer && d.command == yours.value => Verdict::Derived {
                    yours,
                    from: d.from.clone(),
                },
                _ => Verdict::Yours { yours },
            },
            (None, None) => match self.manifest.rejection(name) {
                Some(rejection) => Verdict::Rejected { rejection },
                None => Verdict::Unknown {
                    known: self.known(),
                },
            },
        }
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

fn costs(entry: &Entry, version: &str, out: &mut String) {
    // Pad the value and its subject as one unit. Padding only the subject makes
    // the `measured` column wander, which reads as sloppiness in the exact place
    // the output is asking to be trusted.
    let claims: Vec<String> = entry
        .measured
        .iter()
        .map(|m| format!("{} {}", m.value, m.what))
        .collect();
    let width = claims.iter().map(|c| c.chars().count()).max().unwrap_or(0);

    let mut label = "costs";
    for (m, claim) in entry.measured.iter().zip(&claims) {
        let stale = if is_stale(&m.on, version) {
            "  (stale)"
        } else {
            ""
        };
        // The date rides on every cost line. A measurement without one reads as
        // a fact about right now, which is the fabricated authority this whole
        // command exists to avoid.
        out.push_str(&format!(
            "  {label:<11} {claim:<width$}   measured {}{stale}\n",
            m.on
        ));
        // How it was taken, always — not only when stale, as before. On a
        // command whose thesis is that cost is measured and benefit argued,
        // hiding the method on the happy path leaves the reader with a bare
        // number and no way to check it, which is the shape of the claim this
        // command was built to replace.
        out.push_str(&format!("  {:<11} {}\n", "", m.how));
        label = "";
    }
    // `how` now prints on every line, so a stale measurement needs only to say
    // that the base set has been re-cut since it was taken.
    if entry.measured.iter().any(|m| is_stale(&m.on, version)) {
        out.push_str(&format!(
            "              (stale: taken before base set {version} — re-measure or re-affirm)\n"
        ));
    }
}

fn alternatives(entry: &Entry, out: &mut String) {
    let width = entry
        .instead_of
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut label = "instead of";
    for a in &entry.instead_of {
        out.push_str(&format!("  {label:<11} {:<width$}   {}\n", a.name, a.why));
        label = "";
    }
}

/// Every answer names the manifest that produced it.
///
/// Four separate wrong answers — a stray file becoming the base set, omh
/// disowning its own entries after an upgrade, an untouched hook read as an
/// edit, a permissions error read as "not installed" — were all invisible for
/// the same reason: nothing said which manifest, at which version, answered.
/// One line turns each of them from a confident wrong answer into a visible one.
pub fn render_with_source(verdict: &Verdict, version: &str, source: &str) -> String {
    let mut out = render(verdict, version);
    out.push_str(&format!("\n  answered from {source}\n"));
    out
}

pub fn render(verdict: &Verdict, version: &str) -> String {
    let mut out = String::new();
    match verdict {
        Verdict::Omh { entry, yours } => {
            out.push_str(&format!(
                "{} — omh's choice, in the base set since {}\n\n",
                entry.name, entry.since
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            out.push_str(&format!("  {:<11} {}\n", "part of", entry.feature));
            costs(entry, version, &mut out);
            alternatives(entry, &mut out);
            out.push_str(&format!("  {:<11} {}\n", "installed", yours.layer));
            out.push_str(&format!("  {:<11} {}\n", "remove", entry.remove));
        }
        Verdict::Differs {
            entry,
            ships,
            yours,
        } => {
            out.push_str(&format!(
                "{} — omh's choice, and your copy is not what omh ships now\n\n",
                entry.name
            ));
            out.push_str(&format!("  {:<11} {ships}\n", "omh ships"));
            out.push_str(&format!(
                "  {:<11} {}   in {}\n",
                "on disk", yours.value, yours.layer
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            out.push_str(&format!("  {:<11} {}\n", "part of", entry.feature));
            costs(entry, version, &mut out);
            out.push_str(&format!("  {:<11} {}\n", "remove", entry.remove));
            // Which of the two it is, omh does not know — so it says so rather
            // than picking the flattering guess or the accusing one.
            out.push_str(
                "\n  Either you changed it, or omh did in a later version:\n  \
                 `init` seeds your profile once and never rewrites it.\n",
            );
        }
        Verdict::Removed { entry } => {
            out.push_str(&format!(
                "{} — omh's choice, not installed here\n\n",
                entry.name
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            out.push_str(&format!("  {:<11} {}\n", "part of", entry.feature));
            costs(entry, version, &mut out);
            alternatives(entry, &mut out);
            out.push_str(&format!("  {:<11} omh init\n", "restore"));
        }
        // omh wrote it, so disowning it would be as false as claiming your own
        // entry. But there is no argument to make for it either.
        Verdict::Derived { yours, from } => {
            out.push_str(&format!(
                "{} — written by omh init, from your repo\n\n",
                yours.key
            ));
            out.push_str(&format!("  {:<11} {from}\n", "derived from"));
            out.push_str(&format!("  {:<11} {}\n", "installed", yours.layer));
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
            out.push_str(&format!("  {:<11} {}\n", "added in", yours.layer));
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
        let baselines = m
            .entries
            .iter()
            .filter_map(|e| e.command.clone().map(|c| (e.name.clone(), c)))
            .collect();
        Catalog {
            manifest: m,
            baselines,
            installed,
            derived: BTreeMap::new(),
        }
    }

    #[test]
    fn an_untouched_base_entry_is_omhs() {
        let m = manifest();
        let c = catalog(
            &m,
            vec![setting("codegraph", "codebase-memory-mcp", Layer::Shared)],
        );
        assert!(matches!(c.why("codegraph"), Verdict::Omh { .. }));
    }

    /// The state people actually want to know about after six months.
    #[test]
    fn a_changed_base_entry_reports_both_values() {
        let m = manifest();
        let c = catalog(&m, vec![setting("codegraph", "my-fork", Layer::Local)]);
        match c.why("codegraph") {
            Verdict::Differs { ships, yours, .. } => {
                assert_eq!(ships, "codebase-memory-mcp");
                assert_eq!(yours.value, "my-fork");
                assert_eq!(yours.layer, Layer::Local);
            }
            other => panic!("expected Differs, got {other:?}"),
        }
    }

    /// Removing something omh installed is supposed to be easy, so this is a
    /// normal answer rather than an error.
    #[test]
    fn a_removed_base_entry_is_still_explained() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        assert!(matches!(c.why("codegraph"), Verdict::Removed { .. }));
    }

    /// `graph-first` is not a hook that happens to mention the graph; it is
    /// part of the graph, and removing the server takes it too.
    ///
    /// Unanswerable while the grouping was a comment header in the manifest,
    /// which is the whole reason `feature` became a field. Asserted on all
    /// three entry verdicts because a removed or edited entry is exactly when
    /// somebody is asking what it belonged to.
    #[test]
    fn every_entry_answer_names_the_feature_it_is_part_of() {
        let m = manifest();
        for verdict in [
            catalog(&m, vec![setting("graph-first", "nudge", Layer::Shared)]).why("graph-first"),
            catalog(&m, vec![]).why("graph-first"),
            catalog(&m, vec![setting("codegraph", "my-fork", Layer::Local)]).why("codegraph"),
            catalog(
                &m,
                vec![setting("codegraph", "codebase-memory-mcp", Layer::Shared)],
            )
            .why("codegraph"),
        ] {
            // One line, not two `contains` — `remove` already names the
            // feature for these entries, so a split assertion passes on
            // output that never says what anything is part of.
            let out = render(&verdict, "2026.08");
            assert!(
                out.lines()
                    .any(|l| l.trim().starts_with("part of") && l.trim().ends_with("codegraph")),
                "must say what it belongs to: {out}"
            );
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

    /// Hooks have no `command` in the manifest — theirs lives in code — so
    /// without a baseline omh cannot tell modified from untouched. It must not
    /// assume the worse answer and accuse you of an edit you did not make.
    #[test]
    fn an_entry_with_no_baseline_is_not_reported_as_modified() {
        let m = manifest();
        let mut c = catalog(
            &m,
            vec![setting("graph-read", "anything at all", Layer::Shared)],
        );
        c.baselines.remove("graph-read");
        assert!(matches!(c.why("graph-read"), Verdict::Omh { .. }));
    }

    // ── rendering ────────────────────────────────────────────────────────────

    #[test]
    fn omhs_choice_reports_its_argument_and_its_cost() {
        let m = manifest();
        let c = catalog(
            &m,
            vec![setting("codegraph", "codebase-memory-mcp", Layer::Shared)],
        );
        let out = render(&c.why("codegraph"), "2026.08");

        assert!(out.contains("omh's choice"), "{out}");
        assert!(
            out.contains("re-grepping"),
            "the argument is missing:\n{out}"
        );
        assert!(out.contains("0.46s"), "the cost is missing:\n{out}");
        assert!(
            out.contains("gitnexus"),
            "the alternatives are missing:\n{out}"
        );
        assert!(
            out.contains("omh config mcp rm codegraph"),
            "no way out:\n{out}"
        );
    }

    /// Cost is measured and benefit is argued: two different kinds of claim.
    /// A measurement printed without its date reads as a fact about right now,
    /// which is exactly the fabricated authority this command exists to avoid.
    #[test]
    fn every_measured_cost_carries_the_date_it_was_taken() {
        let m = manifest();
        for entry in &m.entries {
            let out = render(&Verdict::Removed { entry }, "2026.08");
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
        let out = render(
            &Verdict::Removed {
                entry: &m.entries[0],
            },
            "2026.08",
        );
        assert!(
            out.contains("stale"),
            "an outdated measurement must say so:\n{out}"
        );

        // The same measurement, in the base set it was actually taken for, is
        // not stale. Without this the check could be `=> true` and stay green —
        // which it was, and did.
        let fresh = render(
            &Verdict::Removed {
                entry: &m.entries[0],
            },
            "2026.01",
        );
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
        let entry = m.entry("codegraph").unwrap();

        let current = render(&Verdict::Removed { entry }, &m.version);
        assert!(
            !current.contains("stale"),
            "shipped numbers are current:\n{current}"
        );

        // A later cut of the base set, with the same measurements carried over.
        let later = render(&Verdict::Removed { entry }, "2027.01");

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
        let entry = m.entry("codegraph").unwrap();
        let out = render(&Verdict::Removed { entry }, &m.version);

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

        let out = render(&c.why("rust-format"), "2026.08");
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
        let out = render(&c.why("linear"), "2026.08");

        assert!(out.contains("your choice"), "{out}");
        assert!(!out.contains("base set"), "claims omh installed it:\n{out}");
        assert!(!out.contains("because"), "invents a rationale:\n{out}");
        assert!(
            out.contains("local"),
            "provenance is the one thing it can say:\n{out}"
        );
    }

    /// Asserts the label→value **pairing**, not that both strings appear
    /// somewhere. The previous version passed with the two values swapped —
    /// output reading `omh ships my-fork` / `on disk codebase-memory-mcp` — which
    /// is the exact inversion this whole module exists to prevent.
    ///
    /// Pairing survives reformatting; column positions would not.
    #[test]
    fn a_differing_entry_pairs_each_value_with_its_own_label() {
        let m = manifest();
        let c = catalog(&m, vec![setting("codegraph", "my-fork", Layer::Local)]);
        let out = render(&c.why("codegraph"), "2026.08");

        let line = |label: &str| {
            out.lines()
                .find(|l| l.trim_start().starts_with(label))
                .unwrap_or_else(|| panic!("no `{label}` line in:\n{out}"))
        };
        assert!(line("omh ships").contains("codebase-memory-mcp"), "{out}");
        assert!(!line("omh ships").contains("my-fork"), "{out}");
        assert!(line("on disk").contains("my-fork"), "{out}");
        assert!(!line("on disk").contains("codebase-memory-mcp"), "{out}");
    }

    /// omh cannot tell an edit from an upgrade — `init` seeds once and never
    /// revisits, while the shipped baseline moves every release. Claiming "you"
    /// accused every user of an edit they never made, the first time a hook
    /// command changed.
    #[test]
    fn a_difference_never_claims_who_caused_it() {
        let m = manifest();
        let c = catalog(&m, vec![setting("codegraph", "my-fork", Layer::Local)]);
        let out = render(&c.why("codegraph"), "2026.08");
        assert!(
            !out.contains("modified by you") && !out.contains("you set"),
            "omh does not know who changed it:\n{out}"
        );
        assert!(out.contains("Either you changed it, or omh did"), "{out}");
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
            vec![setting("codegraph", "codebase-memory-mcp", Layer::Shared)],
        );
        let out = render_with_source(
            &c.why("codegraph"),
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
        let out = render(&c.why("gitnexus"), &m.version);

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

    /// "A default nobody can leave is a cage" is enforced for `Omh` and was not
    /// for `Removed` — its way back could be deleted green.
    #[test]
    fn a_removed_entry_says_how_to_get_it_back() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c.why("codegraph"), &m.version);
        assert!(out.contains("not installed here"), "{out}");
        assert!(out.contains("omh init"), "no way back:\n{out}");
    }

    #[test]
    fn an_unknown_name_prints_the_alternatives_it_does_know() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c.why("lienar"), "2026.08");
        assert!(out.contains("codegraph"), "{out}");
        assert!(!out.contains("omh's choice"), "guessed at a match:\n{out}");
    }
}
