//! One session's run directory, `~/.omh/run/<repo id>/<session>`, and the name
//! of everything omh keeps in it — so the code that writes a file there and
//! the code that reads it back name it one way.
//!
//! Its children are named after harnesses where they are a harness's staged
//! catalogue (`<harness>/`), so what else omh keeps here carries a `.`, which
//! no adapter name can: `.harness` beside an adapter called `harness` would be
//! a file where a directory is about to go. `transcripts`, `last-used` and
//! `port` are the ones that do not, and an adapter given one of those names
//! would collide with it.

use std::path::{Path, PathBuf};

/// One session's run directory.
pub struct Run(PathBuf);

impl Run {
    /// Session `session`'s, under `runs`, a repo's `Paths::runs`.
    pub fn at(runs: &Path, session: &str) -> Run {
        Run(runs.join(session))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The catalogue staged for `harness`'s launches.
    pub fn staging(&self, harness: &str) -> PathBuf {
        self.0.join(harness)
    }

    /// What the running container was started from, compared at the next
    /// launch to tell whether it drifted.
    pub fn stamp(&self) -> PathBuf {
        self.0.join("stamp.json")
    }

    /// The outcome of the checks `omh sNN commit` last ran.
    pub fn check(&self) -> PathBuf {
        self.0.join("check.json")
    }

    /// The harness's transcripts, mounted into the sandbox.
    pub fn transcripts(&self) -> PathBuf {
        self.0.join("transcripts")
    }

    /// The session's own copies of `harness`'s account files.
    pub fn home(&self, harness: &str) -> PathBuf {
        self.0.join(".home").join(harness)
    }

    /// Every harness's preferences home, one directory each.
    pub fn preferences_root(&self) -> PathBuf {
        self.0.join(".preferences")
    }

    /// `harness`'s preferences home.
    pub fn preferences(&self, harness: &str) -> PathBuf {
        self.preferences_root().join(harness)
    }

    /// When somebody last used the session.
    pub fn last_used(&self) -> PathBuf {
        self.0.join("last-used")
    }

    /// The harness the session last ran.
    pub fn harness(&self) -> PathBuf {
        self.0.join(".harness")
    }

    /// The host port the session's ssh answers on, chosen at its first launch.
    pub fn port(&self) -> PathBuf {
        self.0.join("port")
    }
}

#[cfg(test)]
mod tests {
    /// Each name is spelled here and nowhere else in what ships. A writer and
    /// a reader that each spell a file's name are two answers to where it is,
    /// and the day they differ the reader finds nothing and says so as if
    /// nothing had been written.
    #[test]
    fn every_name_in_a_run_directory_is_spelled_here_alone() {
        let names = [
            "\"stamp.json\"",
            "\"check.json\"",
            "\"transcripts\"",
            "\".home\"",
            "\".preferences\"",
            "\"last-used\"",
            "\".harness\"",
            // A JSON key elsewhere is also called `port`; the file is a join.
            "join(\"port\")",
        ];
        for name in names {
            let mut at = Vec::new();
            for (path, production) in crate::testsrc::production() {
                for (i, line) in production.lines().enumerate() {
                    if line.contains(name) {
                        let file = path.file_name().unwrap().to_string_lossy().to_string();
                        at.push(format!("{file}:{}", i + 1));
                    }
                }
            }
            assert!(
                at.len() == 1 && at[0].starts_with("rundir.rs:"),
                "{name} is spelled at {at:?}, and may be only in rundir.rs"
            );
        }
    }
}
