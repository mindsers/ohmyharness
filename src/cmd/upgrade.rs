//! `omh upgrade` — the ongoing counterpart to `omh init`.
//!
//! `init` sets a repo up once; `upgrade` applies a newly-installed omh to a
//! machine that already ran it: it refreshes the managed catalogue from this
//! binary (keeping edits as `.yours`), rebuilds the images whose recipe moved,
//! reaps the superseded ones, advances the `seeded-by` stamp, and names any
//! session still running on an old image.

use crate::out;
use crate::report::{Outcome, StaleImage, StaleSession};
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// Entry point. Discovers `Paths` itself — the dispatch hands it only the cwd,
/// the dry-run flag and the output context, never the whole `Cli`.
pub(crate) fn upgrade_cmd(cwd: &Path, dry_run: bool, ctx: &out::Ctx) -> Result<()> {
    // Filled in by the driver increment. Wiring lands first so the command is
    // reachable and classified before it does any work.
    let _ = (cwd, dry_run, ctx);
    anyhow::bail!("upgrade is not implemented yet")
}

/// What `upgrade` will do to one adapter's image, decided before building.
///
/// An adapter that pins no version (`version` is `None`) is `Unpinnable` — omh
/// cannot rebuild it to anything reproducible, so it is left alone whatever its
/// tags say. A pinned adapter whose every layer tag (base, harness, this repo's
/// stack) already exists is `AlreadyCurrent`; if any is missing, a build will
/// run, so it is `Rebuilt`. `exists` is injected so this is decided without a
/// runtime; `version` is the only adapter field that decides it.
#[allow(dead_code)] // Called by `upgrade_cmd`; the allow goes when the driver lands.
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
#[allow(dead_code)] // Called by `upgrade_cmd`; the allow goes when the driver lands.
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
