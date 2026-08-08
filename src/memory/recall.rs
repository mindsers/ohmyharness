//! Retrieval: rank, expand one hop, return the neighbourhood.
//!
//! One call, not a walk. The multi-turn graph-browsing agent was the worst and
//! most expensive arm of the benchmark this design came from, and what
//! transfers is that **one call should return enough that a second is rarely
//! needed** — the neighbourhood, not the node. The graph is structure here,
//! never an interface the agent drives.
//!
//! Ranking has no calibrated number in it. Notes are ordered by the **rarity**
//! of the question's terms that they match — a word in every note cannot tell
//! two notes apart, so it barely counts, while a word in one note counts a lot.
//! That is what stands in for a stopword list, which would be a list to
//! maintain, one per language, stale by construction, and carrying a length
//! cutoff to calibrate at its edge. A term matched in the key counts double,
//! then recency, then how many notes point at it, then key for determinism.
//!
//! Connectedness is a **tie-break, never a weight**. As a weight it would need
//! a damping constant calibrated against a store that does not exist, and it
//! would rank a note written five minutes ago last — nothing points at a new
//! observation yet, and a new observation is what this feature exists for.
//!
//! Matching is on **whole tokens**. The first version compared substrings, and
//! it was not marginally wrong: `key.contains("a")` is true of nearly every key
//! ever written, so one stray letter in a question dragged the entire store
//! back. Measured on a ten-note store, `"a"` retrieved all ten and a real
//! question retrieved nine.
//!
//! **Layer is not a tiebreak.** Retrieval never picks a winner; reconciling a
//! contradiction is the agent's job, done with layers and dates in hand.

use crate::memory::{Layer, Note};
#[cfg(test)]
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};

/// What a note must carry to be judgeable at all: which note, how old, and
/// whether a human reviewed it.
///
/// There is no constructor that takes a key without a layer and a date. That
/// is the whole of invariant 1, expressed as a type rather than as a rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cite {
    pub key: String,
    pub layer: Layer,
    pub recorded: String,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub cite: Cite,
    /// 0 for a note the question matched, 1 for one hop out.
    ///
    /// There is no `rank` field: position in `hits` is the ranking, and a
    /// second copy of it is a second thing to keep in step.
    pub depth: u8,
}

#[derive(Debug)]
pub struct Neighbourhood {
    pub question: String,
    pub hits: Vec<Hit>,
    /// Matches that did not fit the budget. Reported, never silent: an agent
    /// that believes it saw everything stops asking.
    pub omitted: usize,
}

/// The one number here, and it is visible in the output whenever it bites.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub roots: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { roots: 8 }
    }
}

/// Whole words, lowercased.
///
/// Tokens rather than substrings, because substrings were the original defect
/// and it was not marginal: `key.contains("a")` is true of almost every key
/// ever written, so one stray letter in a question dragged the whole store
/// back. Measured before this changed, `"a"` retrieved a ten-note store in
/// full.
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The tokens of one note, split by where they appear.
struct Indexed<'a> {
    note: &'a Note,
    key: BTreeSet<String>,
    all: BTreeSet<String>,
}

fn index(notes: &[Note]) -> Vec<Indexed<'_>> {
    notes
        .iter()
        .map(|note| {
            let key = tokens(&note.key);
            let mut all = tokens(&note.body);
            all.extend(key.iter().cloned());
            Indexed { note, key, all }
        })
        .collect()
}

