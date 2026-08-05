//! Session persistence.
//!
//! A long-lived sandbox is not a long-lived *session*. `exec`ing a harness ties
//! its lifetime to your terminal: close the lid and the agent is hung up on,
//! mid-task, while the container keeps running around the corpse.
//!
//! `dtach` fixes that in about a thousand lines. It is deliberately not tmux:
//! omh needs detach/reattach, not multiplexing — SSH already provides the
//! second — and tmux's prefix key, nesting behaviour, and extra translation
//! layer all land squarely on the harness TUIs we care most about not breaking.
//!
//! Some harnesses ship their own resume. Relying on that would be exactly the
//! per-harness behaviour omh exists to abstract away.

use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Reattach if a session is already running, otherwise start one.
    Dtach,
    /// Bind the harness to this terminal's lifetime.
    None,
}

impl Mode {
    pub const NAMES: [&'static str; 2] = ["dtach", "none"];
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "dtach" => Ok(Self::Dtach),
            "none" => Ok(Self::None),
            other => anyhow::bail!(
                "unknown persistence mode `{other}` — expected one of: {}",
                Mode::NAMES.join(", ")
            ),
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dtach => "dtach",
            Self::None => "none",
        })
    }
}

/// Socket path *inside* the sandbox. Kept under `/omh` so it never collides
/// with the workspace, and a pure function of its inputs so a second
/// `omh <harness>` reattaches instead of starting a second agent.
pub fn socket(session: &str, harness: &str) -> PathBuf {
    PathBuf::from(format!("/omh/sock/{session}-{harness}"))
}

/// Wrap the harness invocation so it survives losing the terminal.
pub fn wrap(mode: Mode, session: &str, harness: &str, argv: Vec<String>) -> Vec<String> {
    match mode {
        Mode::None => argv,
        Mode::Dtach => {
            let mut out = vec![
                "dtach".to_string(),
                "-A".to_string(),
                socket(session, harness).to_string_lossy().into_owned(),
            ];
            out.extend(argv);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn argv() -> Vec<String> {
        vec!["claude".into()]
    }

    #[test]
    fn dtach_attaches_or_creates_rather_than_always_starting_fresh() {
        let out = wrap(Mode::Dtach, "s01", "claude", argv());
        assert_eq!(out[0], "dtach");
        assert!(
            out.contains(&"-A".to_string()),
            "-A is what makes a second `omh claude` reattach instead of starting a second agent: {out:?}"
        );
        assert_eq!(out.last().unwrap(), "claude");
    }

    #[test]
    fn none_leaves_the_command_untouched() {
        assert_eq!(wrap(Mode::None, "s01", "claude", argv()), argv());
    }

    #[test]
    fn harness_arguments_survive_wrapping() {
        let out = wrap(
            Mode::Dtach,
            "s01",
            "claude",
            vec!["claude".into(), "--resume".into(), "abc".into()],
        );
        let tail: Vec<_> = out.iter().rev().take(3).rev().cloned().collect();
        assert_eq!(tail, ["claude", "--resume", "abc"]);
    }

    /// Reattaching is only possible if the socket is a pure function of the
    /// session and harness — a path with anything variable in it would silently
    /// start a second agent every time.
    #[test]
    fn the_socket_is_deterministic() {
        assert_eq!(socket("s01", "claude"), socket("s01", "claude"));
    }

    #[test]
    fn sockets_are_per_session_and_per_harness() {
        assert_ne!(socket("s01", "claude"), socket("s02", "claude"));
        assert_ne!(socket("s01", "claude"), socket("s01", "opencode"));
    }

    #[test]
    fn the_socket_lives_inside_the_sandbox() {
        let s = socket("s01", "claude");
        assert!(s.is_absolute(), "guest path: {s:?}");
        assert!(s.starts_with("/omh"), "must not collide with the workspace: {s:?}");
    }

    #[test]
    fn modes_round_trip_through_their_names() {
        for mode in [Mode::Dtach, Mode::None] {
            assert_eq!(Mode::from_str(&mode.to_string()).unwrap(), mode);
        }
    }

    #[test]
    fn an_unknown_mode_lists_the_real_ones() {
        let err = Mode::from_str("tmux").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("dtach") && msg.contains("none"), "got: {msg}");
    }
}
