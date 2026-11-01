//! A launch plan: what a sandbox is started from — its image, mounts,
//! environment and limits — as data, apart from the code that builds it
//! (`container`) and the code that runs it (`runtime`).

use crate::adapter::Capability;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Plan {
    pub image: String,
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub network: String,
    pub workdir: String,
    pub argv: Vec<String>,
    /// How much of the host the sandbox may take. Part of the plan, and so of
    /// the stamp: a limit changed in the settings is drift, and the sandbox
    /// restarts under the new one rather than running on under the old.
    pub limits: Limits,
    /// Capabilities the catalogue carries that this harness cannot express.
    pub dropped: Vec<(Capability, usize)>,
    /// Hooks this harness *nearly* expressed — it has the capability but not
    /// the moment, tool or payload field they asked for. Separate from
    /// `dropped` because the granularity differs: a capability is given up
    /// whole, a hook individually, and reporting the second as the first would
    /// say a harness has no hooks when it has all but one.
    pub dropped_hooks: Vec<crate::hook::Dropped>,
    /// What composing the project's rules turned up that the user should hear.
    pub rules: crate::rules::Report,
    /// What the harness's policy stops that this checkout or account carries,
    /// and each read that could not be made to say so (a line beginning "could
    /// not check"), to be said at launch — see [`crate::notice::policy`].
    pub policy: Vec<String>,
    /// Interactive harnesses need a terminal; a captured probe must not ask.
    pub tty: bool,
}

/// How much of the host the sandbox may take, from `sandbox_memory` and
/// `sandbox_cpus`. Each is passed to the runtime as written — `4g`, `1.5` —
/// because the runtime is the one that knows what it accepts, and `omh doctor`
/// is where a value it rejects gets found out. A write refuses only what no
/// runtime reads as a size or a count (`key::quarrel`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Limits {
    pub memory: Option<String>,
    pub cpus: Option<String>,
}

impl Limits {
    /// The stamp `Plan::labels` carries, so a changed limit reads as drift.
    pub fn stamp(&self) -> String {
        format!(
            "memory={}\ncpus={}",
            self.memory.as_deref().unwrap_or(""),
            self.cpus.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug)]
pub struct Mount {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub read_only: bool,
    /// A single file rather than a directory. Recorded at construction, never
    /// probed: under `Staging::Skip` the file does not exist yet, and a runtime
    /// capability check that changed answer between dry and real runs would be
    /// worse than no check.
    pub file: bool,
}

impl Plan {
    /// One line, once, naming what this harness cannot do.
    ///
    /// Two granularities, because there are two kinds of loss. A capability is
    /// given up whole and counting is enough — nobody needs the names of nine
    /// skills. A hook is given up one at a time, and a count would be a lie
    /// dressed as a summary: "hooks: 0" while three are missing. So a dropped
    /// hook is named, with the word it asked for, because a hook that was never
    /// installed behaves exactly like one that has nothing to say.
    pub fn degradation(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.dropped.is_empty() {
            let caps: Vec<_> = self
                .dropped
                .iter()
                .map(|(cap, n)| format!("{n} {cap}"))
                .collect();
            parts.push(format!("dropped {} (unsupported)", caps.join(", ")));
        }
        if !self.dropped_hooks.is_empty() {
            let hooks: Vec<_> = self
                .dropped_hooks
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            parts.push(format!("dropped hooks: {}", hooks.join(", ")));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}
