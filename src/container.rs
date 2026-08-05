//! Launch plan: stage the resolved profile, then bind-mount it onto whatever
//! paths the chosen harness happens to read.
//!
//! Docker cannot merge two host directories onto one mount point, so layered
//! profiles are materialized into a per-launch staging directory first. Note
//! that staging writes to `~/.omh/run/`, never into the harness's real config
//! location — the harness only ever sees a read-only mount, so there is still
//! nothing to drift and nothing to clean up.

use crate::adapter::{expand, Adapter};
use crate::profile::{Paths, Profile};
use crate::session::Session;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Plan {
    pub image: String,
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub network: String,
    pub workdir: String,
    pub argv: Vec<String>,
}

pub struct Mount {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub read_only: bool,
}

/// Home directory *inside* the container. Adapters template `$HOME` against it.
const GUEST_HOME: &str = "/home/agent";

pub fn plan(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    harness_args: &[String],
) -> Result<Plan> {
    let stage = stage_profile(paths, profile, adapter, session)?;

    let mut mounts = vec![
        // The agent's entire world. Never the host working tree.
        Mount { host: session.worktree.clone(), guest: "/work".into(), read_only: false },
        Mount {
            host: stage.join("skills"),
            guest: expand(&adapter.skills.path, GUEST_HOME),
            read_only: true,
        },
        Mount {
            host: stage.join("mcp"),
            guest: expand(&adapter.mcp.path, GUEST_HOME),
            read_only: true,
        },
    ];

    // Rules land inside /work, so they ride along with the worktree mount rather
    // than needing one of their own.
    let creds = paths.creds(&adapter.name);
    if creds.exists() {
        mounts.push(Mount {
            host: creds,
            guest: expand("$HOME/.config-creds", GUEST_HOME),
            read_only: true,
        });
    }

    Ok(Plan {
        image: format!("omh/{}:latest", adapter.name),
        mounts,
        env: vec![("OMH_SESSION".into(), session.id.clone())],
        network: paths.network(),
        workdir: "/work".into(),
        argv: std::iter::once(adapter.bin.clone())
            .chain(harness_args.iter().cloned())
            .collect(),
    })
}

/// Materialize the layered profile into `~/.omh/run/<session>/`.
fn stage_profile(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
) -> Result<PathBuf> {
    let stage = paths.root.join("run").join(&session.id);
    std::fs::create_dir_all(stage.join("skills"))?;

    // Rules: concatenate layers, global first, into the worktree itself so every
    // harness's expected filename can point at the same bytes.
    let rules: Vec<String> = profile
        .rules()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect();
    if !rules.is_empty() {
        let merged = rules.join("\n\n");
        for target in std::iter::once(&adapter.rules.path).chain(adapter.rules.also.iter()) {
            // Adapter paths are guest-absolute (`/work/CLAUDE.md`); rewrite onto
            // the host worktree that will be mounted there.
            if let Some(rel) = target.strip_prefix("/work/") {
                std::fs::write(session.worktree.join(rel), &merged)
                    .with_context(|| format!("staging rules at {rel}"))?;
            }
        }
    }

    // Skills: union by directory name, later layers shadowing earlier ones.
    for dir in profile.skill_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let link = stage.join("skills").join(entry.file_name());
            let _ = std::fs::remove_file(&link);
            symlink(&entry.path(), &link)?;
        }
    }

    // MCP: merge canonical lists by server name, then render for this harness.
    let servers = crate::mcp::merge(&profile.mcp_files())?;
    std::fs::write(stage.join("mcp"), crate::mcp::render(&servers, adapter.mcp.format)?)?;

    Ok(stage)
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst)
        .with_context(|| format!("linking {} -> {}", dst.display(), src.display()))
}

impl Plan {
    /// The exact `docker` invocation. Kept as a pure function so `--dry-run` and
    /// the real launch can never disagree.
    pub fn docker_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["run".into(), "--rm".into(), "-it".into()];
        for m in &self.mounts {
            a.push("-v".into());
            a.push(format!(
                "{}:{}{}",
                m.host.display(),
                m.guest.display(),
                if m.read_only { ":ro" } else { "" }
            ));
        }
        for (k, v) in &self.env {
            a.push("-e".into());
            a.push(format!("{k}={v}"));
        }
        a.push("--network".into());
        a.push(self.network.clone());
        a.push("-w".into());
        a.push(self.workdir.clone());
        a.push(self.image.clone());
        a.extend(self.argv.iter().cloned());
        a
    }
}
