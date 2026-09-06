//! `omh upgrade` — the ongoing counterpart to `omh init`.
//!
//! `init` sets a repo up once; `upgrade` applies a newly-installed omh to a
//! machine that already ran it: it refreshes the managed catalogue from this
//! binary (keeping edits as `.yours`), rebuilds the images whose recipe moved,
//! reaps the superseded ones, advances the `seeded-by` stamp, and names any
//! session still running on an old image.

use crate::adapter::Adapter;
use crate::cmd::init::{refresh_catalogue, sandbox, SEEDED_BY};
use crate::image::{self, Root};
use crate::profile::Paths;
use crate::report::{self, Outcome, StaleImage, StaleSession};
use crate::{out, runtime, session};
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// Entry point. Discovers `Paths` itself — the dispatch hands it only the cwd,
/// the dry-run flag and the output context, never the whole `Cli`.
///
/// The counterpart to `omh init`, keyed on the same `seeded-by` stamp: `init`
/// runs when a repo is *not* set up, `upgrade` when it *is*. It refreshes the
/// managed catalogue from this binary (a pin that moved is what moves a tag),
/// rebuilds the base, harness and this repo's stack images whose recipe moved,
/// and advances the stamp so the drift row goes quiet. A dry run decides all of
/// that and reports it, but builds, refreshes and stamps nothing.
pub(crate) fn upgrade_cmd(cwd: &Path, dry_run: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    // Symmetric with `init`'s refusal: upgrade updates a repo already set up,
    // and a repo with no stamp is `omh init`'s to set up first.
    anyhow::ensure!(
        paths.repo.join(".omh").join(SEEDED_BY).exists(),
        "this repo is not set up yet — run `omh init` first"
    );

    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    let ca = image::ca_for(&paths)?;
    let (_own, repo) = crate::cmd::session::resolved(&paths)?;

    // Refresh first — moving the pins is what moves the tags. A dry run reads
    // the catalogue as it stands and writes nothing.
    let refreshed = if dry_run {
        Vec::new()
    } else {
        refresh_catalogue(&paths, ctx)?
    };

    // Classify — and, on a real run, rebuild — each installed adapter, and
    // gather every current tag so a running session on any other is named
    // below.
    let mut current: BTreeSet<String> = BTreeSet::new();
    current.insert(image::base_tag(ca.as_ref().map(Root::pem)));
    let mut harnesses = Vec::new();
    for adapter in Adapter::load_dir(&paths.adapters())? {
        let sandbox = sandbox(&paths, &adapter, &repo, ca.clone())?;
        let pem = sandbox.ca.as_ref().map(Root::pem);
        let recipe = sandbox.recipe();
        let tags = vec![
            image::base_tag(pem),
            image::tag_for(&adapter, pem),
            image::stack_tag(&adapter, &recipe, pem),
        ];
        let outcome = outcome_for(adapter.version.as_deref(), &tags, &|t| {
            image::exists(&backend, t)
        });
        harnesses.push((adapter.name.clone(), outcome));

        // An unpinnable adapter is left alone — omh cannot rebuild it to
        // anything reproducible, and its image is whatever it already was.
        if outcome == Outcome::Unpinnable {
            continue;
        }
        current.extend(tags);
        if !dry_run {
            // Builds base/harness/stack as needed and reaps the superseded.
            let tag = image::ensure_stack(&backend, &adapter, &recipe, pem, &paths.repo)?;
            current.insert(tag);
        }
    }

    // Sessions still running on an image this upgrade did not just build.
    let up = image::running_set(&backend);
    let mut running = Vec::new();
    for id in session::list(&paths.worktrees()) {
        let name = paths.container(&id);
        if !matches!(image::running_in(&up, &name), image::Running::Yes) {
            continue;
        }
        let image = match image::container_stamp(&backend, &name) {
            image::Stamp::Read(labels) => labels
                .get("omh.image")
                .cloned()
                .ok_or_else(|| "the sandbox records no image".to_string()),
            image::Stamp::Unknown(why) => Err(why),
        };
        running.push((id, image));
    }
    let stale_sessions = sessions_on_old_images(&running, &current);

    // Advance the stamp so `omh doctor`'s drift row goes quiet — the same
    // write `init` does, and, like it, not `write_if_absent`: the stamp has to
    // move when omh does or it records the first omh that ever ran here.
    if !dry_run {
        std::fs::write(
            paths.repo.join(".omh").join(SEEDED_BY),
            format!("{}\n", env!("CARGO_PKG_VERSION")),
        )?;
    }

    ctx.say(&report::Upgraded {
        harnesses,
        refreshed,
        stale_sessions,
        dry_run,
    });
    Ok(())
}

