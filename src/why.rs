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
use crate::config::Setting;
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
    /// Name → what it was derived from. `init` writes `<stack>-test` and
    /// `<stack>-format` hooks out of what it detected in your repo: omh's
    /// writing, but not omh's opinion.
    pub derived: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum Verdict<'a> {
    /// omh chose it and your copy matches what omh ships.
    Omh {
        entry: &'a Entry,
        yours: &'a Setting,
    },
    /// omh chose it; you changed it. Both values are reported, because "have I
    /// drifted from the defaults" is the question worth answering here.
    Modified {
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
                Some(ships) if ships != &yours.value => Verdict::Modified {
                    entry,
                    ships: ships.clone(),
                    yours,
                },
                _ => Verdict::Omh { entry, yours },
            },
            (Some(entry), None) => Verdict::Removed { entry },
            (None, Some(yours)) => match self.derived.get(name) {
                Some(from) => Verdict::Derived {
                    yours,
                    from: from.clone(),
                },
                None => Verdict::Yours { yours },
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

/// A measurement taken before the entry's own version describes an older thing.
/// Still worth printing — it is the only number there is — but not worth
/// presenting as current.
fn is_stale(measured_on: &str, since: &str) -> bool {
    use crate::base::parse_ym as ym;
    match (ym(measured_on), ym(since)) {
        // Unparseable dates are not evidence of staleness. Saying nothing beats
        // labelling a good measurement stale because a format changed.
        (Some(on), Some(since)) => on < since,
        _ => false,
    }
}

fn costs(entry: &Entry, out: &mut String) {
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
        let stale = if is_stale(&m.on, &entry.since) {
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
        label = "";
    }
    for m in &entry.measured {
        if is_stale(&m.on, &entry.since) {
            out.push_str(&format!(
                "              taken before {} — {}\n",
                entry.since, m.how
            ));
        }
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

pub fn render(verdict: &Verdict) -> String {
    let mut out = String::new();
    match verdict {
        Verdict::Omh { entry, yours } => {
            out.push_str(&format!(
                "{} — omh's choice, in the base set since {}\n\n",
                entry.name, entry.since
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            costs(entry, &mut out);
            alternatives(entry, &mut out);
            out.push_str(&format!("  {:<11} {}\n", "installed", yours.layer));
            out.push_str(&format!("  {:<11} {}\n", "remove", entry.remove));
        }
        Verdict::Modified {
            entry,
            ships,
            yours,
        } => {
            out.push_str(&format!(
                "{} — omh's choice, modified by you\n\n",
                entry.name
            ));
            out.push_str(&format!("  {:<11} {ships}\n", "omh ships"));
            out.push_str(&format!(
                "  {:<11} {}   in {}\n",
                "you set", yours.value, yours.layer
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            costs(entry, &mut out);
            out.push_str(&format!("  {:<11} {}\n", "remove", entry.remove));
        }
        Verdict::Removed { entry } => {
            out.push_str(&format!(
                "{} — omh's choice, not installed here\n\n",
                entry.name
            ));
            out.push_str(&format!("  {:<11} {}\n", "because", entry.because));
            costs(entry, &mut out);
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
            Verdict::Modified { ships, yours, .. } => {
                assert_eq!(ships, "codebase-memory-mcp");
                assert_eq!(yours.value, "my-fork");
                assert_eq!(yours.layer, Layer::Local);
            }
            other => panic!("expected Modified, got {other:?}"),
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
        let out = render(&c.why("codegraph"));

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
            let out = render(&Verdict::Removed { entry });
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
        let out = render(&Verdict::Removed {
            entry: &m.entries[0],
        });
        assert!(
            out.contains("stale"),
            "an outdated measurement must say so:\n{out}"
        );
    }

    /// `init` generates `<stack>-test` and `<stack>-format` hooks from what it
    /// detected. Those are omh's writing but not omh's *opinion* — there is
    /// nothing curated to argue about, `cargo fmt` is simply what formats Rust.
    ///
    /// Calling them "your choice" is the same false claim of authorship as
    /// calling your own entry omh's, just pointing the other way, and it was the
    /// answer this command gave until running it revealed otherwise.
    #[test]
    fn a_hook_derived_from_your_stack_is_neither_omhs_opinion_nor_yours() {
        let m = manifest();
        let mut c = catalog(&m, vec![setting("rust-format", "cargo fmt", Layer::Shared)]);
        c.derived.insert(
            "rust-format".into(),
            "rust, detected from Cargo.toml".into(),
        );

        match c.why("rust-format") {
            Verdict::Derived { from, .. } => assert!(from.contains("Cargo.toml"), "{from}"),
            other => panic!("expected Derived, got {other:?}"),
        }

        let out = render(&c.why("rust-format"));
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
        let out = render(&c.why("linear"));

        assert!(out.contains("your choice"), "{out}");
        assert!(!out.contains("base set"), "claims omh installed it:\n{out}");
        assert!(!out.contains("because"), "invents a rationale:\n{out}");
        assert!(
            out.contains("local"),
            "provenance is the one thing it can say:\n{out}"
        );
    }

    #[test]
    fn a_modified_entry_shows_what_omh_ships_beside_what_you_set() {
        let m = manifest();
        let c = catalog(&m, vec![setting("codegraph", "my-fork", Layer::Local)]);
        let out = render(&c.why("codegraph"));
        assert!(out.contains("codebase-memory-mcp"), "{out}");
        assert!(out.contains("my-fork"), "{out}");
    }

    #[test]
    fn an_unknown_name_prints_the_alternatives_it_does_know() {
        let m = manifest();
        let c = catalog(&m, vec![]);
        let out = render(&c.why("lienar"));
        assert!(out.contains("codegraph"), "{out}");
        assert!(!out.contains("omh's choice"), "guessed at a match:\n{out}");
    }
}
