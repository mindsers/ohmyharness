//! `omh upgrade` — the ongoing counterpart to `omh init`.
//!
//! `init` sets a repo up once; `upgrade` applies a newly-installed omh to a
//! machine that already ran it: it refreshes the managed catalogue from this
//! binary (keeping edits as `.yours`), asks the sandbox again which of this
//! repo's provides apply, rebuilds the images whose recipe moved, reaps the
//! superseded ones, advances the `seeded-by` stamp, and names any session still
//! running on an old image.

use crate::adapter::Adapter;
use crate::cmd::init::{
    ask_conditions, record_resolution, refresh_catalogue, sandbox_from, SEEDED_BY,
};
use crate::image::{self, Root};
use crate::profile::Paths;
use crate::report::{self, Outcome, Reprovisioned, StaleImage, StaleSession};
use crate::{config, out, runtime, session, stack};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Entry point. Discovers `Paths` itself — the dispatch hands it only the cwd,
/// the dry-run flag and the output context, never the whole `Cli`.
///
/// The counterpart to `omh init`, keyed on the same `seeded-by` stamp: `init`
/// runs when a repo is *not* set up, `upgrade` when it *is*. It refreshes the
/// managed catalogue from this binary (a pin that moved is what moves a tag),
/// asks the sandbox again which provides apply, rebuilds the base, harness and
/// this repo's stack images whose recipe moved, and advances the stamp so the
/// drift row goes quiet. A dry run decides all of that and reports it, but
/// builds, refreshes, records and stamps nothing.
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
    // Before anything is written. Settings that do not resolve stop this
    // command, and they are read again below once the table has been looked
    // at — by which time the catalogue has been refreshed, a container run and
    // a committed file rewritten. A refusal after all that reads as though
    // nothing happened.
    crate::cmd::session::resolved(&paths)?;

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
    //
    // The adapters classified against are the ones upgrade *would install*: on
    // a real run that is what it just refreshed to disk; on a dry run the disk
    // still holds the old pins, so it reads the new pins straight from the
    // binary — otherwise the preview would report `current` for exactly the
    // harnesses a real run rebuilds.
    let adapters = if dry_run {
        bundled_adapters()?
    } else {
        Adapter::load_dir(&paths.adapters())?
    };

    // The stacks, by the same rule and for the same reason: a real run has
    // just refreshed them onto disk, and a dry run reads what that refresh
    // would have written.
    let defs = if dry_run {
        stack::over(bundled_stacks()?, &paths.stacks(), &paths.repo_stacks())?
    } else {
        stack::load_all(&paths.stacks(), &paths.repo_stacks())?
    };

    // Ask again which provides apply, before anything reads the table — the
    // recipe every image below is classified and built from comes out of it.
    let (provision, table) = reprovision(
        &paths,
        &backend,
        ca.as_ref(),
        &adapters,
        &defs,
        dry_run,
        ctx,
    )?;
    // What a real run has just written into a committed file, for the one
    // case where the report below never prints: a build that fails after it.
    let wrote = match (&provision, dry_run) {
        (Reprovisioned::Asked { added, removed, .. }, false)
            if !(added.is_empty() && removed.is_empty()) =>
        {
            Some(format!(
                "`[provision]` in {} had already been rewritten when this failed ({})",
                config::Layer::Shared.file(&paths).display(),
                added
                    .iter()
                    .map(|key| format!("+ {key}"))
                    .chain(removed.iter().map(|key| format!("- {key}")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        _ => None,
    };
    let rest = || -> Result<Rebuilt> {
        let (_own, mut repo) = crate::cmd::session::resolved(&paths)?;
        if let Some(table) = table {
            // A dry run wrote nothing, so the resolution on disk is still the old
            // one. Classify against the table a real run would have written, as
            // the layers resolve it — the laptop's own file over the committed one
            // — or the preview reports `current` for a stack image a real run
            // rebuilds.
            let mut would_be = table;
            would_be.extend(config::read_provision(&paths, config::Layer::Local)?);
            repo.provision = would_be;
        }
        let mut current: BTreeSet<String> = BTreeSet::new();
        current.insert(image::base_tag(ca.as_ref().map(Root::pem)));
        let mut harnesses = Vec::new();
        for adapter in &adapters {
            let sandbox = sandbox_from(&paths, adapter, &repo, ca.clone(), &defs)?;
            let pem = sandbox.ca.as_ref().map(Root::pem);
            let recipe = sandbox.recipe();
            let tags = vec![
                image::base_tag(pem),
                image::tag_for(adapter, pem),
                image::stack_tag(adapter, &recipe, pem),
            ];
            let outcome = outcome_for(adapter.version.as_deref(), &tags, &|t| {
                image::exists(&backend, t)
            });
            harnesses.push((adapter.name.to_string(), outcome));

            // These tags are current whatever the outcome — an `Unpinnable`
            // adapter is left on the image it already had, so a session on it is
            // *not* stale. This has to happen before the `continue`, or that
            // session is falsely named "relaunch to move it" with nowhere to move.
            // And the owner layer over each, where a session on this host runs
            // one: without them every such session read as stale, for good.
            current.extend(image::with_owned(&tags, image::host_owner(&backend)?));
            if outcome == Outcome::Unpinnable {
                continue;
            }
            if !dry_run {
                // Builds base/harness/stack as needed and reaps the superseded.
                let tag = image::ensure_stack(&backend, adapter, &recipe, pem, &paths.repo)?;
                current.insert(tag);
            }
        }

        // Sessions still running on an image this upgrade did not just build. If
        // the runtime will not list what is running, that is not "nothing is
        // stale" — the check did not run, and the report says so rather than a
        // false all-clear.
        // A `worktrees/` omh cannot read is the same shape as a runtime that will
        // not answer, and this row already has the field for it: not `?`, because
        // the rest of the upgrade report is still worth printing, and not an empty
        // list either, which is the false all-clear the comment above refuses.
        let listed = session::list(&paths.worktrees());
        let (stale_sessions, sessions_unchecked) = match (image::running_set(&backend), listed) {
            (_, Err(e)) => (Vec::new(), Some(format!("{e:#}"))),
            (Err(why), _) => (Vec::new(), Some(why)),
            (Ok(up), Ok(ids)) => {
                let mut running = Vec::new();
                for id in ids {
                    let name = paths.container(&id);
                    if !up.contains(&name) {
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
                (sessions_on_old_images(&running, &current), None)
            }
        };

        // Advance the stamp so `omh doctor`'s drift row goes quiet — the same
        // write `init` does, and, like it, not `write_if_absent`: the stamp has to
        // move when omh does or it records the first omh that ever ran here.
        if !dry_run {
            std::fs::write(
                paths.repo.join(".omh").join(SEEDED_BY),
                format!("{}\n", env!("CARGO_PKG_VERSION")),
            )?;
        }

        Ok(Rebuilt {
            harnesses,
            stale_sessions,
            sessions_unchecked,
        })
    };
    let Rebuilt {
        harnesses,
        stale_sessions,
        sessions_unchecked,
    } = match (rest(), wrote) {
        (Ok(done), _) => done,
        (Err(e), Some(wrote)) => return Err(e.context(wrote)),
        (Err(e), None) => return Err(e),
    };

    ctx.say(&report::Upgraded {
        provision: Some(provision),
        harnesses,
        refreshed,
        stale_sessions,
        sessions_unchecked,
        dry_run,
    });
    Ok(())
}

/// What the part of an upgrade after the second look found: the outcome per
/// harness, and the sessions it left on an old image or could not check.
struct Rebuilt {
    harnesses: Vec<(String, Outcome)>,
    stale_sessions: Vec<StaleSession>,
    sessions_unchecked: Option<String>,
}

/// Ask the sandbox again which of this repo's provides apply, and record it.
///
/// `init` asks once and writes the answer into the repo's committed
/// `[provision]`, which every launch then builds from without asking. That is
/// right for a launch and wrong for ever: a `yarn.lock` swapped for a
/// `pnpm-lock.yaml` leaves the table describing a repo that no longer exists,
/// and `init`, being one-time, refuses to look again. This is the look.
///
/// The second value is the table a **dry run** would have written, and `None`
/// otherwise — after a real run the file holds it, and after a question that
/// could not be put there is no new table at all.
///
/// The conditions are asked in a harness image, as `init` asks them, and any
/// harness's will do, from any build: the ones omh ships read the repo with
/// the base image's tools, and nothing of the harness. One that is already
/// built is preferred — under the tag this omh gives it, or failing that under
/// whatever tag an earlier one did. A real run with none builds the first it
/// can, which the loop after this would have built anyway; a dry run with none
/// has nowhere to ask, and says so rather than guessing.
fn reprovision(
    paths: &Paths,
    backend: &runtime::Backend,
    ca: Option<&Root>,
    adapters: &[Adapter],
    defs: &[stack::Definition],
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<(Reprovisioned, Option<BTreeMap<String, bool>>)> {
    let pem = ca.map(Root::pem);
    let detected = stack::detected(defs, &paths.repo);
    let asking = detected.iter().any(|d| !d.provides.is_empty());

    // Nothing to ask is asked of nothing: `ask_conditions` starts no container
    // for it, so there is no image to find or build first.
    let tag = if !asking {
        String::new()
    } else if let Some(built) = adapters
        .iter()
        .map(|a| image::tag_for(a, pem))
        .find(|tag| image::exists(backend, tag))
        // Or one from before a pin moved. After a release every tag this omh
        // computes names an image nothing has built yet, on a machine with a
        // perfectly good one from yesterday.
        .or_else(|| adapters.iter().find_map(|a| image::built_for(backend, a)))
    {
        built
    } else if dry_run {
        return Ok((
            Reprovisioned::Unasked(
                "no harness image is built to ask in, and a dry run builds none".into(),
            ),
            None,
        ));
    } else {
        let Some(adapter) = adapters.iter().find(|a| a.version.is_some()) else {
            return Ok((
                Reprovisioned::Unasked(
                    "no harness image is built to ask in, and no harness here pins a \
                     version omh can build one from"
                        .into(),
                ),
                None,
            ));
        };
        ctx.progress(&format!(
            "building {} to ask what this repo needs…",
            image::tag_for(adapter, pem)
        ));
        image::ensure(backend, adapter, pem)?;
        image::tag_for(adapter, pem)
    };

    let asked = ask_conditions(backend, &tag, &paths.repo, &detected);
    if let Some(why) = asked.why_unasked() {
        return Ok((Reprovisioned::Unasked(why), None));
    }

    let shared = config::read_provision(paths, config::Layer::Shared)?;
    let look = stack::reread(&shared, &asked.verdicts());
    let would_be = if dry_run {
        Some(look.table.clone())
    } else {
        // Only when it differs. The write goes through a document editor and
        // keeps what somebody typed around the table, but a command that
        // touches a committed file on every run is one whose diff people stop
        // reading.
        if look.table != shared {
            record_resolution(paths, &look.standing)?;
        }
        None
    };
    Ok((
        Reprovisioned::Asked {
            added: look.added,
            removed: look.removed,
            unanswered: look.unanswered,
        },
        would_be,
    ))
}

/// The stacks as this binary ships them, read without writing anything — what
/// a real run's refresh would have put on disk before it asked.
fn bundled_stacks() -> Result<Vec<stack::Definition>> {
    crate::bundled::Shipped::Stacks
        .files()
        .iter()
        .map(|f| stack::parse(f.contents, Path::new(f.name)))
        .collect()
}

/// The adapters as this binary ships them — the new pins, read without writing
/// anything. `upgrade --dry-run` classifies against these so its preview
/// matches what a real run, which refreshes the same definitions to disk first,
/// would rebuild.
fn bundled_adapters() -> Result<Vec<Adapter>> {
    use anyhow::Context;
    crate::bundled::Shipped::Adapters
        .files()
        .iter()
        .map(|f| {
            let adapter: Adapter = toml::from_str(f.contents)
                .with_context(|| format!("parsing bundled adapter {}", f.name))?;
            // Same pin validation `Adapter::load` runs, so a preview cannot
            // read a version the real load would reject.
            adapter
                .install_command()
                .with_context(|| format!("bundled adapter {}", f.name))?;
            Ok(adapter)
        })
        .collect()
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
