//! `local` → `team`: the one place a human gates anything.
//!
//! §12: promotion is the only point where a wrong note reaches somebody else,
//! which is why it is the only point that asks. Everything else is invisible —
//! a memory you have to approve is a notebook, and nobody keeps one.
//!
//! Decide everything, move nothing, then move exactly what was decided. Same
//! shape as `container::plan` and its `validate`: the part that touches disk
//! has no opinions, and every judgement is driven by values a test can supply
//! — the loaded notes, and an injected answer to "does git ignore this".
//!
//! `plan` does read the filesystem for one thing: whether the destination is
//! already occupied. The store's own view answers the common case and is
//! checked first, but a file the store never loaded still cannot be silently
//! overwritten, and only the disk knows about that one.

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
    /// One key over several local files. `rm` refuses this store and asks for
    /// `--at`; picking the first silently promoted one and left the other
    /// claiming the key.
    Ambiguous(Vec<PathBuf>),
    /// A key that is not a key. `validate_key` guards the mint; a key read
    /// back off disk has never been through it, and it becomes a path.
    InvalidKey,
    /// The schema refuses this note. Sharing a note the store would not accept
    /// is the one thing promotion must not do — and an unclosed fence hides
    /// the links invariant 2 is about, so the gate below would pass on it
    /// having read nothing.
    Refused(Vec<crate::memory::Violation>),
    /// Invariant 2: the keys that would dangle in a teammate's clone.
    UncommittedLinks(Vec<String>),
    /// The destination is gitignored, so the note would reach nobody. Promote
    /// would exit 0 having done nothing that mattered.
    DestinationIgnored(PathBuf),
    /// Something already occupies the destination path. The conflict check
    /// above asks about a *key*; this asks about the file that key would be
    /// written to, which a note whose frontmatter disagrees with its filename
    /// owns without owning the key.
    DestinationExists(PathBuf),
    /// git could not say whether the destination is ignored. Not an answer,
    /// and on this gate the open direction publishes.
    IgnoreUnknown(String),
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
            Blocker::Ambiguous(paths) => format!(
                "`{}` is one key over {} files — name one with --at: {}",
                self.key,
                paths.len(),
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Blocker::InvalidKey => format!(
                "`{}` is not a key: a key is slash-separated slugs, never a path",
                self.key
            ),
            Blocker::Refused(violations) => format!(
                "`{}` is refused by the schema, so it is not shareable: {}",
                self.key,
                violations
                    .iter()
                    .map(|v| v.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
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
            Blocker::DestinationExists(path) => format!(
                "{} already exists, so promoting `{}` would overwrite it",
                path.display(),
                self.key
            ),
            Blocker::IgnoreUnknown(why) => format!(
                "cannot tell whether the committed layer is gitignored, so `{}` is \
                 not safe to promote: {why}",
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
    is_ignored: &dyn Fn(&Path) -> Result<bool>,
) -> std::result::Result<Vec<Promotion>, Vec<Blocked>> {
    let mut promotions = Vec::new();
    let mut blocked = Vec::new();

    // A key named twice is one note. Left as two steps the plan moved the file
    // on the first and failed to read it on the second, so the promotion
    // succeeded *and* the command reported failure.
    let mut asked: Vec<&String> = Vec::new();
    for key in keys {
        if !asked.contains(&key) {
            asked.push(key);
        }
    }

    for key in asked {
        let mut block = |reason| {
            blocked.push(Blocked {
                key: key.clone(),
                reason,
            })
        };

        // Before anything derived from it: every check below builds a path out
        // of this key, and `create_dir_all` on that path is an arbitrary write.
        if crate::memory::validate_key(key).is_err() {
            block(Blocker::InvalidKey);
            continue;
        }

        let claimants: Vec<&Note> = notes
            .iter()
            .filter(|n| n.key == *key && n.layer == Layer::Local)
            .collect();
        let note = match claimants.as_slice() {
            [] => {
                block(Blocker::Missing);
                continue;
            }
            [one] => *one,
            many => {
                block(Blocker::Ambiguous(
                    many.iter().map(|n| n.path.clone()).collect(),
                ));
                continue;
            }
        };
        if notes
            .iter()
            .any(|n| n.key == *key && n.layer == Layer::Team)
        {
            block(Blocker::AlreadyCommitted);
            continue;
        }

        // Asked before invariant 2, not after. An unclosed fence hides every
        // link beneath it, so a refused note would clear the link check having
        // had none of its links read.
        let refused: Vec<crate::memory::Violation> = crate::memory::check(note)
            .into_iter()
            .filter(|v| v.rule.severity() == crate::memory::Severity::Refused)
            .collect();
        if !refused.is_empty() {
            block(Blocker::Refused(refused));
            continue;
        }

        // Checked against the whole plan's closure, not just what is committed
        // now: two notes that point at each other are otherwise unpromotable
        // in either order.
        let dangling = crate::memory::uncommitted_links(notes, note, keys);
        if !dangling.is_empty() {
            block(Blocker::UncommittedLinks(dangling));
            continue;
        }

        let to = Layer::Team.dir(paths).join(format!("{key}.md"));
        // The store's own view first, so the common collision is caught with
        // no filesystem at all: a committed note whose frontmatter disagrees
        // with its filename owns this path without owning the key.
        if notes.iter().any(|n| n.path == to) || to.exists() {
            block(Blocker::DestinationExists(to));
            continue;
        }
        match is_ignored(&to) {
            Err(why) => {
                block(Blocker::IgnoreUnknown(format!("{why:#}")));
                continue;
            }
            Ok(true) => {
                block(Blocker::DestinationIgnored(to));
                continue;
            }
            Ok(false) => {}
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
/// In two passes, because a note that exists in neither layer is the one
/// outcome no message can repair. Every destination is written first; only
/// once all of them are there is any source removed. A failure in the first
/// pass rolls the new files back and nothing has moved, which is the promise
/// `plan`'s all-or-nothing gate makes and the loop below used to break as soon
/// as the second step hit a full disk.
pub fn apply(plan: &[Promotion]) -> Result<()> {
    let mut written: Vec<&Promotion> = Vec::new();
    for step in plan {
        match copy(step) {
            Ok(()) => written.push(step),
            Err(e) => {
                // The sources are all still here, so undoing the copies
                // restores the store exactly. Say so if it cannot be undone:
                // a rollback that half-works and reports nothing is the
                // failure this pass exists to avoid.
                let stranded: Vec<String> = written
                    .iter()
                    .filter(|done| std::fs::remove_file(&done.to).is_err())
                    .map(|done| done.to.display().to_string())
                    .collect();
                if stranded.is_empty() {
                    return Err(e);
                }
                return Err(e.context(format!(
                    "and these copies could not be rolled back, so they now \
                     claim their keys in both layers: {}",
                    stranded.join(", ")
                )));
            }
        }
    }

    let stranded: Vec<String> = plan
        .iter()
        .filter(|step| std::fs::remove_file(&step.from).is_err())
        .map(|step| step.key.clone())
        .collect();
    if !stranded.is_empty() {
        anyhow::bail!(
            "promoted, but the local copies of {} could not be removed — those \
             keys are now claimed in both layers; remove them by hand",
            stranded.join(", ")
        );
    }
    Ok(())
}

/// One note into the committed layer, refusing to land on anything already
/// there. `create_new` rather than `write`: `apply` is handed paths and must
/// not be the component that trusts them.
fn copy(step: &Promotion) -> Result<()> {
    let bytes =
        std::fs::read(&step.from).with_context(|| format!("reading {}", step.from.display()))?;
    if let Some(parent) = step.to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&step.to)
        .with_context(|| format!("writing {}", step.to.display()))?;
    std::io::Write::write_all(&mut out, &bytes)
        .with_context(|| format!("writing {}", step.to.display()))
}

/// What to say afterwards. Pure, so the part people actually read is testable.
///
/// Paths are named relative to the repo: the destination is inside it, an
/// absolute path leaks the operator's home into anything they paste, and the
/// documented transcript shows the short form.
pub fn report(plan: &[Promotion], paths: &Paths) -> String {
    let mut out = String::new();
    for step in plan {
        let to = step.to.strip_prefix(&paths.repo).unwrap_or(&step.to);
        out.push_str(&format!("promoted {} → {}\n", step.key, to.display()));
    }
    // The file has moved; the teammate has not received anything. Saying
    // "promoted" and stopping invites believing otherwise.
    //
    // `:/` is git's root-relative pathspec. Without it this line is advice
    // that only works from the repo root: from a subdirectory a bare
    // `.omh/notes` matches nothing and git exits non-zero, which is the worst
    // moment to hand somebody a command that fails.
    let notes = Layer::Team
        .dir(paths)
        .strip_prefix(&paths.repo)
        .map(|rel| format!(":/{}", rel.display()))
        .unwrap_or_else(|_| Layer::Team.dir(paths).display().to_string());
    out.push_str(&format!(
        "\nnot shared until committed:\n  git add {notes} && git commit\n"
    ));
    out
}

/// Whether git ignores a path. Shells out exactly as `carry::exclude_path` and
/// `session::git` do.
///
/// `check-ignore` has three answers, not two: 0 ignored, 1 not ignored, and
/// anything else — no git on `PATH`, a `.git` that is not a repository — meaning
/// it could not tell. Collapsing that third case into `false` disabled the one
/// gate standing between a promotion and a directory git will never track, and
/// no test could see it because nothing asserted the `true` direction either.
pub fn git_ignores(repo: &Path, path: &Path) -> Result<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "-q"])
        .arg(path)
        .output()
        .with_context(|| format!("running git check-ignore in {}", repo.display()))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "git check-ignore in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
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

    /// A note written by hand rather than by `render`, for the states the
    /// happy-path helper cannot express: a key the store would never mint, a
    /// fence left open, two files claiming one key.
    fn seed_raw(paths: &Paths, layer: Layer, at: &str, key: &str, body: &str) {
        let path = layer.dir(paths).join(at);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "---\nkey: {key}\ntype: surprise\nsource: session s01, claude\n\
                 recorded: 2026-08-07\n---\n\n{body}"
            ),
        )
        .unwrap();
    }

    const NEVER: &dyn Fn(&Path) -> Result<bool> = &|_: &Path| Ok(false);
    const ALWAYS: &dyn Fn(&Path) -> Result<bool> = &|_: &Path| Ok(true);

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    // ── what the gate must refuse ───────────────────────────────────────────

    /// **`promote` re-validates a key it did not mint.** `validate_key` runs in
    /// `expand_key`, where the key is created; `promote` reads one back off
    /// disk, and `apply` calls `create_dir_all` on its parent. That makes the
    /// destination path an arbitrary write, which is exactly the hazard
    /// `validate_key`'s own comment names.
    #[test]
    fn promote_refuses_a_key_that_is_not_a_key() {
        let (_d, paths) = fixture();
        seed_raw(
            &paths,
            Layer::Local,
            "escape.md",
            "../../../../pwned",
            &body(&[]),
        );
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["../../../../pwned"]), NEVER).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(
            matches!(err[0].reason, Blocker::InvalidKey),
            "got {:?}",
            err[0].reason
        );
        assert!(
            !Layer::Team
                .dir(&paths)
                .join("../../../../pwned.md")
                .exists(),
            "nothing was written outside the store"
        );
    }

    /// **The commit is titled "refuse to share a broken one".** `links` skips
    /// fenced lines, so a fence left open hides every link after it — which is
    /// why `check` refuses such a note outright. Without asking `check`, the
    /// invariant-2 gate passes *vacuously* on the one note whose links cannot
    /// be read, and both detectors stay blind once it is committed.
    #[test]
    fn promote_refuses_a_note_the_schema_refuses() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &[]);
        seed_raw(
            &paths,
            Layer::Local,
            "candidate.md",
            "candidate",
            "# T\n\n## Expected\na\n\n## Observed\nb\n\n## Evidence\n\n```sh\nopen\n\n\
             ## Answers\n\n- q\n\n## Related\n\n- [[private]]\n",
        );
        let notes = load(&paths).unwrap();

        assert!(
            crate::memory::uncommitted_links(
                &notes,
                notes.iter().find(|n| n.key == "candidate").unwrap(),
                &[]
            )
            .is_empty(),
            "premise: the open fence hides the link, so invariant 2 sees nothing"
        );
        let err = plan(&notes, &paths, &keys(&["candidate"]), NEVER).unwrap_err();
        assert!(
            matches!(err[0].reason, Blocker::Refused(_)),
            "got {:?}",
            err[0].reason
        );
    }

    /// **The key check and the write target are different things.** The
    /// conflict guard asks whether a *key* is committed; `apply` writes to a
    /// *path* derived from that key. A committed note whose frontmatter
    /// disagrees with its filename owns the path without owning the key, and
    /// `fs::write` clobbers.
    #[test]
    fn promote_refuses_to_overwrite_a_file_the_key_does_not_own() {
        let (_d, paths) = fixture();
        seed_raw(
            &paths,
            Layer::Team,
            "deploy.md",
            "deploy-runbook",
            &body(&[]),
        );
        seed(&paths, Layer::Local, "deploy", &[]);
        let theirs = std::fs::read(Layer::Team.dir(&paths).join("deploy.md")).unwrap();
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["deploy"]), NEVER).unwrap_err();
        assert!(
            matches!(err[0].reason, Blocker::DestinationExists(_)),
            "got {:?}",
            err[0].reason
        );
        assert_eq!(
            std::fs::read(Layer::Team.dir(&paths).join("deploy.md")).unwrap(),
            theirs,
            "the note that already lived there is untouched"
        );
    }

    /// The backstop for anything the plan could not see. `apply` is handed
    /// paths; it must not be the component that trusts them.
    #[test]
    fn apply_refuses_to_clobber_an_existing_destination() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let to = Layer::Team.dir(&paths).join("k.md");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&to, b"theirs").unwrap();

        let step = Promotion {
            key: "k".into(),
            from: Layer::Local.dir(&paths).join("k.md"),
            to: to.clone(),
        };
        assert!(apply(&[step]).is_err(), "must not overwrite");
        assert_eq!(std::fs::read(&to).unwrap(), b"theirs");
        assert!(
            Layer::Local.dir(&paths).join("k.md").exists(),
            "and the source is still there"
        );
    }

    // ── what the gate must not answer by guessing ───────────────────────────

    /// `check-ignore` exits 0 for ignored, 1 for not, and 128 when it could not
    /// answer. Collapsing that to a bool made "git could not tell" mean "not
    /// ignored" — the open direction, on the gate whose whole job is to stop a
    /// promotion that would reach nobody.
    #[test]
    fn an_ignore_check_that_cannot_answer_blocks_the_promotion() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["k"]), &|_: &Path| {
            Err(anyhow::anyhow!("git exploded"))
        })
        .unwrap_err();
        assert!(
            matches!(err[0].reason, Blocker::IgnoreUnknown(_)),
            "got {:?}",
            err[0].reason
        );
        assert!(err[0].say().contains("git exploded"), "{}", err[0].say());
    }

    /// Nothing asserted `git_ignores` ever returns `true`, so the production
    /// wiring of the gate could be replaced with `=> false` and stay green.
    #[test]
    fn git_ignores_recognises_a_path_git_actually_ignores() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".omh")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), ".omh/\n").unwrap();

        assert!(
            git_ignores(&repo, &repo.join(".omh/notes/k.md")).unwrap(),
            "a path under an ignored directory is ignored"
        );
        assert!(
            !git_ignores(&repo, &repo.join("README.md")).unwrap(),
            "and one that is not, is not"
        );
    }

    /// The other half: outside a repository git cannot answer, and that must
    /// surface as an error rather than as "not ignored".
    #[test]
    fn git_ignores_reports_an_error_where_git_cannot_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            git_ignores(dir.path(), &dir.path().join("k.md")).is_err(),
            "not a git repository is not the same answer as `not ignored`"
        );
    }

    // ── what must survive a failure ─────────────────────────────────────────

    /// The docs promise the batch is all-or-nothing. That held for gate
    /// blockers and not for the writes: an I/O error on a later step left the
    /// earlier ones moved, with `report` never printed and only the failing key
    /// named.
    #[test]
    fn a_failed_write_leaves_the_store_exactly_as_it_was() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "first", &[]);
        seed(&paths, Layer::Local, "second", &[]);
        // A directory where the second note's file must go: the write fails,
        // and it fails after the first note has already been copied.
        let blocked = Layer::Team.dir(&paths).join("second.md");
        std::fs::create_dir_all(&blocked).unwrap();

        let steps = vec![
            Promotion {
                key: "first".into(),
                from: Layer::Local.dir(&paths).join("first.md"),
                to: Layer::Team.dir(&paths).join("first.md"),
            },
            Promotion {
                key: "second".into(),
                from: Layer::Local.dir(&paths).join("second.md"),
                to: blocked,
            },
        ];
        assert!(apply(&steps).is_err());

        assert!(
            Layer::Local.dir(&paths).join("first.md").exists(),
            "the source that would have moved is still in the local layer"
        );
        assert!(
            !Layer::Team.dir(&paths).join("first.md").exists(),
            "and the half that landed was rolled back"
        );
    }

    /// `omh memory promote k k` — trivially produced by a shell glob. The plan
    /// held two identical steps, the first moved the file and the second failed
    /// to read it, so the promotion succeeded *and* the command exited 1 with
    /// the `git add` instruction swallowed.
    #[test]
    fn a_key_named_twice_is_promoted_once() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();

        let steps = plan(&notes, &paths, &keys(&["k", "k"]), NEVER).unwrap();
        assert_eq!(steps.len(), 1, "one note, one move");
        apply(&steps).unwrap();
        assert!(Layer::Team.dir(&paths).join("k.md").exists());
    }

    /// `rm` refuses this store and demands `--at`. `promote` silently picked
    /// the first file and left the other claiming the key — two subsystems
    /// telling two stories about one store, which is the failure this module's
    /// own comments are written against.
    #[test]
    fn two_local_files_claiming_one_key_are_ambiguous() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "dup", &[]);
        seed_raw(&paths, Layer::Local, "ns/dup.md", "dup", &body(&[]));
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["dup"]), NEVER).unwrap_err();
        assert!(
            matches!(err[0].reason, Blocker::Ambiguous(_)),
            "got {:?}",
            err[0].reason
        );
        assert!(err[0].say().contains("--at"), "{}", err[0].say());
        assert!(
            Layer::Local.dir(&paths).join("dup.md").exists()
                && Layer::Local.dir(&paths).join("ns/dup.md").exists(),
            "neither was moved"
        );
    }

    /// `main` prints every blocker, so the plan must collect every blocker. An
    /// early return would send the operator through one fix-and-retry cycle per
    /// bad key.
    #[test]
    fn the_plan_reports_every_blocked_key_not_only_the_first() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "private", &[]);
        seed(&paths, Layer::Local, "one", &["private"]);
        seed(&paths, Layer::Local, "two", &["private"]);
        let notes = load(&paths).unwrap();

        let err = plan(&notes, &paths, &keys(&["one", "two", "nope"]), NEVER).unwrap_err();
        let blocked: Vec<&str> = err.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(blocked, vec!["one", "two", "nope"]);
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
            crate::memory::uncommitted_links(
                &notes,
                notes.iter().find(|n| n.key == "bad").unwrap(),
                &[]
            ),
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
        let text = report(&plan(&notes, &paths, &keys(&["k"]), NEVER).unwrap(), &paths);

        assert!(text.contains("git commit"), "{text}");
        assert!(text.contains("k.md"), "name where it went: {text}");
    }

    /// The destination is inside the repo, so naming it from the repo is what a
    /// reader can act on — and it is what the documented transcript shows. An
    /// absolute path also leaks the operator's home directory into output that
    /// gets pasted into issues.
    #[test]
    fn the_report_names_the_destination_relative_to_the_repo() {
        let (_d, paths) = fixture();
        seed(&paths, Layer::Local, "surprise/thing", &[]);
        let notes = load(&paths).unwrap();
        let text = report(
            &plan(&notes, &paths, &keys(&["surprise/thing"]), NEVER).unwrap(),
            &paths,
        );

        assert!(
            text.contains(".omh/notes/surprise/thing.md"),
            "repo-relative: {text}"
        );
        assert!(
            !text.contains(&paths.repo.display().to_string()),
            "not the absolute path: {text}"
        );
    }

    /// **The instruction is run, not read.** `git add .omh/notes` matches
    /// nothing from a subdirectory and exits 128, so the one line telling the
    /// operator how to finish the job failed exactly when they were not sitting
    /// at the repo root.
    #[test]
    fn the_git_add_the_report_suggests_works_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: repo.clone(),
        };
        git(&repo, &["init", "-q", "-b", "main"]);

        seed(&paths, Layer::Local, "k", &[]);
        let notes = load(&paths).unwrap();
        let steps = plan(&notes, &paths, &keys(&["k"]), NEVER).unwrap();
        apply(&steps).unwrap();
        let text = report(&steps, &paths);

        // The literal line the operator would copy, run where they actually
        // are. Only up to the `&&`: the commit that follows opens an editor,
        // and it is the `add` that has to resolve a path.
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with("git add"))
            .unwrap_or_else(|| panic!("no git add line: {text}"));
        let suggested = line.split("&&").next().unwrap().trim();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(suggested)
            .current_dir(repo.join("src/deep"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "`{suggested}` from a subdirectory: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let staged = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&staged.stdout).contains(".omh/notes/k.md"),
            "and it staged the promoted note: {}",
            String::from_utf8_lossy(&staged.stdout)
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
    /// The negative half is what makes it worth writing — but not for the
    /// reason it first appears. The local layer lives outside the checkout, so
    /// no `git add -A` could publish it and no implementation could fail that
    /// way. What asserting the *exact* set catches is a `plan` that promotes
    /// more than it was asked to, which is a mistake this module can make.
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
        // What `omh init` writes. It is not what keeps the notes local layer
        // out of the clone — that layer lives outside the checkout entirely
        // (§4) — but it covers the in-repo config layer, and unlike
        // `info/exclude` it is committed, so it travels.
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

    /// The gitignored layer must not travel. This does not test the shipped
    /// local layer, which lives outside the checkout and so cannot reach a
    /// clone by any route: it pins what `omh init` writes, so that moving the
    /// layer back inside the checkout — where §4 first put it — would still be
    /// caught by the committed ignore rule rather than silently publishing
    /// every private note.
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
