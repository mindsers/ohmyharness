//! What the repo already documents, as notes that point rather than restate.
//!
//! §10: `docs/` and `detect::seeds()` become **stubs** — one note per
//! document, a link, and the questions it answers. Deliberately not summaries:
//! curation summarises away the verbatim detail coding work needs — the flag,
//! the path, the error string — and a distilled copy drifts from `docs/`
//! undetectably.
//!
//! This is the **floor**, not the point. It makes the store useful on day one;
//! agent writing is the growth path, and the growth path is where the feature
//! is irreplaceable — there is no `grep` for a session that has been removed.

use crate::memory::{Kind, Layer, Note};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A repo document worth pointing at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Repo-relative, forward slashes.
    pub path: String,
    pub title: String,
    /// Its `##` headings, verbatim and in order. Derivable with no model, and
    /// honest — nothing is summarised, so nothing can drift.
    pub answers: Vec<String>,
}

/// Markdown files git tracks.
///
/// "Tracked" is the calibration-free rule. It excludes vendored trees, build
/// output and `node_modules` without a list of exclusions that goes stale, and
/// it defers to the authority this repo already shells out to everywhere else.
pub fn documents(repo: &Path) -> Result<Vec<Document>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-z", "--", "*.md"])
        .output()
        .with_context(|| format!("listing tracked markdown in {}", repo.display()))?;

    let mut docs: Vec<Document> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        // The store is not a document. Ingesting it produces one stub per
        // note, every run, for ever.
        .filter(|p| !p.starts_with(".omh/"))
        .filter_map(|path| read(repo, path))
        .collect();
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(docs)
}

fn read(repo: &Path, path: &str) -> Option<Document> {
    let body = std::fs::read_to_string(repo.join(path)).ok()?;
    let title = body
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .unwrap_or_else(|| path.to_string());
    let answers = body
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .map(|h| h.trim().to_string())
        .collect();
    Some(Document {
        path: path.to_string(),
        title,
        answers,
    })
}

/// `{{path}}` for a document: its repo-relative path, extension dropped, every
/// segment canonicalised.
///
/// The shipped template is `{{path}}` rather than §6's `docs/{{path}}`,
/// because the path already carries its directory — prefixing `docs/` would
/// key `docs/design/memory.md` as `docs/docs/design/memory`. Canonicalising
/// per segment is what makes `docs/README.md` and `docs/readme.md` one key on
/// a case-insensitive filesystem, which is invariant 6 at the ingest boundary.
pub fn path_key(path: &str) -> Result<String> {
    let trimmed = path.strip_suffix(".md").unwrap_or(path);
    let parts: Result<Vec<String>> = trimmed.split('/').map(crate::memory::slug).collect();
    Ok(parts?.join("/"))
}

pub fn stub(doc: &Document, templates: &BTreeMap<Kind, String>, today: &str) -> Result<Note> {
    let template = templates
        .get(&Kind::Stub)
        .context("no key template for `stub`")?;
    let key = crate::memory::expand_key(template, &[("path", &path_key(&doc.path)?)])?;

    let mut body = format!(
        "# {}\n\nRepo document. Read `{}`; this note points at it and does not \
         restate it.\n\n## Answers\n\n",
        doc.title, doc.path
    );
    if doc.answers.is_empty() {
        // A section is required, and an empty one is refused — so say the true
        // thing rather than emitting a heading with nothing under it.
        body.push_str(&format!("- the whole of `{}`\n", doc.path));
    }
    for question in &doc.answers {
        body.push_str(&format!("- {question}\n"));
    }

    Ok(Note {
        key,
        kind: Kind::Stub,
        source: doc.path.clone(),
        recorded: today.to_string(),
        invalidated_by: None,
        body,
        layer: Layer::Team,
        path: PathBuf::new(),
    })
}

