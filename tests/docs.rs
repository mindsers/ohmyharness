//! The documentation is a tree, not a file, and a tree rots differently.
//!
//! One `DESIGN.md` had exactly three inbound links and you could check them by
//! looking. Splitting it across `docs/` turns cross-references into the same
//! kind of unverified claim as an adapter path: plausible, cheap to write, and
//! silently wrong the moment a file is renamed. Nothing else in this repo
//! notices — a dead link does not fail a build, and the reader who finds it is
//! by definition the person who could least afford the detour.
//!
//! So the tree gets the treatment everything else here gets: an invariant with
//! a test that goes red without it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every markdown file in the repo, minus build output and anything git ignores
/// by convention. Walked rather than listed: a doc that is not walked is a doc
/// whose links are not checked, and the failure mode of a stale list is that it
/// silently stops covering new files.
fn markdown_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo(), &mut out);
    out.sort();
    out
}

/// Inline markdown links: `[text](target)`. Deliberately not a full parser —
/// it needs to find link targets, and anything it cannot parse it must skip
/// rather than guess at, since a false failure trains people to ignore it.
fn links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ']' && i + 1 < bytes.len() && bytes[i + 1] == '(' {
            let mut depth = 1;
            let mut j = i + 2;
            let mut target = String::new();
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                target.push(bytes[j]);
                j += 1;
            }
            if depth == 0 {
                // A link may carry a title: [t](path "Title"). Keep the path.
                let t = target.split_whitespace().next().unwrap_or("").to_string();
                out.push(t);
            }
            i = j;
        }
        i += 1;
    }
    out
}

fn is_external(target: &str) -> bool {
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with('#')
        || target.is_empty()
}

/// A relative link that points at nothing is a dead end for the one reader who
/// followed it, and nothing in the build notices.
#[test]
fn every_relative_link_resolves() {
    let mut dead: Vec<String> = Vec::new();

    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap();
        let shown = file.strip_prefix(repo()).unwrap().display().to_string();

        for target in links(&body) {
            if is_external(&target) {
                continue;
            }
            // Strip an anchor: docs/foo.md#section addresses a file plus a spot
            // in it, and only the file half is checkable here.
            let path_part = target.split('#').next().unwrap_or(&target);
            if path_part.is_empty() {
                continue;
            }
            if !dir.join(path_part).exists() {
                dead.push(format!("{shown} → {target}"));
            }
        }
    }

    assert!(dead.is_empty(), "dead links:\n  {}", dead.join("\n  "));
}

/// GitHub's heading slug: lowercase, drop punctuation, spaces become hyphens.
/// Inline markdown is stripped first — `` `omh bench` `` renders as a heading
/// whose anchor contains no backticks, and half the headings here are code.
fn slug(heading: &str) -> String {
    let text: String = heading
        .trim_start_matches('#')
        .trim()
        .chars()
        // `_` is emphasis syntax, but in these headings it is almost always an
        // identifier (`carry_in`, `idle_timeout`) and GitHub keeps it either way.
        .filter(|c| !matches!(c, '`' | '*' | '[' | ']' | '(' | ')'))
        .collect();
    // Underscores survive: GitHub strips punctuation but keeps `-` and `_`, so
    // a `carry_in` heading anchors at #carry_in, not #carryin.
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn anchors(body: &str) -> BTreeSet<String> {
    body.lines()
        .filter(|l| l.starts_with('#'))
        .map(slug)
        .collect()
}

/// An anchor is the half of a link the file check cannot see, and it is the
/// half that breaks during exactly the kind of edit that renames a section —
/// the link still resolves, so nothing complains, and the reader lands at the
/// top of a long page with no idea what they were meant to be looking at.
#[test]
fn every_anchor_resolves() {
    let mut dead: Vec<String> = Vec::new();

    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap();
        let shown = file.strip_prefix(repo()).unwrap().display().to_string();

        for target in links(&body) {
            if target.starts_with("http") || target.starts_with("mailto:") {
                continue;
            }
            let Some((path_part, anchor)) = target.split_once('#') else {
                continue;
            };
            if anchor.is_empty() {
                continue;
            }

            // A bare `#anchor` addresses this file; otherwise the target file.
            let target_body = if path_part.is_empty() {
                Some(body.clone())
            } else {
                fs::read_to_string(dir.join(path_part)).ok()
            };
            let Some(target_body) = target_body else {
                continue;
            }; // dead file: the other test owns that

            if !anchors(&target_body).contains(anchor) {
                dead.push(format!("{shown} → {target}"));
            }
        }
    }

    assert!(
        dead.is_empty(),
        "anchors pointing at no heading:\n  {}",
        dead.join("\n  ")
    );
}

/// A page nothing links to is a page nobody reads. When docs were one file that
/// was impossible by construction; in a tree it is the default outcome of
/// adding a file and forgetting the index, which is how documentation quietly
/// becomes a place things go to be lost.
#[test]
fn every_docs_page_is_reachable() {
    let docs = repo().join("docs");
    let mut linked: BTreeSet<PathBuf> = BTreeSet::new();

    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap();
        for target in links(&body) {
            if is_external(&target) {
                continue;
            }
            let path_part = target.split('#').next().unwrap_or(&target);
            if let Ok(c) = dir.join(path_part).canonicalize() {
                linked.insert(c);
            }
        }
    }

    let orphans: Vec<String> = markdown_files()
        .into_iter()
        .filter(|f| f.starts_with(&docs))
        .filter(|f| f.file_name().is_some_and(|n| n != "README.md"))
        .filter(|f| {
            !f.canonicalize()
                .map(|c| linked.contains(&c))
                .unwrap_or(false)
        })
        .map(|f| f.strip_prefix(repo()).unwrap().display().to_string())
        .collect();

    assert!(
        orphans.is_empty(),
        "unreachable pages:\n  {}",
        orphans.join("\n  ")
    );
}

/// The README is the entry point, so a link out of it that dies is the most
/// expensive one in the repo.
#[test]
fn the_readme_points_into_the_docs_tree() {
    let body = fs::read_to_string(repo().join("README.md")).unwrap();
    assert!(
        links(&body).iter().any(|l| l.starts_with("docs/")),
        "README should route readers into docs/"
    );
}
