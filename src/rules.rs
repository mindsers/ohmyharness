//! Composing the rules file, rather than replacing it.
//!
//! omh mounts its rules read-only over the filenames each harness reads, which
//! keeps omh's staging out of the user's commit — and, until this module
//! existed, also hid the project's own `AGENTS.md` for the length of the
//! session. The agent ran without ever reading the conventions the repo had
//! written down, and nothing said so: the file was intact on disk, so the guard
//! that watched the disk stayed green.
//!
//! So the mount stays and the *document* grows: the project's rules are read
//! before anything is mounted over them, and composed in.

use crate::adapter::{Adapter, Binding, Capability};
use crate::config::Layer;
use crate::profile::Paths;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The filename omh reads on the project's side. Harness-neutral on purpose:
/// what the *container* sees is the adapter's business, and an adapter that
/// reads something else says so in `path` and `also`.
const CANONICAL: &str = "AGENTS.md";

/// One contribution to the composed document, and where it came from.
#[derive(Debug, PartialEq)]
pub struct Section {
    pub origin: String,
    pub body: String,
}

/// What the launcher has to tell the user. Empty means everything was ordinary.
///
/// Composing a file the user did not name is a fallback, and a fallback nobody
/// is told about is indistinguishable from a bug — that is the whole reason
/// this is returned rather than handled quietly.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// The project's rules came from a name other than `AGENTS.md`.
    pub read_instead: Option<String>,
    /// A second declared name exists with different content and was not used.
    pub not_composed: Option<String>,
}

/// The whole rules document for one launch, plus what to say about it.
///
/// `worktree` rather than the repo root: a session works on its own checkout,
/// and a branch that has just written its rules should be governed by them.
pub fn compose(
    paths: &Paths,
    adapter: &Adapter,
    worktree: &Path,
    base: &str,
) -> Result<(String, Report)> {
    let (project, report) = match adapter.supports(Capability::Rules) {
        Some(binding) => project(binding, paths, worktree, base)?,
        // A harness that cannot express rules never gets here — `plan` drops
        // the capability first — but reading the binding is how the filenames
        // are discovered, so its absence has to mean "no project section"
        // rather than a panic.
        None => (None, Report::default()),
    };

    let mut project = project;
    let mut sections = Vec::new();
    for layer in Layer::ALL {
        if let Some(body) = read(&layer_source(layer, paths))? {
            sections.push(Section {
                origin: layer.to_string(),
                body,
            });
        }
        // The project's own rules sit after your personal ones and before the
        // omh-seeded layers: `.omh/profile/AGENTS.md` currently carries omh's
        // generated sections, and those belong last.
        if layer == Layer::Personal {
            sections.extend(project.take());
        }
    }

    Ok((render(&sections), report))
}

/// The project's own rules, and what to report about finding them.
fn project(
    binding: &Binding,
    paths: &Paths,
    worktree: &Path,
    base: &str,
) -> Result<(Option<Section>, Report)> {
    let mut report = Report::default();
    let names = candidates(binding);

    let mut found: Option<(String, String)> = None;
    for name in &names {
        let Some(body) = body(paths, worktree, base, name)? else {
            continue;
        };
        match &found {
            None => found = Some((name.clone(), body)),
            // A second name with the same bytes is the common
            // one-points-at-the-other case, and warning about it would train
            // people to ignore the warning that matters.
            Some((_, chosen)) if *chosen == body => {}
            Some(_) => {
                report.not_composed = Some(name.clone());
                break;
            }
        }
    }

    let Some((name, body)) = found else {
        return Ok((None, report));
    };
    if name != CANONICAL {
        report.read_instead = Some(name.clone());
    }
    Ok((
        Some(Section {
            origin: format!("<repo>/{name}"),
            body,
        }),
        report,
    ))
}

/// Filenames to look for, canonical first.
///
/// Taken from the adapter rather than written here: the harness that reads
/// `CLAUDE.md` is a fact the adapter already records, and duplicating it would
/// give omh two places to disagree with itself.
fn candidates(binding: &Binding) -> Vec<String> {
    let mut names: Vec<String> = std::iter::once(&binding.path)
        .chain(binding.also.iter())
        .filter_map(|t| {
            Path::new(t)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect();
    names.sort_by_key(|n| (n != CANONICAL, n.clone()));
    names.dedup();
    names
}

/// The branch's copy if the worktree has one, otherwise the default branch's.
///
/// Branch-first because a session that has just written an `AGENTS.md` the
/// default branch does not have yet should be governed by it. `git show` is
/// best-effort: a scratch directory has no `.git` at all, and a repo with no
/// commits has nothing to show — neither is a reason to refuse to launch.
fn body(paths: &Paths, worktree: &Path, base: &str, name: &str) -> Result<Option<String>> {
    if let Some(body) = read(&worktree.join(name))? {
        return Ok(Some(body));
    }
    if base.is_empty() {
        return Ok(None);
    }
    let out = Command::new("git")
        .current_dir(&paths.repo)
        .args(["show", &format!("{base}:{name}")])
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        _ => None,
    })
}