/// What `upgrade` will do to one adapter's image, decided before building.
///
/// An adapter that pins no version (`version` is `None`) is `Unpinnable` — omh
/// cannot rebuild it to anything reproducible, so it is left alone whatever its
/// tags say. A pinned adapter whose every layer tag (base, harness, this repo's
/// stack) already exists is `AlreadyCurrent`; if any is missing, a build will
/// run, so it is `Rebuilt`. `exists` is injected so this is decided without a
/// runtime; `version` is the only adapter field that decides it.
pub(crate) fn outcome_for(
    version: Option<&str>,
    tags: &[String],
    exists: &dyn Fn(&str) -> bool,
) -> Outcome {
    if version.is_none() {
        return Outcome::Unpinnable;
    }
    if tags.iter().all(|t| exists(t)) {
        Outcome::AlreadyCurrent
    } else {
        Outcome::Rebuilt
    }
}

/// Name the running sessions left on an image this upgrade did not just build.
///
/// `running` pairs each session id with what omh read as its image: `Ok(tag)`
/// when it read one, `Err(reason)` when it could not. A session on a tag in
/// `current` is fine and skipped; a session on any other tag is named as
/// stale; and a session omh could not read is named as **uncertain**, never
/// dropped — an unreadable image is not evidence the session is current, and
/// treating it as such is the false-negative the runtime layer refuses.
pub(crate) fn sessions_on_old_images(
    running: &[(String, Result<String, String>)],
    current: &BTreeSet<String>,
) -> Vec<StaleSession> {
    running
        .iter()
        .filter_map(|(id, image)| match image {
            Ok(tag) if current.contains(tag) => None,
            Ok(tag) => Some(StaleSession {
                id: id.clone(),
                image: StaleImage::Known(tag.clone()),
            }),
            Err(why) => Some(StaleSession {
                id: id.clone(),
                image: StaleImage::Unknown(why.clone()),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session on a current image is left out; one on any other tag is named
    /// stale; and one omh could not read is named uncertain rather than dropped.
    #[test]
    fn only_sessions_not_on_a_current_image_are_named() {
        let current: BTreeSet<String> = ["omh/claude:new".to_string(), "omh/base:new".to_string()]
            .into_iter()
            .collect();
        let running = vec![
            ("s01".to_string(), Ok("omh/claude:new".to_string())), // current — skipped
            ("s02".to_string(), Ok("omh/claude:old".to_string())), // stale — named
            ("s03".to_string(), Err("daemon did not answer".to_string())), // uncertain — named
        ];

        let named = sessions_on_old_images(&running, &current);
        assert_eq!(named.len(), 2, "the current one is not named: {named:?}");
        assert!(named
            .iter()
            .any(|s| s.id == "s02"
                && matches!(&s.image, StaleImage::Known(t) if t == "omh/claude:old")));
        assert!(named
            .iter()
            .any(|s| s.id == "s03" && matches!(&s.image, StaleImage::Unknown(_))));
    }

    /// An unpinned adapter cannot be rebuilt reproducibly, so it is `Unpinnable`
    /// no matter what its tags say; a pinned one is `AlreadyCurrent` only when
    /// every layer already exists, and `Rebuilt` the moment one is missing.
    #[test]
    fn the_outcome_is_unpinnable_current_or_rebuilt() {
        let tags = vec!["omh/base:x".to_string(), "omh/claude:y".to_string()];

        // Unpinnable ignores the tags entirely.
        let all_present = |_: &str| true;
        assert_eq!(outcome_for(None, &tags, &all_present), Outcome::Unpinnable);

        // Pinned + every tag present → current.
        assert_eq!(
            outcome_for(Some("2.0"), &tags, &all_present),
            Outcome::AlreadyCurrent
        );

        // Pinned + one tag missing → rebuilt.
        let missing_harness = |t: &str| t != "omh/claude:y";
        assert_eq!(
            outcome_for(Some("2.0"), &tags, &missing_harness),
            Outcome::Rebuilt
        );
    }
}
