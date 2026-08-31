//! `omh eject` — write out this repo's config for a harness, and step aside.
//!
//! The exit, and the argument for it is not technical. omh asks for your
//! rules, your credentials and your sandbox policy, and an opinionated tool
//! that cannot be left is a cage rather than a default. Being able to walk
//! away with the files is what makes adopting it a reversible decision.
//!
//! Nearly free to build, because omh already renders exactly these documents
//! on every launch. What is different here is only the destination: a launch
//! stages them for a container, mounts them read-only at guest paths and
//! links directories at layer paths that exist nowhere but inside the
//! sandbox. None of that survives being copied to a host. So eject renders
//! the same content through the same functions and writes **real files** at
//! the paths each adapter declares, mapped onto a directory of your choosing.
//!
//! It deliberately does not write into your checkout. The command exists to
//! *show* you what you would be keeping, and one that overwrote a working
//! tree by default would be the opposite of reassuring.

use crate::adapter::{Adapter, Binding, Capability, Render};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::{render, report};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Render every capability this profile carries into `to`.
pub(crate) fn eject(
    cwd: &Path,
    harness: &str,
    to: &Path,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    // Named rather than guessed. Every other harness-taking command refuses an
    // unknown name here, and an eject that quietly wrote an empty directory
    // would be discovered three commands later with nothing to explain it.
    let adapter = Adapter::find(&paths.adapters(), harness)
        .with_context(|| format!("`{harness}` is not a harness omh has an adapter for"))?;

    let (own, repo) = crate::cmd::session::resolved(&paths)?;

    // Composed exactly as a launch composes it, which is the point: the
    // project's own rules file is read and labelled with where it came from,
    // and the result is one document rather than a pile of layers the reader
    // has to merge in their head.
    let (rules_doc, _) = crate::rules::compose(
        &paths,
        &adapter,
        &paths.repo,
        None,
        &own.sections,
        &repo.selection,
    )?;

    let mut wrote = Vec::new();
    let mut dropped = Vec::new();
    let mut sandboxed = Vec::new();
    for cap in Capability::ALL {
        let Some(binding) = adapter.supports(cap) else {
            // An absent key means the harness cannot do that thing. Degradation
            // is a missing map entry rather than special-case logic, here as
            // everywhere else.
            dropped.push(cap.to_string());
            continue;
        };
        let sources = profile.sources(cap)?;
        let carries = match cap {
            // Rules are never "what the profile carries": the project's own
            // `AGENTS.md` is composed in, and on a fresh install it is the only
            // thing there. Asking the profile would eject an empty rules file
            // for the configuration most people start from.
            Capability::Rules => !rules_doc.trim().is_empty(),
            _ => !sources.is_empty(),
        };
        if !carries {
            continue;
        }

        for out_path in destinations(binding, to)? {
            match binding.render {
                // Copied, not linked. A launch links each entry at the layer
                // path it is mounted on, which resolves only inside the
                // container; on a host those links dangle, and a directory of
                // dangling links is worse than nothing because it looks like it
                // worked.
                Render::Dir => {
                    let n = copy_selected(&sources, cap, &repo, &out_path, dry_run)?;
                    if n > 0 {
                        wrote.push((cap, out_path.clone(), n));
                    }
                }
                Render::Concat => {
                    write_file(&out_path, &rules_doc, dry_run)?;
                    if names_a_sandbox_path(&rules_doc) {
                        sandboxed.push(out_path.display().to_string());
                    }
                    wrote.push((cap, out_path.clone(), 1));
                }
                _ => {
                    // Rendered even on a dry run, so a document that will not
                    // build is reported now rather than by the run that meant
                    // it — the same reasoning `Sandbox::stage` gives.
                    let doc = render::document(
                        cap,
                        binding,
                        &sources,
                        &own,
                        &repo,
                        &adapter.tools,
                        // Nothing has been measured, because nothing is being
                        // launched. An empty map suppresses nothing, which is
                        // the right default: a hook omh cannot prove the image
                        // lacks is one the user should see and decide about.
                        &Default::default(),
                    )?;
                    write_file(&out_path, &doc.body, dry_run)?;
                    if names_a_sandbox_path(&doc.body) {
                        sandboxed.push(out_path.display().to_string());
                    }
                    wrote.push((cap, out_path.clone(), 1));
                }
            }
        }
    }

    ctx.say(&report::Ejected {
        harness: adapter.name.clone(),
        to: to.display().to_string(),
        wrote: wrote
            .into_iter()
            .map(|(cap, at, n)| report::EjectedFile {
                capability: cap.to_string(),
                at: at.display().to_string(),
                entries: n,
            })
            .collect(),
        dropped,
        sandboxed,
        dry_run,
    });
    Ok(())
}

