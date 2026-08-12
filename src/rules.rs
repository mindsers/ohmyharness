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
