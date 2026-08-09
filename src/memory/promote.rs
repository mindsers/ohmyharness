//! `local` → `team`: the one place a human gates anything.
//!
//! §12: promotion is the only point where a wrong note reaches somebody else,
//! which is why it is the only point that asks. Everything else is invisible —
//! a memory you have to approve is a notebook, and nobody keeps one.
//!
//! Decide everything, move nothing, then move exactly what was decided. Same
//! shape as `container::plan` and its `validate`: the judgement is pure and
//! testable without a filesystem, and the part that touches disk has no
//! opinions.

use crate::memory::{Layer, Note};
use crate::profile::Paths;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub key: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// Not in the gitignored layer at all.
    Missing,
    /// The committed layer already holds this key. Layers do not merge and a
    /// key is a primary key within its layer, so this is §6's conflict rather
    /// than something to reconcile.
    AlreadyCommitted,
    /// Invariant 2: the keys that would dangle in a teammate's clone.
    UncommittedLinks(Vec<String>),
    /// The destination is gitignored, so the note would reach nobody. Promote
    /// would exit 0 having done nothing that mattered.
    DestinationIgnored(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub key: String,
    pub reason: Blocker,
}

impl Blocked {
    pub fn say(&self) -> String {
        match &self.reason {
            Blocker::Missing => format!("no local note `{}`", self.key),
            Blocker::AlreadyCommitted => format!(
                "`{}` is already committed; update that note instead",
                self.key
            ),
            Blocker::UncommittedLinks(keys) => format!(
                "`{}` links to {}, which {} not committed — promote {} too, or drop the link",
                self.key,
                keys.join(", "),
                if keys.len() == 1 { "is" } else { "are" },
                if keys.len() == 1 { "it" } else { "them" },
            ),
            Blocker::DestinationIgnored(path) => format!(
                "{} is gitignored, so `{}` would reach nobody",
                path.display(),
                self.key
            ),
        }
    }
}

/// Decide, touching nothing.
///
/// `is_ignored` is injected the way `runtime::select` and
/// `detect::preferred_harness` inject theirs, so the entire gate is testable
/// with no git and no filesystem — and the gate is the part that must not be
/// wrong.
pub fn plan(
    notes: &[Note],
    paths: &Paths,
    keys: &[String],
    is_ignored: &dyn Fn(&Path) -> bool,
) -> std::result::Result<Vec<Promotion>, Vec<Blocked>> {
    let mut promotions = Vec::new();
    let mut blocked = Vec::new();

    for key in keys {
        let Some(note) = notes
            .iter()
            .find(|n| n.key == *key && n.layer == Layer::Local)
        else {
            blocked.push(Blocked {
                key: key.clone(),
                reason: Blocker::Missing,
            });
            continue;
        };
        if notes
            .iter()
            .any(|n| n.key == *key && n.layer == Layer::Team)
        {
            blocked.push(Blocked {
                key: key.clone(),
                reason: Blocker::AlreadyCommitted,
            });
            continue;
        }

        // Checked against the whole plan's closure, not just what is committed
        // now: two notes that point at each other are otherwise unpromotable
        // in either order.
        let dangling = crate::memory::uncommitted_links(notes, key, keys);
        if !dangling.is_empty() {
            blocked.push(Blocked {
                key: key.clone(),
                reason: Blocker::UncommittedLinks(dangling),
            });
            continue;
        }

        let to = Layer::Team.dir(paths).join(format!("{key}.md"));
        if is_ignored(&to) {
            blocked.push(Blocked {
                key: key.clone(),
                reason: Blocker::DestinationIgnored(to),
            });
            continue;
        }

        promotions.push(Promotion {
            key: key.clone(),
            from: note.path.clone(),
            to,
        });
    }

    match blocked.is_empty() {
        true => Ok(promotions),
        false => Err(blocked),
    }
}

/// Move what a plan named, and nothing else.
///
/// The bytes are copied verbatim. The key does not change — identity is
/// `(layer, key)`, so promotion is a removal and a creation rather than a
/// rename — and nothing is re-rendered on the way, because a renderer touching
/// a note it was not asked to change is the defect that produced the vendor's
/// entire remaining quality gap.
pub fn apply(plan: &[Promotion]) -> Result<()> {
    for step in plan {
        let bytes = std::fs::read(&step.from)
            .with_context(|| format!("reading {}", step.from.display()))?;
        if let Some(parent) = step.to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&step.to, &bytes)
            .with_context(|| format!("writing {}", step.to.display()))?;
        std::fs::remove_file(&step.from)
            .with_context(|| format!("removing {}", step.from.display()))?;
    }
    Ok(())
}