/// Does this document only work inside omh's sandbox?
///
/// The honest half of the exit. omh renders these for a container: the memory
/// server is invoked with `--local /omh/notes/local`, hooks read
/// `$OMH_GRAPH_PROJECT`, rules point at `/work`. On a host none of it
/// resolves — so handing somebody a directory and saying *these are yours
/// now* would be a thing that cannot be relied on, spelled exactly like a
/// thing that can.
///
/// **Named rather than rewritten.** omh does not know where you want your
/// notes, or whether you will run the harness in a container of your own, and
/// a guess written into a file you are about to depend on is worse than a
/// sentence telling you to look. The three prefixes are omh's own mount
/// points and its one environment variable; nothing else in a rendered
/// document is container-specific.
fn names_a_sandbox_path(body: &str) -> bool {
    ["/omh/", "/work/", "$OMH_"]
        .iter()
        .any(|marker| body.contains(marker))
}

/// Where a guest path lands under the eject root.
///
/// An adapter declares where the *container* expects a file — `/work/CLAUDE.md`,
/// `$HOME/.claude/skills`. Neither is a host path, and both have to become one
/// without losing which is which:
///
/// - `/work/…` is the checkout, so it lands at the root. These are the files
///   you would commit, or put beside the ones you already have.
/// - everything else is the harness's own home, so it lands under `home/`.
///   Kept apart deliberately: a reader has to be able to tell "this belongs in
///   my repo" from "this belongs in my dotfiles" without knowing omh's mount
///   layout, and flattening them together loses exactly that.
fn destinations(binding: &Binding, to: &Path) -> Result<Vec<PathBuf>> {
    std::iter::once(&binding.path)
        .chain(binding.also.iter())
        .map(|target| {
            let expanded = crate::adapter::expand(target, crate::image::GUEST_HOME);
            let s = expanded.to_string_lossy().to_string();
            Ok(match s.strip_prefix("/work/") {
                Some(rel) => to.join(rel),
                None => to.join("home").join(
                    s.strip_prefix(crate::image::GUEST_HOME)
                        .unwrap_or(&s)
                        .trim_start_matches('/'),
                ),
            })
        })
        .collect()
}

/// Copy the entries this repo has selected, and say how many.
///
/// The selection decides what is *offered*, exactly as it does at launch — so
/// an ejected directory holds what the harness would have been given, not the
/// whole catalogue. Ejecting everything would be a different and more
/// surprising claim: that leaving omh means taking every skill you have ever
/// written into this one project.
fn copy_selected(
    sources: &[PathBuf],
    cap: Capability,
    repo: &crate::settings::RepoPolicy,
    into: &Path,
    dry_run: bool,
) -> Result<usize> {
    let mut n = 0;
    for src in sources {
        let Ok(entries) = std::fs::read_dir(src) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !repo
                .selection
                .allows(cap, &crate::profile::entry_name(&name))
            {
                continue;
            }
            n += 1;
            if dry_run {
                continue;
            }
            std::fs::create_dir_all(into)?;
            let dst = into.join(&name);
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &dst)?;
            } else {
                std::fs::copy(entry.path(), &dst)
                    .with_context(|| format!("copying {}", entry.path().display()))?;
            }
        }
    }
    Ok(n)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let dst = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), &dst)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn write_file(at: &Path, body: &str, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(at, body).with_context(|| format!("writing {}", at.display()))
}