/// How many notes contain each term.
///
/// This is what replaces a stopword list — which would be a list to maintain,
/// one per language, stale by construction, and carrying a length cutoff to
/// calibrate at its edge. A word in every note cannot tell two notes apart,
/// and rarity says so without anybody deciding which words those are.
fn document_frequency<'a>(indexed: &'a [Indexed<'_>]) -> BTreeMap<&'a str, usize> {
    let mut df: BTreeMap<&str, usize> = BTreeMap::new();
    for doc in indexed {
        for term in &doc.all {
            *df.entry(term.as_str()).or_insert(0) += 1;
        }
    }
    df
}

/// Rarity of a matched term, weighted by where it matched.
///
/// Smoothed so that rarity **orders but never excludes**: with a bare
/// `ln(n/df)` a term in every note scores zero, and a store where every note
/// is about the same subject becomes unsearchable for that subject — exactly
/// as it gets good. `ln((n+1)/df)` stays positive, so a universal term still
/// retrieves; it simply cannot rank.
///
/// Returned in thousandths as an integer: the ordering has to be total and
/// reproducible, and floats give neither for free.
fn score(doc: &Indexed<'_>, terms: &BTreeSet<String>, df: &BTreeMap<&str, usize>, n: usize) -> u64 {
    let mut total = 0.0_f64;
    for term in terms {
        if !doc.all.contains(term) {
            continue;
        }
        let Some(seen) = df.get(term.as_str()) else {
            continue;
        };
        let rarity = ((n as f64 + 1.0) / *seen as f64).ln();
        // A term in the key is a term about the note's subject rather than one
        // that merely occurs in it. A weight, not a threshold: it orders, and
        // nothing is compared against it.
        let where_it_matched = if doc.key.contains(term) { 2.0 } else { 1.0 };
        total += rarity * where_it_matched;
    }
    (total * 1000.0) as u64
}

/// How many notes point at each key, from either layer.
///
/// A tie-break, never a weight. Alphabetical order carries no information about
/// which of two equally-rare, equally-recent notes answers the question; what
/// the rest of the store points at does.
fn inbound_counts(notes: &[Note]) -> BTreeMap<&str, usize> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for note in notes {
        for target in crate::memory::links(&note.body) {
            // Keyed by the note that exists, so a dangling link inflates
            // nothing: the lint reports those, the ranking ignores them.
            if let Some(existing) = notes.iter().find(|n| n.key == target) {
                *counts.entry(existing.key.as_str()).or_insert(0) += 1;
            }
        }
    }
    counts
}

fn cite(note: &Note) -> Cite {
    Cite {
        key: note.key.clone(),
        layer: note.layer,
        recorded: note.recorded.clone(),
    }
}

pub fn search(notes: &[Note], question: &str, budget: Budget) -> Neighbourhood {
    let terms = tokens(question);
    let indexed = index(notes);
    let df = document_frequency(&indexed);

    let mut scored: Vec<(u64, &Note)> = indexed
        .iter()
        .map(|doc| (score(doc, &terms, &df, notes.len()), doc.note))
        .filter(|(s, _)| *s > 0)
        .collect();

    // How many notes point at each one. Used only to break a tie rarity and
    // recency could not — never as a weight, because a weight needs a constant
    // calibrated against a store that does not exist, and because centrality
    // ranks a note written five minutes ago last. Nothing links to a new
    // observation yet, and a new observation is what this feature is for.
    let inbound = inbound_counts(notes);

    // Recency, then connectedness, then key. Layer is deliberately absent:
    // sorting the committed layer above the local one would present a
    // reconciliation the ranking is not entitled to make.
    scored.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then(b.recorded.cmp(&a.recorded))
            .then(
                inbound
                    .get(b.key.as_str())
                    .unwrap_or(&0)
                    .cmp(inbound.get(a.key.as_str()).unwrap_or(&0)),
            )
            .then(a.key.cmp(&b.key))
            .then(a.layer.cmp(&b.layer))
    });

    let omitted = scored.len().saturating_sub(budget.roots);
    let roots: Vec<&Note> = scored
        .into_iter()
        .take(budget.roots)
        .map(|(_, n)| n)
        .collect();

    // Identity is (layer, key) throughout. Deduping by key alone would drop
    // one of two notes that disagree, which is the one thing retrieval must
    // never do.
    let by_identity: BTreeMap<(Layer, &str), &Note> = notes
        .iter()
        .map(|n| ((n.layer, n.key.as_str()), n))
        .collect();

    let mut seen: BTreeSet<(Layer, String)> = BTreeSet::new();
    let mut hits: Vec<Hit> = Vec::new();

    for note in &roots {
        if !seen.insert((note.layer, note.key.clone())) {
            continue;
        }
        hits.push(Hit {
            cite: cite(note),
            depth: 0,
        });
    }

    // One hop, and only one. A visited set rather than a depth guard alone:
    // without it a note reachable from two roots is listed twice and a cycle
    // never terminates.
    for note in &roots {
        for target in crate::memory::links(&note.body) {
            // A link resolves into either layer — §4 says both retrieve — so a
            // neighbour may appear in both, and each keeps its own provenance.
            for layer in Layer::ALL {
                let Some(child) = by_identity.get(&(layer, target.as_str())) else {
                    continue;
                };
                if !seen.insert((child.layer, child.key.clone())) {
                    continue;
                }
                hits.push(Hit {
                    cite: cite(child),
                    depth: 1,
                });
            }
        }
    }

    Neighbourhood {
        question: question.to_string(),
        hits,
        omitted,
    }
}

