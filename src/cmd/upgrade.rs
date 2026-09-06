//! `omh upgrade` — the ongoing counterpart to `omh init`.
//!
//! `init` sets a repo up once; `upgrade` applies a newly-installed omh to a
//! machine that already ran it: it refreshes the managed catalogue from this
//! binary (keeping edits as `.yours`), rebuilds the images whose recipe moved,
//! reaps the superseded ones, advances the `seeded-by` stamp, and names any
//! session still running on an old image.

use crate::out;
use anyhow::Result;
use std::path::Path;

/// Entry point. Discovers `Paths` itself — the dispatch hands it only the cwd,
/// the dry-run flag and the output context, never the whole `Cli`.
pub(crate) fn upgrade_cmd(cwd: &Path, dry_run: bool, ctx: &out::Ctx) -> Result<()> {
    // Filled in by the driver increment. Wiring lands first so the command is
    // reachable and classified before it does any work.
    let _ = (cwd, dry_run, ctx);
    anyhow::bail!("upgrade is not implemented yet")
}