/// The facts `omh init` derives about the repo, as one note.
///
/// One topic, richly filled — §3's definition — rather than one note per fact.
/// This is where `detect::seeds()` finally lands: it has been deriving these
/// and printing them to a terminal nobody reads twice.
pub fn overview(
    repo_name: &str,
    seeds: &[crate::detect::Seed],
    stubs: &[String],
    templates: &BTreeMap<Kind, String>,
    today: &str,
) -> Result<Option<Note>> {
    if seeds.is_empty() && stubs.is_empty() {
        // Nothing derived is not an empty note. A note asserting nothing is
        // worse than no note: it retrieves, and it answers with silence.
        return Ok(None);
    }
    let template = templates
        .get(&Kind::Topic)
        .context("no key template for `topic`")?;
    let key = crate::memory::expand_key(template, &[("slug", &crate::memory::slug(repo_name)?)])?;

    let mut body = format!(
        "# {repo_name}\n\nWhat `omh init` derived about this repo, each line with the \
         file it came from.\n\n## Related\n\n"
    );
    for seed in seeds {
        // The source is not decoration: every derived fact has to be traceable
        // to the file that produced it, or it is indistinguishable from a guess.
        body.push_str(&format!("- {} — from `{}`\n", seed.fact, seed.source));
    }
    // And a link to every stub, which is what stops a freshly-ingested store
    // being *entirely* orphans. Without this the lint fires on 100% of a new
    // store, and a check that flags everything trains people to ignore it —
    // the same failure as one that flags nothing.
    for key in stubs {
        body.push_str(&format!("- [[{key}]]\n"));
    }

    Ok(Some(Note {
        key,
        kind: Kind::Topic,
        source: "omh init".into(),
        recorded: today.to_string(),
        invalidated_by: None,
        body,
        layer: Layer::Team,
        path: PathBuf::new(),
    }))
}