/// The separator between a note and its provenance. One glyph, so a key
/// containing spaces cannot be mistaken for a layer.
const SEP: &str = " · ";

/// The only place a note becomes text an agent reads.
pub fn render(n: &Neighbourhood) -> String {
    if n.hits.is_empty() {
        return format!(
            "No notes about `{}`. The store holds nothing on this yet — \
             which is a fact about the store, not about the repo.\n",
            n.question
        );
    }

    let label_of = |i: usize, hit: &Hit| {
        let last = n.hits.get(i + 1).is_none_or(|next| next.depth == 0);
        let prefix = match (hit.depth, last) {
            (0, _) => "",
            (_, false) => "├─ ",
            (_, true) => "└─ ",
        };
        format!("{prefix}{}", hit.cite.key)
    };

    // Width comes from the rendered label — tree prefix included — plus a gap
    // that cannot close. Deriving it from the bare key let a long child's key
    // run straight into its layer (`…-returns-ebusylocal · 2026-08-08`), which
    // is invariant 1 broken in the only text an agent ever reads. Counted in
    // characters, not bytes, because the tree glyphs are multi-byte and
    // `{:width$}` pads by character.
    let width = n
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| label_of(i, h).chars().count())
        .max()
        .unwrap_or(0)
        + 2;

    let mut out = String::new();
    for (i, hit) in n.hits.iter().enumerate() {
        let label = label_of(i, hit);
        out.push_str(&format!(
            "{label:width$}{}{SEP}{}\n",
            hit.cite.layer, hit.cite.recorded
        ));
    }

    if n.omitted > 0 {
        out.push_str(&format!(
            "\n… {} more match{}; ask a narrower question.\n",
            n.omitted,
            if n.omitted == 1 { "" } else { "es" }
        ));
    }

    out
}

