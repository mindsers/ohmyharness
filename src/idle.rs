//! Stopping sessions nobody is using.
//!
//! N sessions is N containers, which `docs/design/risks.md` names as sandbox
//! sprawl and answers with `policy.idle_timeout`. The setting existed, resolved
//! through every layer, and was read by nothing — so a user who set it got
//! provenance for a value that did nothing.
//!
//! Only the **container** stops. The worktree and the branch survive, so
//! `omh sNN resume` restarts exactly where you left off: stopping an idle
//! session must never be able to lose work, which is the same rule `omh s rm`
//! follows for branches.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where a session records that somebody used it.
///
/// Touched on launch and on attach. This measures *engagement*, not the
/// agent's own file writes — a session left running after you walked away is
/// exactly what this exists to reap, and one where an agent is working
/// unattended is one you launched recently.
pub fn marker(run_dir: &Path, session: &str) -> PathBuf {
    run_dir.join(session).join("last-used")
}

pub fn touch(run_dir: &Path, session: &str) -> std::io::Result<()> {
    let path = marker(run_dir, session);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"")
}

/// `30m`, `2h`, `90s`, `1d`. Bare digits are seconds.
///
/// Returns `None` rather than erroring: this is read from a profile layer on
/// every launch, and a typo in `policy.toml` must not stop you working. The
/// caller says what it ignored.
pub fn parse_duration(raw: &str) -> Option<Duration> {
    let s = raw.trim().trim_matches('"');
    if s.is_empty() {
        return None;
    }
    let (digits, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = digits.parse().ok()?;
    let secs = match unit.trim() {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Sessions idle longer than `timeout`, given each one's last-used time.
///
/// Pure so the decision is testable without containers or a clock: the caller
/// supplies what is running and when each was last touched.
pub fn expired(
    running: &[(String, Option<SystemTime>)],
    timeout: Duration,
    now: SystemTime,
    keep: &str,
) -> Vec<String> {
    running
        .iter()
        // Never reap the session being launched — it is about to be used, and
        // its marker may not exist yet on the very first launch.
        .filter(|(id, _)| id != keep)
        .filter(|(_, last)| match last {
            // No marker means it predates this feature, or the run directory was
            // cleared. Left alone: stopping a container on a guess is worse than
            // one extra container.
            None => false,
            Some(t) => now
                .duration_since(*t)
                .map(|age| age > timeout)
                .unwrap_or(false),
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// When a session was last used, if it has ever been recorded.
pub fn last_used(run_dir: &Path, session: &str) -> Option<SystemTime> {
    std::fs::metadata(marker(run_dir, session))
        .ok()
        .and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_the_forms_people_write() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86_400)));
        // `omh settings set idle_timeout 30m` stores a bare string; the resolver
        // hands values back quoted from TOML.
        assert_eq!(parse_duration("\"30m\""), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration(" 45m "), Some(Duration::from_secs(2700)));
        assert_eq!(parse_duration("600"), Some(Duration::from_secs(600)));
    }

    /// A typo in a profile layer must not stop you working — every launch reads
    /// this, and refusing to start because `idle_timeout = "half an hour"` would
    /// be a worse failure than not reaping.
    #[test]
    fn an_unparseable_duration_is_ignored_rather_than_fatal() {
        for bad in ["half an hour", "m30", "", "0", "0m", "30x", "-5m"] {
            assert_eq!(parse_duration(bad), None, "{bad} should not parse");
        }
    }

    fn ago(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::now() - Duration::from_secs(secs))
    }

    #[test]
    fn only_sessions_past_the_timeout_are_reaped() {
        let now = SystemTime::now();
        let running = vec![
            ("s01".into(), ago(120)),  // fresh
            ("s02".into(), ago(7200)), // two hours idle
            ("s03".into(), ago(3540)), // just under the hour
        ];
        let out = expired(&running, Duration::from_secs(3600), now, "");
        assert_eq!(out, vec!["s02"]);
    }

    /// The session being launched is about to be used, and on a first launch its
    /// marker does not exist yet — reaping it would stop the container the user
    /// is currently starting.
    #[test]
    fn the_session_being_launched_is_never_reaped() {
        let now = SystemTime::now();
        let running = vec![("s01".into(), ago(99_999))];
        assert!(expired(&running, Duration::from_secs(60), now, "s01").is_empty());
    }

    /// A session with no marker predates this feature or had its run directory
    /// cleared. Stopping a container on a guess is worse than one extra
    /// container — the cost of a false positive is losing an agent mid-task.
    #[test]
    fn a_session_with_no_recorded_use_is_left_alone() {
        let now = SystemTime::now();
        let running = vec![("s01".into(), None)];
        assert!(expired(&running, Duration::from_secs(1), now, "").is_empty());
    }

    #[test]
    fn touching_a_session_records_a_time_that_reads_back() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(last_used(d.path(), "s01"), None);
        touch(d.path(), "s01").unwrap();
        let t = last_used(d.path(), "s01").expect("recorded");
        assert!(SystemTime::now().duration_since(t).unwrap() < Duration::from_secs(5));
    }
}
