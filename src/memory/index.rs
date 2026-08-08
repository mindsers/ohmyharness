//! What the store contains, in one sentence.
//!
//! §9.3: the index rides in the tool description. The agent has direct access
//! to the graph, so injection only ever solved the *trigger* problem — and a
//! description arrives attached to the call rather than competing inside a
//! document that decays as context grows.
//!
//! **Counts, not titles.** That is the whole economic claim: the injected cost
//! stops growing with the graph, so a store of four hundred notes advertises
//! itself in the same breath as a store of four.

use crate::memory::Note;
use std::collections::BTreeMap;

/// How many notes, grouped by the first segment of their key.
///
/// Grouping by key namespace rather than by topic is deliberate: the namespace
/// is minted by the key templates, which live in project config, so the
/// grouping is configuration rather than a judgement omh makes at read time.
pub struct Index {
    pub total: usize,
    pub groups: Vec<(String, usize)>,
}

/// Groups past this many collapse into `other`, which is what stops the
/// sentence growing with the store. Visible rather than hidden: it is the only
/// number here, and the `other` bucket says what it swallowed.
const NAMED_GROUPS: usize = 4;

impl Index {
    pub fn of(notes: &[Note]) -> Index {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for note in notes {
            // A flat key has no namespace, so its own name would be a group of
            // one. `other` is the honest bucket for those.
            let group = note
                .key
                .split_once('/')
                .map(|(head, _)| head)
                .unwrap_or("other");
            *counts.entry(group).or_insert(0) += 1;
        }

        let mut ranked: Vec<(String, usize)> = counts
            .into_iter()
            .map(|(name, n)| (name.to_string(), n))
            .collect();
        // Count descending, then name, so the sentence is stable for a store
        // that has not changed.
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let mut groups: Vec<(String, usize)> = Vec::new();
        let mut rest = 0;
        for (name, n) in ranked {
            if groups.len() < NAMED_GROUPS && name != "other" {
                groups.push((name, n));
            } else {
                rest += n;
            }
        }
        if rest > 0 {
            groups.push(("other".to_string(), rest));
        }

        Index {
            total: notes.len(),
            groups,
        }
    }
}

/// The sentence that rides in `recall`'s description.
pub fn describe(index: &Index) -> String {
    if index.total == 0 {
        return "Search this repo's accumulated notes. The store is empty so far — \
                it fills as work turns up things that were not obvious. Ask anyway: \
                an empty answer is itself a fact about what has been learned here."
            .to_string();
    }

    let breakdown: Vec<String> = index
        .groups
        .iter()
        .map(|(name, n)| format!("{n} {name}"))
        .collect();

    format!(
        "Search this repo's accumulated notes. The store holds {} note{}: {}. \
         Most exist because an assumption turned out wrong. Query before assuming \
         how something here works.",
        index.total,
        if index.total == 1 { "" } else { "s" },
        breakdown.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, Layer};
    use std::path::PathBuf;

    fn note(layer: Layer, key: &str) -> Note {
        Note {
            key: key.to_string(),
            kind: Kind::Surprise,
            source: "session s01, claude".into(),
            recorded: "2026-08-07".into(),
            invalidated_by: None,
            body: "# t\n\n## Expected\na\n\n## Observed\nb\n\n## Evidence\nc\n".into(),
            layer,
            path: PathBuf::from(format!("{key}.md")),
        }
    }

    fn store(n: usize, namespaces: usize) -> Vec<Note> {
        (0..n)
            .map(|i| {
                let layer = if i % 2 == 0 {
                    Layer::Team
                } else {
                    Layer::Local
                };
                note(layer, &format!("ns{}/note-{i}", i % namespaces.max(1)))
            })
            .collect()
    }

    /// **The economic claim, tested rather than asserted.** Listing titles "to
    /// be helpful" makes the injected cost grow with the graph, which is the
    /// one thing §9.3 exists to prevent.
    ///
    /// Compared with digits stripped, so there is no constant to calibrate:
    /// two stores of wildly different size must produce the same sentence
    /// apart from the numbers in it.
    #[test]
    fn the_description_does_not_grow_with_the_store() {
        let shape = |s: &str| {
            s.chars()
                .filter(|c| !c.is_ascii_digit())
                .collect::<String>()
        };

        // Two stores an order of magnitude apart, both past the point where
        // the grouping saturates. Identical shape means the sentence is
        // bounded by construction rather than by how much anyone wrote.
        let big = describe(&Index::of(&store(400, 40)));
        let bigger = describe(&Index::of(&store(4_000, 400)));
        assert_eq!(
            shape(&big),
            shape(&bigger),
            "ten times the store, same sentence:\n{big}\n{bigger}"
        );

        // And the bound holds for any store, not only saturated ones: never
        // more than the named groups plus the bucket that swallows the tail.
        for (notes, namespaces) in [(3, 2), (50, 7), (4_000, 400)] {
            let index = Index::of(&store(notes, namespaces));
            assert!(
                index.groups.len() <= NAMED_GROUPS + 1,
                "{notes} notes over {namespaces} namespaces listed {} groups",
                index.groups.len()
            );
        }
    }

    /// The tail must be *reported*, not dropped — a breakdown that silently
    /// omits 360 notes says the store is smaller than it is.
    #[test]
    fn the_groups_account_for_every_note() {
        for (notes, namespaces) in [(3, 2), (50, 7), (400, 40)] {
            let index = Index::of(&store(notes, namespaces));
            let counted: usize = index.groups.iter().map(|(_, n)| n).sum();
            assert_eq!(counted, index.total, "the breakdown must add up");
        }
    }

    /// A description derived from one layer tells a repo whose store is mostly
    /// promoted that it holds two notes, and the agent stops querying.
    #[test]
    fn the_description_counts_both_layers() {
        let notes = vec![
            note(Layer::Team, "surprise/a"),
            note(Layer::Local, "surprise/b"),
        ];
        assert_eq!(Index::of(&notes).total, 2);
        assert!(describe(&Index::of(&notes)).contains('2'));
    }

    /// A flat key has no namespace, and inventing one group per note would put
    /// the whole store in the sentence — titles by another route.
    #[test]
    fn keys_with_no_namespace_collapse_rather_than_each_becoming_a_group() {
        let notes: Vec<Note> = (0..30)
            .map(|i| note(Layer::Local, &format!("bare-key-{i}")))
            .collect();
        let index = Index::of(&notes);
        assert_eq!(index.groups.len(), 1);
        assert_eq!(index.groups[0], ("other".to_string(), 30));
    }

    /// An empty store must not read as a broken tool: the agent has to be told
    /// there is nothing yet, or it concludes the server is not working.
    #[test]
    fn an_empty_store_describes_itself_as_empty_rather_than_saying_nothing() {
        let text = describe(&Index::of(&[]));
        assert!(!text.trim().is_empty());
        assert!(text.to_lowercase().contains("empty"), "{text}");
    }

    /// Same store, same sentence — or the description changes under an agent
    /// for no reason it can see.
    #[test]
    fn the_description_is_stable_for_an_unchanged_store() {
        let notes = store(20, 6);
        let mut shuffled = notes.clone();
        shuffled.reverse();
        assert_eq!(
            describe(&Index::of(&notes)),
            describe(&Index::of(&shuffled))
        );
    }

    /// The trigger is the part that cannot live anywhere else — a rules file
    /// decays as context grows, and this arrives attached to the call.
    #[test]
    fn the_description_says_why_the_notes_exist_not_only_how_many() {
        let text = describe(&Index::of(&store(5, 2))).to_lowercase();
        assert!(text.contains("wrong") || text.contains("assum"), "{text}");
    }
}