/// Parse the rendered tree back into the citations it claims to carry.
///
/// Test-only, and load-bearing: this is the half of invariant 1 a `contains`
/// assertion cannot fake. **Strict and total** — any non-empty line it cannot
/// fully decompose is an error, so a dropped layer becomes a failure rather
/// than a line that quietly does not match.
#[cfg(test)]
pub fn parse_rendered(rendered: &str) -> Result<Vec<Cite>> {
    let mut out = Vec::new();

    for line in rendered.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        // The omitted-matches marker is prose about the answer, not a note.
        if line.trim_start().starts_with('…') {
            continue;
        }

        let stripped =
            line.trim_start_matches(|c| c == '├' || c == '└' || c == '─' || c == '│' || c == ' ');

        let Some((key, rest)) = stripped.split_once("  ") else {
            bail!("`{line}` carries no provenance at all");
        };
        let Some((layer, recorded)) = rest.trim().split_once(SEP) else {
            bail!("`{line}` is missing its layer or its date");
        };
        let layer: Layer = layer
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("`{line}` names no layer omh knows"))?;
        let recorded = recorded.trim();
        if !crate::memory::is_calendar_date(recorded) {
            bail!("`{line}` carries `{recorded}`, which is not a date");
        }
        out.push(Cite {
            key: key.trim().to_string(),
            layer,
            recorded: recorded.to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, Layer, Note};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn note(layer: Layer, key: &str, recorded: &str, links: &[&str]) -> Note {
        // No single-letter filler: those are whole tokens, and they made every
        // note match every question by accident.
        let mut body = format!(
            "# {key}\n\n## Expected\nsomething predictable\n\n## Observed\nsomething else\n\n## Evidence\nthe recorded output\n"
        );
        if !links.is_empty() {
            body.push_str("\n## Related\n\n");
            for target in links {
                body.push_str(&format!("- [[{target}]]\n"));
            }
        }
        Note {
            key: key.to_string(),
            kind: Kind::Surprise,
            source: "session s01, claude".into(),
            recorded: recorded.to_string(),
            invalidated_by: None,
            body,
            layer,
            path: PathBuf::from(format!("{key}.md")),
        }
    }

    /// The fixture *is* part of the invariant-1 guard, so each property below
    /// is deliberate:
    ///
    /// - **pairwise distinct dates**, so a renderer printing a constant, or
    ///   today, or the parent's date, cannot pass;
    /// - **both layers among the roots and among the children**, with a `team`
    ///   parent whose child is `local`, so an inherited layer cannot pass;
    /// - **one key present in both layers**, so key-only dedup cannot pass;
    /// - **one note reachable from two roots**, so a duplicate-emitting
    ///   expansion cannot pass.
    fn store() -> Vec<Note> {
        vec![
            note(
                Layer::Team,
                "credentials-are-a-named-volume",
                "2026-06-12",
                &[
                    "credentials-file-mount-returns-ebusy",
                    "accounts-are-single-path-components",
                ],
            ),
            // Same key, other layer, contradicting — and a different date.
            note(
                Layer::Local,
                "credentials-are-a-named-volume",
                "2026-07-02",
                &[],
            ),
            // A local child of a team parent.
            note(
                Layer::Local,
                "credentials-file-mount-returns-ebusy",
                "2026-08-07",
                &[],
            ),
            note(
                Layer::Team,
                "accounts-are-single-path-components",
                "2026-06-14",
                &[],
            ),
            // A second root reaching the same child.
            note(
                Layer::Team,
                "credentials-the-image-ends-unprivileged",
                "2026-07-20",
                &["credentials-file-mount-returns-ebusy"],
            ),
        ]
    }

    /// **Invariant 1 — a note is never retrieved without its date and layer.**
    ///
    /// Asserted as a bijection, not a containment. `rendered.contains("team")`
    /// passes while every line carries the same date, while a child inherits
    /// its parent's layer, and while one of two same-key notes has silently
    /// vanished. This repo has shipped exactly that shape of guard twice.
    ///
    /// The rendered text is parsed back **strictly and totally** — any line
    /// the parser cannot fully decompose into key, layer and date is an error,
    /// so a dropped field becomes a failure rather than a line that quietly
    /// does not match. Then the set of what came back must equal the set of
    /// what the search selected, and the line count must equal it too.
    ///
    /// Mutations this must go red against, in order:
    ///   1. drop the layer from the line format   → a line fails to parse
    ///   2. (the parent-layer mutation is NOT caught here — a hit built with the
    ///      wrong layer renders faithfully wrong. That one belongs to
    ///      `an_expanded_neighbour_carries_its_own_layer_not_its_parents`.)
    ///   3. print today() instead of `recorded`   → the dates mismatch
    ///   4. dedup by key, dropping the local twin → the sets differ in size
    #[test]
    fn no_note_is_retrieved_without_its_date_and_layer() {
        let notes = store();
        let found = search(&notes, "credentials", Budget::default());
        assert!(!found.hits.is_empty(), "the fixture must match something");

        let rendered = render(&found);
        let cites = parse_rendered(&rendered).unwrap_or_else(|e| {
            panic!("every rendered line must carry provenance: {e}\n{rendered}")
        });

        assert_eq!(
            cites.len(),
            rendered.lines().filter(|l| !l.trim().is_empty()).count(),
            "a line that did not parse is a line missing its provenance:\n{rendered}"
        );

        let got: BTreeSet<Cite> = cites.into_iter().collect();
        let want: BTreeSet<Cite> = found.hits.iter().map(|h| h.cite.clone()).collect();
        assert_eq!(
            got, want,
            "each note carries its own layer and its own date"
        );
    }

    /// §4: `team/deploy` and `local/deploy` are different notes and both
    /// retrieve. Dedup by `Key` instead of `(Layer, Key)` hides a teammate's
    /// note behind yours, and the reader never learns it existed.
    #[test]
    fn a_note_and_its_local_counterpart_both_retrieve() {
        let found = search(
            &store(),
            "credentials-are-a-named-volume",
            Budget::default(),
        );
        let both: Vec<&Hit> = found
            .hits
            .iter()
            .filter(|h| h.cite.key == "credentials-are-a-named-volume")
            .collect();

        assert_eq!(both.len(), 2, "one key in two layers is two notes");
        let layers: BTreeSet<Layer> = both.iter().map(|h| h.cite.layer).collect();
        assert_eq!(layers.len(), 2, "and they are not the same layer");
    }

    /// Printing the root's layer down the tree makes a local child of a team
    /// parent read as reviewed — which is the exact laundering invariant 1
    /// exists to stop.
    #[test]
    fn an_expanded_neighbour_carries_its_own_layer_not_its_parents() {
        // The question is chosen so these neighbours are reachable *only* by
        // expansion. An earlier version asked something the neighbours matched
        // directly, so they arrived as roots, the child path never ran, and
        // inheriting the parent's layer left the whole suite green. Asserting
        // `depth == 1` is what stops that recurring.
        let found = search(&store(), "named volume", Budget::default());

        let parent = found
            .hits
            .iter()
            .find(|h| h.depth == 0 && h.cite.layer == Layer::Team)
            .expect("a team note must match the question directly");
        let child = found
            .hits
            .iter()
            .find(|h| h.cite.key == "credentials-file-mount-returns-ebusy")
            .expect("the local neighbour must be expanded");

        assert_eq!(child.depth, 1, "it arrives by expansion, not by matching");
        assert_eq!(
            child.cite.layer,
            Layer::Local,
            "a local child of a team parent must not read as reviewed"
        );
        assert_ne!(child.cite.layer, parent.cite.layer, "inheritance, caught");
        assert_eq!(
            child.cite.recorded, "2026-08-07",
            "and it keeps its own date"
        );
        assert_ne!(child.cite.recorded, parent.cite.recorded);

        // The other neighbour really is `team`, so hardcoding `local` to
        // satisfy the assertion above fails here instead.
        let sibling = found
            .hits
            .iter()
            .find(|h| h.cite.key == "accounts-are-single-path-components")
            .expect("the team neighbour must be expanded too");
        assert_eq!(sibling.depth, 1);
        assert_eq!(sibling.cite.layer, Layer::Team);
    }

    /// Expansion without a visited set renders a note once per path that
    /// reaches it, and a two-note cycle renders forever.
    #[test]
    fn a_note_reachable_from_two_roots_is_listed_once() {
        let found = search(&store(), "credentials", Budget::default());
        let seen: Vec<&Hit> = found
            .hits
            .iter()
            .filter(|h| h.cite.key == "credentials-file-mount-returns-ebusy")
            .collect();
        assert_eq!(seen.len(), 1, "reached twice, listed once: {seen:?}");
    }

    /// Found by running the server, not by the suite: a child whose key is the
    /// longest in the neighbourhood got zero padding, because the column width
    /// was computed from bare key lengths while the label also carries a tree
    /// prefix. The rendered line read
    /// `surprise/…-returns-ebusylocal · 2026-08-08` — key and layer fused,
    /// which is invariant 1 broken in the output an agent actually reads.
    ///
    /// The fixture in `store()` could never catch it: its keys were similar
    /// enough that the padding happened to work. So this one is built the
    /// other way round — the **child** holds the longest key.
    #[test]
    fn a_long_key_never_runs_into_its_provenance() {
        let notes = vec![
            note(
                Layer::Team,
                "short",
                "2026-01-01",
                &["a-very-much-longer-key-than-its-parent"],
            ),
            note(
                Layer::Local,
                "a-very-much-longer-key-than-its-parent",
                "2026-02-02",
                &[],
            ),
        ];
        let rendered = render(&search(&notes, "short", Budget::default()));

        parse_rendered(&rendered)
            .unwrap_or_else(|e| panic!("provenance must stay separable: {e}\n{rendered}"));

        for line in rendered.lines().filter(|l| !l.trim().is_empty()) {
            assert!(line.contains("  "), "key and layer must not fuse: {line:?}");
        }
    }

    #[test]
    fn a_cycle_terminates() {
        let notes = vec![
            note(Layer::Local, "a", "2026-01-01", &["b"]),
            note(Layer::Local, "b", "2026-01-02", &["a"]),
        ];
        let found = search(&notes, "a", Budget::default());
        assert_eq!(found.hits.len(), 2);
    }

    /// §9.2: retrieval never picks a winner. Contradicting notes both return,
    /// and the agent reconciles with layers and dates in hand — which is what
    /// an LLM is good at and an indexer is not.
    #[test]
    fn retrieval_never_picks_a_winner_between_contradicting_notes() {
        let found = search(
            &store(),
            "credentials-are-a-named-volume",
            Budget::default(),
        );
        let both: Vec<&Hit> = found
            .hits
            .iter()
            .filter(|h| h.cite.key == "credentials-are-a-named-volume")
            .collect();
        assert_eq!(both.len(), 2, "both claims come back; neither is filtered");
    }

    /// §9.2 says layer outranks recency **when the agent reconciles** — which
    /// is precisely why the ranking must not do it first. Sorting the
    /// committed layer above the local one presents a reconciliation the
    /// indexer is not entitled to make, and hides that the local note is the
    /// newer claim.
    ///
    /// Asserted in both directions, because a one-directional check passes on
    /// an implementation that always sorts `team` first.
    #[test]
    fn order_follows_recency_not_layer() {
        let key = "deploy";
        let newer_is_local = vec![
            note(Layer::Team, key, "2026-01-01", &[]),
            note(Layer::Local, key, "2026-09-09", &[]),
        ];
        let newer_is_team = vec![
            note(Layer::Team, key, "2026-09-09", &[]),
            note(Layer::Local, key, "2026-01-01", &[]),
        ];

        for notes in [newer_is_local, newer_is_team] {
            let found = search(&notes, key, Budget::default());
            assert_eq!(
                found.hits[0].cite.recorded, "2026-09-09",
                "the newer claim leads, whichever layer it is in"
            );
        }
    }

    /// Truncating with no marker tells the agent it has seen everything.
    #[test]
    fn no_result_is_silently_dropped() {
        let notes = store();
        let found = search(&notes, "credentials", Budget { roots: 1 });

        assert!(found.omitted > 0, "the fixture must exceed a budget of one");
        assert!(
            render(&found).contains(&found.omitted.to_string()),
            "say how many were left out:\n{}",
            render(&found)
        );
    }

    /// An empty content block reads as a failed tool call, and an agent that
    /// believes the tool is broken stops calling it.
    #[test]
    fn an_empty_store_says_so_instead_of_looking_broken() {
        let found = search(&[], "anything", Budget::default());
        let rendered = render(&found);
        assert!(!rendered.trim().is_empty(), "silence is not an answer");
        assert!(
            parse_rendered(&rendered).is_err() || parse_rendered(&rendered).unwrap().is_empty(),
            "and it is prose, not a note nobody wrote"
        );
    }

    #[test]
    fn a_question_matching_nothing_says_so_rather_than_returning_the_store() {
        let found = search(&store(), "kubernetes", Budget::default());
        assert!(found.hits.is_empty(), "no match is not every note");
        assert!(!render(&found).trim().is_empty());
    }

    // ── ranking ─────────────────────────────────────────────────────────────

    /// Substring matching was the original defect, and it was not subtle:
    /// `key.contains("a")` is true of almost every key ever written, so a
    /// single stray letter in a question dragged the whole store back.
    ///
    /// Measured before this changed: asking `"a"` of a ten-note store returned
    /// all ten, and `"why does a credential mount fail"` returned nine.
    #[test]
    fn a_question_matches_whole_tokens_not_fragments_of_them() {
        // `named-volume-mounts` contains the letter `a` only *inside* words.
        // `a-thing` carries it as a word of its own.
        let notes = vec![
            note(Layer::Local, "named-volume-mounts", "2026-01-01", &[]),
            note(Layer::Local, "a-thing", "2026-01-02", &[]),
        ];

        let hits = search(&notes, "a", Budget::default()).hits;
        let keys: Vec<&str> = hits.iter().map(|h| h.cite.key.as_str()).collect();
        assert!(
            keys.contains(&"a-thing"),
            "`a` is a whole token there: {keys:?}"
        );
        assert!(
            !keys.contains(&"named-volume-mounts"),
            "`a` inside `named` is not a match: {keys:?}"
        );
    }

    /// A word in every note cannot tell two notes apart, so it must not decide
    /// the order. Rarity does that without a stopword list — which would be a
    /// list to maintain, per language, stale by construction, and a number to
    /// calibrate at the edge of it.
    ///
    /// The discriminating note is made the **oldest** deliberately: if rarity
    /// is not doing the work, recency puts it last and this fails.
    #[test]
    fn a_word_in_every_note_cannot_decide_the_ranking() {
        // The fixture has to separate *weighting by rarity* from *counting
        // matched terms*, and the first version did not: its answer also
        // matched more terms, so plain counting produced the same order and
        // deleting rarity outright left the suite green.
        //
        // So the note that should win matches **one rare** term while its
        // rivals match **two common** ones. Counting puts a rival first; only
        // rarity puts the answer first. It is also the oldest, so recency
        // cannot explain the win either.
        let mut notes: Vec<Note> = (0..9)
            .map(|i| {
                note(
                    Layer::Local,
                    &format!("the-note-about-things-{i}"),
                    &format!("2026-02-0{}", i + 1),
                    &[],
                )
            })
            .collect();
        notes.push(note(
            Layer::Local,
            "ebusy-on-file-mounts",
            "2026-01-01",
            &[],
        ));

        let hits = search(&notes, "the note ebusy", Budget::default()).hits;
        assert_eq!(
            hits[0].cite.key,
            "ebusy-on-file-mounts",
            "one rare term beats two common ones, and beats recency: {:?}",
            hits.iter().map(|h| &h.cite.key).collect::<Vec<_>>()
        );
    }

    /// The question from the probe that exposed the problem, kept as a
    /// regression: the note that actually answers it must lead.
    #[test]
    fn a_real_question_puts_the_note_that_answers_it_first() {
        let notes = vec![
            note(
                Layer::Local,
                "mounting-a-credential-file-returns-ebusy",
                "2026-01-01",
                &[],
            ),
            note(
                Layer::Local,
                "the-graph-cache-is-keyed-by-repo",
                "2026-06-01",
                &[],
            ),
            note(
                Layer::Local,
                "a-latest-tag-skips-the-rebuild",
                "2026-06-02",
                &[],
            ),
            note(
                Layer::Local,
                "the-image-ends-unprivileged",
                "2026-06-03",
                &[],
            ),
        ];

        let hits = search(
            &notes,
            "why does a credential mount fail",
            Budget::default(),
        )
        .hits;
        assert_eq!(hits[0].cite.key, "mounting-a-credential-file-returns-ebusy");
        assert!(
            !hits
                .iter()
                .any(|h| h.cite.key == "the-graph-cache-is-keyed-by-repo"),
            "a note sharing only filler words is not a match: {:?}",
            hits.iter().map(|h| &h.cite.key).collect::<Vec<_>>()
        );
    }

    /// A term nothing in the store contains must not quietly become a match
    /// for everything, and must not crash the weighting either.
    #[test]
    fn a_term_the_store_has_never_seen_matches_nothing() {
        let notes = vec![note(Layer::Local, "credentials", "2026-01-01", &[])];
        assert!(search(&notes, "kubernetes", Budget::default())
            .hits
            .is_empty());
        // …and it does not poison a question that also carries a real term.
        let hits = search(&notes, "kubernetes credentials", Budget::default()).hits;
        assert_eq!(hits.len(), 1);
    }

    /// Rarity orders; it never excludes. In a store where every note is about
    /// the same thing, asking about that thing must still answer — otherwise a
    /// focused store becomes unsearchable exactly as it gets good.
    #[test]
    fn a_store_where_every_note_shares_a_term_is_still_searchable() {
        let notes = vec![
            note(
                Layer::Local,
                "credentials-are-a-named-volume",
                "2026-01-01",
                &[],
            ),
            note(
                Layer::Local,
                "credentials-refresh-in-place",
                "2026-01-02",
                &[],
            ),
        ];
        assert_eq!(
            search(&notes, "credentials", Budget::default()).hits.len(),
            2,
            "a universal term still retrieves; it just cannot rank"
        );
    }

    /// When rarity and recency both tie, the note the rest of the store points
    /// at is the better answer — and connectedness is the only signal left that
    /// carries information. Alphabetical order carries none.
    ///
    /// Asserted in both directions: the linked note must *overtake* one that
    /// alphabetical order would otherwise put first, and with the link removed
    /// the original order must come back. Without the second half this passes
    /// on any implementation that happens to sort the other way.
    #[test]
    fn a_note_the_store_points_at_wins_a_tie() {
        let tied = || {
            vec![
                note(Layer::Local, "alpha-topic", "2026-01-01", &[]),
                note(Layer::Local, "beta-topic", "2026-01-01", &[]),
            ]
        };
        let position = |notes: &[Note], key: &str| {
            search(notes, "topic", Budget::default())
                .hits
                .iter()
                .position(|h| h.cite.key == key)
                .unwrap_or_else(|| panic!("`{key}` must be retrieved"))
        };

        // Nothing links to either: the tie falls through to the key, so the
        // alphabetically first one leads.
        let alone = tied();
        assert!(position(&alone, "alpha-topic") < position(&alone, "beta-topic"));

        // Now one of them is pointed at, and it overtakes.
        let mut linked = tied();
        linked.push(note(
            Layer::Local,
            "referrer",
            "2026-01-01",
            &["beta-topic"],
        ));
        assert!(
            position(&linked, "beta-topic") < position(&linked, "alpha-topic"),
            "the note the store points at leads once the tie is broken"
        );
    }

    /// Same store, same question, same answer — or a store that grew an
    /// unrelated note reshuffles what the agent reads.
    #[test]
    fn ranking_is_deterministic() {
        let notes = store();
        let once = render(&search(&notes, "credentials", Budget::default()));
        let mut shuffled = notes.clone();
        shuffled.reverse();
        let twice = render(&search(&shuffled, "credentials", Budget::default()));
        assert_eq!(once, twice);
    }

    /// The strict parser is a guard, so it has to actually be strict — a
    /// lenient one would make invariant 1's bijection vacuous.
    #[test]
    fn the_rendered_form_parser_rejects_a_line_missing_its_provenance() {
        assert!(parse_rendered("just-a-key\n").is_err());
        assert!(parse_rendered("a-key  team\n").is_err());
        assert!(parse_rendered("a-key  team · not-a-date\n").is_err());
        assert!(parse_rendered("a-key  sideways · 2026-08-07\n").is_err());
        assert!(parse_rendered("a-key  team · 2026-08-07\n").is_ok());
    }
}