/// Write a note into a layer's directory, deriving its filename from its key.
pub fn write(dir: &Path, note: &Note, if_exists: crate::memory::IfExists) -> Result<bool> {
    let path = dir.join(format!("{}.md", note.key));
    if path.exists() && matches!(if_exists, crate::memory::IfExists::Skip) {
        return Ok(false);
    }
    let stamped = Note {
        path: path.clone(),
        ..note.clone()
    };
    let rendered = crate::memory::render(&stamped);
    // Validated against the same schema that refuses an agent's write. A
    // generated note omh would refuse is a bug in ingestion, not an exception
    // ingestion gets to make for itself.
    let parsed = crate::memory::parse(&rendered, stamped.layer, &path)?;
    if let Some(first) = crate::memory::check(&parsed).first() {
        anyhow::bail!("`{}` would be refused: {}", note.key, first.detail);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, rendered).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{check, shipped_templates};

    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        for (name, body) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        git(&["add", "-A"]);
        (dir, root)
    }

    const DOC: &str =
        "# Memory\n\nSome prose about the design.\n\n## Storage\n\nmore\n\n## Identity\n\nmore\n";

    /// §10 chose stubs over summaries for a reason: a summary drifts from the
    /// document undetectably, and it summarises away exactly the verbatim
    /// detail — the flag, the path, the error string — that coding work needs.
    #[test]
    fn a_stub_points_at_its_document_rather_than_restating_it() {
        let doc = Document {
            path: "docs/design/memory.md".into(),
            title: "Memory".into(),
            answers: vec!["Storage".into(), "Identity".into()],
        };
        let note = stub(&doc, &shipped_templates(), "2026-08-08").unwrap();

        assert!(note.body.contains("docs/design/memory.md"), "{}", note.body);
        assert!(
            !note.body.contains("Some prose"),
            "a stub carries no prose of its own"
        );
        assert_eq!(
            note.source, "docs/design/memory.md",
            "provenance is the doc"
        );
    }

    /// Verbatim and in order, because that is the text a question-shaped query
    /// actually matches on — and because deriving it needs no model, so it
    /// cannot invent a heading the document does not have.
    #[test]
    fn the_questions_a_stub_answers_are_the_documents_own_headings() {
        let (_d, root) = repo(&[("docs/memory.md", DOC)]);
        let docs = documents(&root).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].answers, ["Storage", "Identity"]);
        assert_eq!(docs[0].title, "Memory");

        let note = stub(&docs[0], &shipped_templates(), "2026-08-08").unwrap();
        let listed: Vec<&str> = note
            .body
            .lines()
            .filter_map(|l| l.strip_prefix("- "))
            .collect();
        assert_eq!(listed, ["Storage", "Identity"], "verbatim, and in order");
    }

    /// A generated note the schema refuses means ingestion half-populates the
    /// store while `init` reports success.
    #[test]
    fn every_generated_stub_passes_the_schema() {
        let (_d, root) = repo(&[
            ("docs/memory.md", DOC),
            // No headings at all: the `Answers` section still has to be filled,
            // because an empty required section is refused.
            ("docs/bare.md", "# Bare\n\njust prose\n"),
            // No title either.
            ("docs/untitled.md", "prose with no heading\n"),
        ]);
        let templates = shipped_templates();
        let docs = documents(&root).unwrap();
        assert_eq!(docs.len(), 3);
        for doc in &docs {
            let note = stub(doc, &templates, "2026-08-08").unwrap();
            assert_eq!(
                check(&note),
                vec![],
                "`{}` produced a refused note",
                doc.path
            );
        }
    }

    /// Invariant 6 at the ingest boundary. `docs/README.md` and
    /// `docs/readme.md` are the same document on a case-insensitive
    /// filesystem, and two keys for one document is the same failure as two
    /// spellings of one observation.
    #[test]
    fn two_spellings_of_one_document_derive_one_key() {
        assert_eq!(
            path_key("docs/README.md").unwrap(),
            path_key("docs/readme.md").unwrap()
        );
        assert_eq!(
            path_key("docs/Design/Memory.md").unwrap(),
            "docs/design/memory"
        );
    }

    /// The path already carries its directory. §6's `docs/{{path}}` would key
    /// `docs/design/memory.md` as `docs/docs/design/memory`.
    #[test]
    fn a_stubs_key_names_the_document_once() {
        let doc = Document {
            path: "docs/design/memory.md".into(),
            title: "Memory".into(),
            answers: vec!["Storage".into()],
        };
        let note = stub(&doc, &shipped_templates(), "2026-08-08").unwrap();
        assert_eq!(note.key, "docs/design/memory");
    }

    /// The store ingests itself otherwise — one stub per note, every run.
    #[test]
    fn the_note_store_is_never_ingested_as_a_document() {
        let (_d, root) = repo(&[
            ("docs/memory.md", DOC),
            (
                ".omh/notes/a-note.md",
                "---\nkey: a-note\n---\n\n# A note\n",
            ),
        ]);
        let paths: Vec<String> = documents(&root)
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert_eq!(paths, ["docs/memory.md"], "the store is not a document");
    }

    /// The seeds must survive into the note, with their sources. `detect`
    /// already requires every seed to cite where it came from, and ingestion
    /// is not where that gets dropped.
    #[test]
    fn every_derived_fact_reaches_the_note_with_the_file_it_came_from() {
        let seeds = vec![
            crate::detect::Seed {
                source: "README.md".into(),
                fact: "one sandbox per session".into(),
            },
            crate::detect::Seed {
                source: "Cargo.toml".into(),
                fact: "stack: rust".into(),
            },
        ];
        let note = overview(
            "ohmyharness",
            &seeds,
            &[],
            &shipped_templates(),
            "2026-08-08",
        )
        .unwrap()
        .expect("seeds produce a note");

        assert_eq!(check(&note), vec![], "the overview must satisfy the schema");
        for seed in &seeds {
            assert!(note.body.contains(&seed.fact), "{}", note.body);
            assert!(note.body.contains(&seed.source), "{}", note.body);
        }
    }

    /// An empty repo derives nothing, and a note asserting nothing is worse
    /// than no note — it retrieves, and answers with silence.
    #[test]
    fn a_repo_that_derives_nothing_gets_no_note_rather_than_an_empty_one() {
        assert!(overview("x", &[], &[], &shipped_templates(), "2026-08-08")
            .unwrap()
            .is_none());
    }

    /// §6: skip-if-exists is a mode you ask for. Without it, running `init`
    /// twice doubles the store.
    #[test]
    fn a_second_ingest_of_an_unchanged_repo_writes_nothing() {
        let (dir, root) = repo(&[("docs/memory.md", DOC)]);
        let store = dir.path().join("notes");
        let templates = shipped_templates();
        let doc = &documents(&root).unwrap()[0];
        let note = stub(doc, &templates, "2026-08-08").unwrap();

        assert!(write(&store, &note, crate::memory::IfExists::Skip).unwrap());
        assert!(
            !write(&store, &note, crate::memory::IfExists::Skip).unwrap(),
            "the second run creates nothing"
        );
        assert_eq!(
            crate::memory::notes_in(&store, Layer::Team).unwrap().len(),
            1
        );
    }

    /// Fail toward the recoverable mistake: a stub somebody edited is left
    /// alone rather than silently regenerated over.
    #[test]
    fn a_hand_edited_stub_is_never_overwritten() {
        let (dir, root) = repo(&[("docs/memory.md", DOC)]);
        let store = dir.path().join("notes");
        let templates = shipped_templates();
        let note = stub(&documents(&root).unwrap()[0], &templates, "2026-08-08").unwrap();
        write(&store, &note, crate::memory::IfExists::Skip).unwrap();

        let path = store.join(format!("{}.md", note.key));
        let edited = std::fs::read_to_string(&path).unwrap() + "\n- a human added this\n";
        std::fs::write(&path, &edited).unwrap();

        write(&store, &note, crate::memory::IfExists::Skip).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    }

    /// A lint that fires on every note in a new store is a lint people learn
    /// to skip past — the same failure as one that fires on nothing. Linking
    /// the stubs from the overview is a real edge, not a workaround: the
    /// overview *is* the entry point to what `init` derived.
    #[test]
    fn a_freshly_ingested_store_is_not_entirely_orphans() {
        let (dir, root) = repo(&[("docs/a.md", DOC), ("docs/b.md", DOC)]);
        let store = dir.path().join("notes");
        let templates = shipped_templates();

        let mut keys = Vec::new();
        for doc in documents(&root).unwrap() {
            let note = stub(&doc, &templates, "2026-08-08").unwrap();
            keys.push(note.key.clone());
            write(&store, &note, crate::memory::IfExists::Skip).unwrap();
        }
        let seeds = vec![crate::detect::Seed {
            source: "Cargo.toml".into(),
            fact: "stack: rust".into(),
        }];
        let note = overview("repo", &seeds, &keys, &templates, "2026-08-08")
            .unwrap()
            .unwrap();
        write(&store, &note, crate::memory::IfExists::Skip).unwrap();

        let notes = crate::memory::notes_in(&store, Layer::Team).unwrap();
        let orphans: Vec<String> = crate::memory::hygiene(&notes)
            .into_iter()
            .filter(|v| v.rule == crate::memory::Rule::Orphan)
            .map(|v| v.key)
            .collect();

        assert_eq!(
            orphans,
            [note.key.as_str()],
            "only the entry point is unreferenced; every stub is reachable"
        );
        assert!(
            crate::memory::hygiene(&notes)
                .iter()
                .all(|v| v.rule != crate::memory::Rule::DanglingLink),
            "and the links it writes all resolve"
        );
    }

    /// Untracked files are build output, vendored trees and scratch. Ingesting
    /// them fills the store with things nobody wrote.
    #[test]
    fn an_untracked_document_is_not_ingested() {
        let (_d, root) = repo(&[("docs/memory.md", DOC)]);
        std::fs::write(root.join("docs/scratch.md"), "# Scratch\n").unwrap();
        let paths: Vec<String> = documents(&root)
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert_eq!(paths, ["docs/memory.md"]);
    }

    /// Same repo, same stubs — or `init` reshuffles the store every run.
    #[test]
    fn ingestion_is_deterministic() {
        let (_d, root) = repo(&[("docs/b.md", DOC), ("docs/a.md", DOC)]);
        let once = documents(&root).unwrap();
        let twice = documents(&root).unwrap();
        assert_eq!(once, twice);
        assert_eq!(
            once.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["docs/a.md", "docs/b.md"]
        );
    }
}