/// What to say afterwards. Pure, so the part people actually read is testable.
pub fn report(plan: &[Promotion]) -> String {
    let mut out = String::new();
    for step in plan {
        out.push_str(&format!("promoted {} → {}\n", step.key, step.to.display()));
    }
    // The file has moved; the teammate has not received anything. Saying
    // "promoted" and stopping invites believing otherwise.
    out.push_str("\nnot shared until committed:\n  git add .omh/notes && git commit\n");
    out
}

/// Whether git ignores a path. Shells out exactly as `carry::exclude_path` and
/// `session::git` do.
pub fn git_ignores(repo: &Path, path: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "-q"])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{load, Kind};

    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        (dir, paths)
    }

    fn body(related: &[&str]) -> String {
        let mut b = "# T\n\n## Expected\na\n\n## Observed\nb\n\n## Evidence\nc\n\n\
                     ## Answers\n\n- what happens\n"
            .to_string();
        if !related.is_empty() {
            b.push_str("\n## Related\n\n");
            for r in related {
                b.push_str(&format!("- [[{r}]]\n"));
            }
        }
        b
    }

    fn seed(paths: &Paths, layer: Layer, key: &str, related: &[&str]) {
        let path = layer.dir(paths).join(format!("{key}.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let note = Note {
            key: key.into(),
            kind: Kind::Surprise,
            source: "session s01, claude".into(),
            recorded: "2026-08-07".into(),
            invalidated_by: None,
            body: body(related),
            layer,
            path: path.clone(),
        };
        std::fs::write(&path, crate::memory::render(&note)).unwrap();
    }

    const NEVER: &dyn Fn(&Path) -> bool = &|_: &Path| false;
    const ALWAYS: &dyn Fn(&Path) -> bool = &|_: &Path| true;

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    /// **Invariant 2's second check.** The lint warns; this refuses. §12 makes
    /// promotion the one place a wrong note reaches somebody else, and a
    /// warning is negotiable where a refusal is not.
    #[test]
    fn promote_refuses_a_note_whose_links_are_not_committed() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &[]);
        seed(&paths, Layer::Local, "candidate", &["private"]);
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["candidate"]), NEVER).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(
            err[0].reason,
            Blocker::UncommittedLinks(vec!["private".into()])
        );
        assert!(err[0].say().contains("private"), "{}", err[0].say());
        // And nothing moved.
        assert!(Layer::Local.dir(&paths).join("candidate.md").exists());
    }

    /// The lint and the gate must agree about which links are a problem. Two
    /// implementations drift, and then one subsystem says a store is fine
    /// while the other refuses it.
    #[test]
    fn promote_and_the_lint_agree_about_a_committed_notes_links() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &[]);
        seed(&paths, Layer::Local, "bad", &["private"]);
        seed(&paths, Layer::Team, "already", &[]);
        seed(&paths, Layer::Local, "good", &["already"]);
        let notes = load(&paths).unwrap();

        assert!(plan(&notes, &paths, &keys(&["good"]), NEVER).is_ok());
        let blocked = plan(&notes, &paths, &keys(&["bad"]), NEVER).unwrap_err();
        assert_eq!(blocked[0].key, "bad");
        assert_eq!(
            crate::memory::uncommitted_links(&notes, "bad", &[]),
            vec!["private".to_string()],
            "both sides ask the same question"
        );
    }

    #[test]
    fn mutually_linked_notes_promote_together_or_not_at_all() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "a", &["b"]);
        seed(&paths, Layer::Local, "b", &["a"]);
        let notes = load(&paths).unwrap();

        assert!(
            plan(&notes, &paths, &keys(&["a"]), NEVER).is_err(),
            "alone, `a` would leave `b` dangling"
        );
        assert_eq!(
            plan(&notes, &paths, &keys(&["a", "b"]), NEVER)
                .unwrap()
                .len(),
            2,
            "together, neither dangles"
        );
    }

    /// `fs::rename` clobbers on unix, silently, in one line. §6 makes a key a
    /// primary key: two claims on one key is a conflict, not a merge.
    #[test]
    fn promoting_a_key_the_team_layer_already_holds_is_a_conflict() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Team, "deploy", &[]);
        seed(&paths, Layer::Local, "deploy", &[]);
        let notes = load(&paths).unwrap();
        let before: Vec<Vec<u8>> = [Layer::Team, Layer::Local]
            .iter()
            .map(|l| std::fs::read(l.dir(&paths).join("deploy.md")).unwrap())
            .collect();

        let err = plan(&notes, &paths, &keys(&["deploy"]), NEVER).unwrap_err();
        assert_eq!(err[0].reason, Blocker::AlreadyCommitted);
        assert!(err[0].say().contains("update"), "{}", err[0].say());
        for (i, layer) in [Layer::Team, Layer::Local].iter().enumerate() {
            assert_eq!(
                std::fs::read(layer.dir(&paths).join("deploy.md")).unwrap(),
                before[i],
                "both files untouched"
            );
        }
    }

    /// A root `.gitignore` holding `.omh/` makes promote exit 0 having
    /// achieved nothing: the human ran the only gate there is, and the
    /// teammate still gets nothing.
    #[test]
    fn promote_refuses_when_the_committed_layer_is_gitignored() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["k"]), ALWAYS).unwrap_err();
        assert!(matches!(err[0].reason, Blocker::DestinationIgnored(_)));
        assert!(err[0].say().contains("reach nobody"), "{}", err[0].say());
    }

    #[test]
    fn promoting_a_key_that_is_not_there_says_so() {
        let (_d, paths) = fixture();
        let err = plan(&[], &paths, &keys(&["ghost"]), NEVER).unwrap_err();
        assert_eq!(err[0].reason, Blocker::Missing);
        assert!(err[0].say().contains("ghost"));
    }

    /// Promotion moves one note. It does not rewrite the neighbours whose
    /// links now point into the committed layer, and it does not re-render the
    /// note itself — a renderer touching what it was not asked to change is
    /// the defect behind the vendor's whole remaining quality gap.
    #[test]
    fn promote_moves_one_note_and_rewrites_nothing_else() {
        let (dir, paths) = fixture();
        // The promoted note must itself carry links. An earlier version of
        // this test promoted a note with none, so a mutation that re-rendered
        // it and rewrote `## Related` had nothing to touch and stayed green —
        // which is precisely the vendor defect this assertion exists for.
        seed(&paths, Layer::Team, "anchor", &[]);
        seed(&paths, Layer::Local, "target", &["anchor"]);
        seed(&paths, Layer::Local, "pointer", &["target"]);
        seed(&paths, Layer::Team, "bystander", &[]);

        let snapshot = |skip: &str| -> Vec<(PathBuf, Vec<u8>)> {
            let mut out = Vec::new();
            let mut stack = vec![dir.path().to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if !p.ends_with(skip) {
                        out.push((p.clone(), std::fs::read(&p).unwrap()));
                    }
                }
            }
            out.sort();
            out
        };
        let before = snapshot("target.md");
        let original = std::fs::read(Layer::Local.dir(&paths).join("target.md")).unwrap();

        let notes = load(&paths).unwrap();
        let steps = plan(&notes, &paths, &keys(&["target"]), NEVER).unwrap();
        apply(&steps).unwrap();

        assert_eq!(
            snapshot("target.md"),
            before,
            "every other note is untouched, byte for byte"
        );
        assert_eq!(
            std::fs::read(Layer::Team.dir(&paths).join("target.md")).unwrap(),
            original,
            "and the promoted note's own bytes are unchanged"
        );
        assert!(!Layer::Local.dir(&paths).join("target.md").exists());
    }

    /// The key is identity, not a title, and promotion does not change it —
    /// only which layer holds it.
    #[test]
    fn promotion_changes_the_layer_and_not_the_key() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();
        apply(&plan(&notes, &paths, &keys(&["k"]), NEVER).unwrap()).unwrap();

        let after = load(&paths).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].key, "k");
        assert_eq!(after[0].layer, Layer::Team);
    }

    /// A local note pointing at what was just promoted must still resolve —
    /// promotion moves its target from `{Local}` to `{Team}`, and from `local`
    /// both are reachable. Promotion never cascades, the same rule as deletion.
    #[test]
    fn a_note_pointing_at_a_promoted_one_still_resolves() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "target", &[]);
        seed(&paths, Layer::Local, "pointer", &["target"]);
        let notes = load(&paths).unwrap();
        apply(&plan(&notes, &paths, &keys(&["target"]), NEVER).unwrap()).unwrap();

        let after = load(&paths).unwrap();
        assert_eq!(
            crate::memory::resolve(&after, "target", Layer::Local),
            vec![Layer::Team]
        );
        assert!(
            !crate::memory::hygiene(&after)
                .iter()
                .any(|v| v.rule == crate::memory::Rule::DanglingLink),
            "nothing dangles"
        );
    }

    /// Saying "promoted" and stopping invites believing a teammate now has it.
    /// The file has moved; nothing has been shared.
    #[test]
    fn the_report_says_the_note_is_not_shared_until_it_is_committed() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();
        let text = report(&plan(&notes, &paths, &keys(&["k"]), NEVER).unwrap());

        assert!(text.contains("git commit"), "{text}");
        assert!(
            text.contains(&Layer::Team.dir(&paths).join("k.md").display().to_string()),
            "name where it went: {text}"
        );
    }

    // ── the milestone's gate ────────────────────────────────────────────────

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// **M3 is done when this passes.** A note promoted here is retrievable in
    /// a fresh clone.
    ///
    /// The negative half is what makes it worth writing. Without it the test
    /// passes on an implementation that commits *both* layers — which is worse
    /// than shipping no `promote` at all, because it publishes the layer whose
    /// entire purpose is not being published.
    ///
    /// A fresh `Paths.root` for the clone, so nothing outside the repo can
    /// supply the answer: whatever is found there arrived through git.
    #[test]
    fn a_promoted_note_is_retrievable_in_a_fresh_clone() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: origin.clone(),
        };

        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["config", "user.email", "t@t"]);
        git(&origin, &["config", "user.name", "t"]);
        // What `omh init` writes, and the only reason the local layer stays
        // private in a clone: `info/exclude` is per-clone and never travels.
        std::fs::create_dir_all(origin.join(".omh")).unwrap();
        std::fs::write(
            origin.join(".omh/.gitignore"),
            "local/
",
        )
        .unwrap();

        seed(&paths, Layer::Local, "shared-fact", &[]);
        seed(&paths, Layer::Local, "private-fact", &[]);

        let notes = load(&paths).unwrap();
        apply(
            &plan(&notes, &paths, &keys(&["shared-fact"]), &|p: &Path| {
                git_ignores(&origin, p)
            })
            .unwrap(),
        )
        .unwrap();

        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-qm", "promote"]);

        let clone = dir.path().join("clone");
        let out = std::process::Command::new("git")
            .args(["clone", "-q"])
            .arg(&origin)
            .arg(&clone)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let theirs = Paths {
            root: dir.path().join("their-home"),
            repo: clone.clone(),
        };
        let found = load(&theirs).unwrap();
        let got: Vec<(Layer, &str)> = found.iter().map(|n| (n.layer, n.key.as_str())).collect();

        assert_eq!(
            got,
            vec![(Layer::Team, "shared-fact")],
            "the promoted note and nothing else"
        );
        // Said separately, because the assertion above could be satisfied by a
        // clone that received nothing at all.
        assert!(
            clone.join(".omh/notes/shared-fact.md").exists(),
            "it arrived through git, not through the filesystem"
        );
    }

    /// The other half: the gitignored layer must not travel. Without this the
    /// test above passes on an implementation that commits everything, and
    /// every private note reaches the whole team.
    #[test]
    fn the_gitignored_layer_never_reaches_a_clone() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: origin.clone(),
        };
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["config", "user.email", "t@t"]);
        git(&origin, &["config", "user.name", "t"]);
        std::fs::create_dir_all(origin.join(".omh")).unwrap();
        std::fs::write(
            origin.join(".omh/.gitignore"),
            "local/
",
        )
        .unwrap();

        // Deliberately inside the checkout, which is where §4 first put the
        // local layer — so this also pins that moving it back would be caught.
        let inside = origin.join(".omh/local/notes");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(
            inside.join("private.md"),
            "---
key: private
---
",
        )
        .unwrap();
        seed(&paths, Layer::Team, "public", &[]);

        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-qm", "seed"]);

        let tracked = std::process::Command::new("git")
            .arg("-C")
            .arg(&origin)
            .args(["ls-files", ".omh"])
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&tracked.stdout);
        assert!(listed.contains(".omh/notes/public.md"), "got: {listed}");
        assert!(
            !listed.contains("local"),
            "the gitignored layer is tracked: {listed}"
        );
    }

    /// One bad key blocks the batch rather than half-promoting it. A partial
    /// promotion leaves a store nobody planned, and the human who ran the gate
    /// would have to work out which half landed.
    #[test]
    fn one_blocked_key_stops_the_whole_batch() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "fine", &[]);
        seed(&paths, Layer::Local, "private", &[]);
        seed(&paths, Layer::Local, "bad", &["private"]);
        let notes = load(&paths).unwrap();

        assert!(plan(&notes, &paths, &keys(&["fine", "bad"]), NEVER).is_err());
        assert!(
            Layer::Local.dir(&paths).join("fine.md").exists(),
            "nothing moved"
        );
    }
}
