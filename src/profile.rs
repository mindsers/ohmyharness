//! Profile resolution. Two layers: global (`~/.omh/profile`) then project
//! (`<repo>/.omh/profile`), project last so it wins.
//!
//! Nothing here is ever copied into your home directory. Layers are resolved to
//! a list of paths, and the launcher bind-mounts them. That is why there is no
//! drift to fight and no daemon to run.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Paths {
    pub root: PathBuf,
    pub repo: PathBuf,
}

impl Paths {
    pub fn discover(cwd: &Path) -> Result<Self> {
        let home = dirs::home_dir().context("no home directory")?;
        Ok(Self {
            root: home.join(".omh"),
            repo: repo_root(cwd)?,
        })
    }

    pub fn adapters(&self) -> PathBuf {
        self.root.join("adapters")
    }

    pub fn creds(&self, harness: &str) -> PathBuf {
        self.root.join("creds").join(harness)
    }

    pub fn worktrees(&self) -> PathBuf {
        self.repo.join(".omh/worktrees")
    }

    /// Cache volume name — keyed by repo, deliberately not by harness. This is
    /// what lets memory survive a harness switch.
    pub fn cache_volume(&self) -> String {
        format!("omh-cache-{}", self.repo_id())
    }

    pub fn network(&self) -> String {
        format!("omh-{}", self.repo_id())
    }

    fn repo_id(&self) -> String {
        self.repo
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".into())
    }

    /// Global first, project second. Only existing layers are returned.
    pub fn profile_layers(&self) -> Vec<PathBuf> {
        [self.root.join("profile"), self.repo.join(".omh/profile")]
            .into_iter()
            .filter(|p| p.exists())
            .collect()
    }
}

pub struct Profile {
    pub layers: Vec<PathBuf>,
}

impl Profile {
    pub fn resolve(paths: &Paths) -> Self {
        Self { layers: paths.profile_layers() }
    }

    /// Rules files, in application order. Concatenated at launch, global first.
    pub fn rules(&self) -> Vec<PathBuf> {
        self.each("AGENTS.md")
    }

    /// Skill directories. Unioned by folder name, later layers shadowing earlier.
    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        self.each("skills")
    }

    /// Canonical MCP server lists, merged by server name.
    pub fn mcp_files(&self) -> Vec<PathBuf> {
        self.each("mcp.json")
    }

    fn each(&self, name: &str) -> Vec<PathBuf> {
        self.layers
            .iter()
            .map(|l| l.join(name))
            .filter(|p| p.exists())
            .collect()
    }
}

/// Walk up looking for `.git`. The worktree model needs a real repo, so a
/// missing one is a hard error rather than a silent fallback to `cwd`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let mut cur = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        if !cur.pop() {
            anyhow::bail!(
                "{} is not inside a git repository\n\
                 omh isolates the agent on a worktree branch, which needs one.\n\
                 run `git init` first.",
                start.display()
            );
        }
    }
}