/// Absent and unreadable are different answers.
///
/// `config::read_layer` exists for this reason: `let Ok(..) else { continue }`
/// made a `chmod 000` file report as "not declared", and the resulting advice
/// was a closed loop that exited 0.
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(Some(body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Each section says where it came from, so the agent — and anyone reading the
/// staged file — can tell a project convention from omh's own instruction.
fn render(sections: &[Section]) -> String {
    sections
        .iter()
        .map(|s| format!("<!-- omh: {} -->\n{}", s.origin, s.body.trim_end()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Where each layer's rules live. Kept next to the composition so the two
/// cannot drift.
fn layer_source(layer: Layer, paths: &Paths) -> PathBuf {
    layer.dir(paths).join(CANONICAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    struct Fx {
        _dir: tempfile::TempDir,
        paths: Paths,
        worktree: PathBuf,
    }

    /// The real `claude` adapter, so the filenames under test are the ones
    /// omh actually ships rather than a fixture's idea of them.
    fn claude() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    fn fixture() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        Fx {
            _dir: dir,
            paths,
            worktree,
        }
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn layer(fx: &Fx, layer: Layer, body: &str) {
        write(layer_source(layer, &fx.paths), body);
    }

    /// No base: these cases are about files, and consulting git would drag a
    /// repository into tests that do not need one.
    fn composed(fx: &Fx) -> (String, Report) {
        compose(&fx.paths, &claude(), &fx.worktree, "").unwrap()
    }

    /// Position, not presence. A `contains` assertion stays green when the
    /// order is wrong, and the order is the whole question: omh's own sections
    /// live in the shared layer today and have to come after the project's.
    #[test]
    fn sections_are_ordered_personal_project_shared_local() {
        let fx = fixture();
        layer(&fx, Layer::Personal, "PERSONAL");
        layer(&fx, Layer::Shared, "SHARED");
        layer(&fx, Layer::Local, "LOCAL");
        write(fx.worktree.join("AGENTS.md"), "PROJECT");

        let (body, _) = composed(&fx);
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing"))
        };

        assert!(
            at("PERSONAL") < at("PROJECT"),
            "personal before project:\n{body}"
        );
        assert!(
            at("PROJECT") < at("SHARED"),
            "project before shared:\n{body}"
        );
        assert!(at("SHARED") < at("LOCAL"), "shared before local:\n{body}");
    }

    /// Four sources reach the agent as one document with no seam. Without a
    /// marker per section, a project convention and an omh instruction are the
    /// same kind of sentence to whoever reads it next.
    #[test]
    fn each_section_names_where_it_came_from() {
        let fx = fixture();
        layer(&fx, Layer::Personal, "PERSONAL");
        write(fx.worktree.join("AGENTS.md"), "PROJECT");

        let (body, _) = composed(&fx);

        assert!(body.contains("<!-- omh: personal -->"), "got:\n{body}");
        assert!(
            body.contains("<!-- omh: <repo>/AGENTS.md -->"),
            "got:\n{body}"
        );
    }

    /// Most repos that have used Claude Code have a `CLAUDE.md` and no
    /// `AGENTS.md`. Refusing to read it would leave the agent with no project
    /// rules at all, which is worse than the bug being fixed — so it is composed
    /// and the user is told which file was used.
    #[test]
    fn claude_md_is_composed_when_agents_md_is_absent() {
        let fx = fixture();
        write(fx.worktree.join("CLAUDE.md"), "PROJECT VIA CLAUDE");

        let (body, report) = composed(&fx);

        assert!(body.contains("PROJECT VIA CLAUDE"), "got:\n{body}");
        assert_eq!(report.read_instead.as_deref(), Some("CLAUDE.md"));
        assert_eq!(report.not_composed, None);
    }

    /// `AGENTS.md` is canonical, so it wins — but silently dropping the other
    /// file is how a repo loses rules it believes are in force.
    #[test]
    fn agents_md_wins_when_both_exist_and_claude_is_reported() {
        let fx = fixture();
        write(fx.worktree.join("AGENTS.md"), "THE CANONICAL ONE");
        write(fx.worktree.join("CLAUDE.md"), "SOMETHING ELSE ENTIRELY");

        let (body, report) = composed(&fx);

        assert!(body.contains("THE CANONICAL ONE"), "got:\n{body}");
        assert!(!body.contains("SOMETHING ELSE ENTIRELY"), "got:\n{body}");
        assert_eq!(report.not_composed.as_deref(), Some("CLAUDE.md"));
        assert_eq!(report.read_instead, None, "it read the canonical name");
    }

    /// The common shape is one file pointing at the other, or a symlink-like
    /// copy. Warning about it every launch trains people to ignore the warning
    /// that matters.
    #[test]
    fn identical_agents_and_claude_stay_quiet() {
        let fx = fixture();
        write(fx.worktree.join("AGENTS.md"), "SAME BYTES");
        write(fx.worktree.join("CLAUDE.md"), "SAME BYTES");

        let (_, report) = composed(&fx);

        assert_eq!(
            report,
            Report::default(),
            "identical files are not a problem"
        );
    }

    /// A repo with no rules of its own must compose exactly what it did before
    /// this module existed.
    #[test]
    fn a_repo_with_no_rules_file_composes_only_the_profile_layers() {
        let fx = fixture();
        layer(&fx, Layer::Personal, "PERSONAL");
        layer(&fx, Layer::Shared, "SHARED");

        let (body, report) = composed(&fx);

        assert!(!body.contains("<repo>/"), "no project section:\n{body}");
        assert!(body.contains("PERSONAL") && body.contains("SHARED"));
        assert_eq!(report, Report::default());
    }

    // ── against a real repository ───────────────────────────────────────────
    //
    // `git show` is the half of this that no filesystem fixture can reach, and
    // the half that decides what a session sees when its branch has no rules
    // file of its own.

    fn git(cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo whose default branch commits `AGENTS.md`.
    fn committed(fx: &Fx, body: &str) {
        std::fs::create_dir_all(&fx.paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            git(&fx.paths.repo, &args);
        }
        std::fs::write(fx.paths.repo.join("AGENTS.md"), body).unwrap();
        git(&fx.paths.repo, &["add", "AGENTS.md"]);
        git(&fx.paths.repo, &["commit", "-q", "-m", "rules"]);
    }

    /// A session that has just written its own rules should be governed by
    /// them. Reading the default branch first would hand the agent the rules it
    /// is in the middle of replacing.
    #[test]
    fn the_worktree_copy_wins_over_the_default_branch() {
        let fx = fixture();
        committed(&fx, "WHAT MAIN SAYS");
        write(fx.worktree.join("AGENTS.md"), "WHAT THIS BRANCH SAYS");

        let (body, _) = compose(&fx.paths, &claude(), &fx.worktree, "main").unwrap();

        assert!(body.contains("WHAT THIS BRANCH SAYS"), "got:\n{body}");
        assert!(!body.contains("WHAT MAIN SAYS"), "got:\n{body}");
    }

    /// The case the filesystem cannot answer: a session branch that deleted the
    /// rules file, or a worktree checked out before it existed. Falling back to
    /// the default branch is what keeps the project's conventions in force.
    #[test]
    fn the_default_branch_supplies_it_when_the_worktree_has_none() {
        let fx = fixture();
        committed(&fx, "WHAT MAIN SAYS");

        let (body, _) = compose(&fx.paths, &claude(), &fx.worktree, "main").unwrap();

        assert!(body.contains("WHAT MAIN SAYS"), "got:\n{body}");
    }

    /// `omh auth` and `omh doctor` run in scratch directories with no `.git` at
    /// all, and a fresh repo has no commit to show. Neither is a reason to
    /// refuse to launch — `git show` is best-effort by construction.
    #[test]
    fn a_repo_with_no_git_history_still_composes() {
        let fx = fixture();
        layer(&fx, Layer::Personal, "PERSONAL");
        std::fs::create_dir_all(&fx.paths.repo).unwrap();

        let (body, report) = compose(&fx.paths, &claude(), &fx.worktree, "main").unwrap();

        assert!(body.contains("PERSONAL"), "got:\n{body}");
        assert_eq!(report, Report::default());
    }
}
