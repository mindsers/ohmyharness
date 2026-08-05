//! `omh doctor` — the only thing that can validate an adapter.
//!
//! Adapters assert facts about *external software*: that Claude Code reads
//! `~/.mcp.json`, that opencode reads `~/.config/opencode/command`. A green unit
//! suite proves omh mounts a path faithfully; it proves nothing about whether
//! anything reads it. Until this command runs, every adapter path is an
//! unverified claim and the most likely place for omh to be confidently wrong.
//!
//! So doctor launches the real image with the real mounts and inspects the
//! **guest** paths the adapter declares. Checking anything host-side would test
//! the staging directory omh just wrote, which is circular.

use crate::adapter::{expand, Adapter, Capability, Render};
use crate::profile::Profile;
use std::path::PathBuf;

/// Home inside the sandbox. Mirrors `container::GUEST_HOME`.
const GUEST_HOME: &str = "/home/agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// The file exists and is not empty.
    NonEmptyFile,
    /// The file mentions each of these.
    Mentions(Vec<String>),
    /// The directory holds an entry for each of these.
    Entries(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub capability: Capability,
    /// Path **inside the sandbox**, never on the host.
    pub guest: PathBuf,
    pub expect: Expect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// What must be true inside the sandbox, given this profile and adapter.
pub fn checks(profile: &Profile, adapter: &Adapter) -> Vec<Check> {
    let mut out = Vec::new();
    for capability in Capability::ALL {
        let sources = profile.sources(capability);
        if sources.is_empty() {
            continue;
        }
        // A capability the harness cannot express was already reported as
        // dropped at launch; checking it would fail forever.
        let Some(binding) = adapter.supports(capability) else { continue };

        let guest = match binding.render {
            // `concat` writes into the worktree, which is mounted at /work.
            Render::Concat => PathBuf::from(&binding.path),
            _ => expand(&binding.path, GUEST_HOME),
        };

        let expect = match binding.render {
            Render::Concat => Expect::NonEmptyFile,
            Render::Dir => Expect::Entries(entry_names(&sources)),
            Render::McpJson | Render::CodexToml | Render::OpencodeJson => {
                Expect::Mentions(server_names(&sources))
            }
            Render::ClaudeSettings => Expect::NonEmptyFile,
        };

        out.push(Check { capability, guest, expect });
    }
    out
}

/// Union of entry names across layers — what the harness should be able to see.
fn entry_names(sources: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = sources
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| {
            entries
                .flatten()
                // The literal staged name. Stripping extensions would assert a
                // guess about how the harness names things instead of asserting
                // what omh actually mounted.
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn server_names(sources: &[PathBuf]) -> Vec<String> {
    crate::render::parse_layers(sources)
        .map(|servers| servers.into_keys().collect())
        .unwrap_or_default()
}

/// Shell run inside the sandbox. Emits one `ok|fail<TAB>name<TAB>detail` line
/// per check.
pub fn probe_script(checks: &[Check]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for check in checks {
        let path = check.guest.display();
        let name = check.capability;
        match &check.expect {
            Expect::NonEmptyFile => out.push_str(&format!(
                "if [ -s '{path}' ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                 else printf 'fail\\t{name}\\t{path} missing or empty\\n'; fi\n"
            )),
            Expect::Entries(names) => out.push_str(&format!(
                "missing=''; for n in {}; do [ -e '{path}'/\"$n\" ] || missing=\"$missing $n\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                 else printf 'fail\\t{name}\\tmissing:%s\\n' \"$missing\"; fi\n",
                shell_list(names)
            )),
            Expect::Mentions(names) => out.push_str(&format!(
                "missing=''; for n in {}; do grep -q \"$n\" '{path}' 2>/dev/null || missing=\"$missing $n\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                 else printf 'fail\\t{name}\\tmissing:%s\\n' \"$missing\"; fi\n",
                shell_list(names)
            )),
        }
    }
    out
}

fn shell_list(names: &[String]) -> String {
    if names.is_empty() {
        return "''".into();
    }
    names
        .iter()
        .map(|n| format!("'{}'", n.replace('\'', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse(output: &str) -> Vec<Outcome> {
    output
        .lines()
        .filter_map(|line| {
            // Anything that is not our protocol is runtime or harness noise,
            // and guessing at it would invent results.
            let mut parts = line.splitn(3, '\t');
            let status = parts.next()?;
            let name = parts.next()?;
            let detail = parts.next().unwrap_or("");
            let ok = match status {
                "ok" => true,
                "fail" => false,
                _ => return None,
            };
            Some(Outcome { name: name.to_string(), ok, detail: detail.to_string() })
        })
        .collect()
}

pub fn passed(outcomes: &[Outcome]) -> bool {
    !outcomes.is_empty() && outcomes.iter().all(|o| o.ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Paths;
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    struct Fx {
        _dir: tempfile::TempDir,
        profile: Profile,
    }

    fn fixture() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { root: dir.path().join("home"), repo: dir.path().join("repo") };
        let write = |p: PathBuf, body: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        let personal = paths.root.join("profile");
        write(personal.join("AGENTS.md"), "rules");
        write(personal.join("skills/graphify/SKILL.md"), "s");
        write(personal.join("subagents/explorer.md"), "a");
        write(
            personal.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"}}}"#,
        );
        Fx { _dir: dir, profile: Profile::resolve(&paths) }
    }

    fn adapter(name: &str) -> Adapter {
        Adapter::find(Path::new(ADAPTERS), name).unwrap()
    }

    #[test]
    fn every_declared_capability_is_checked() {
        let fx = fixture();
        let got: Vec<_> = checks(&fx.profile, &adapter("claude"))
            .into_iter()
            .map(|c| c.capability)
            .collect();
        assert_eq!(
            got,
            vec![Capability::Rules, Capability::Skills, Capability::Mcp, Capability::Subagents]
        );
    }

    /// A capability the harness cannot express is not a failure — it was
    /// already reported as dropped at launch. Checking it would fail forever.
    #[test]
    fn capabilities_the_harness_cannot_express_are_not_checked() {
        let fx = fixture();
        let caps: Vec<_> = checks(&fx.profile, &adapter("opencode"))
            .into_iter()
            .map(|c| c.capability)
            .collect();
        assert!(!caps.contains(&Capability::Subagents), "opencode has no subagents");
        assert!(caps.contains(&Capability::Skills));
    }

    /// The entire point: doctor must inspect where the *harness* looks, not
    /// where omh staged. Checking the host would be circular.
    #[test]
    fn checks_target_guest_paths_only() {
        let fx = fixture();
        for check in checks(&fx.profile, &adapter("claude")) {
            let p = check.guest.to_string_lossy().to_string();
            assert!(
                p.starts_with("/work") || p.starts_with(GUEST_HOME),
                "{p} is not a sandbox path"
            );
        }
    }

    #[test]
    fn content_checks_name_what_must_be_present() {
        let fx = fixture();
        let cs = checks(&fx.profile, &adapter("claude"));

        let skills = cs.iter().find(|c| c.capability == Capability::Skills).unwrap();
        assert_eq!(skills.expect, Expect::Entries(vec!["graphify".into()]));

        let mcp = cs.iter().find(|c| c.capability == Capability::Mcp).unwrap();
        assert_eq!(mcp.expect, Expect::Mentions(vec!["codegraph".into()]));
    }

    /// Regression: the check stripped `.md`, guessing at how a harness names a
    /// command. omh stages the literal filename, so doctor must assert what omh
    /// actually did — a check that tests a guess reports failures that are not
    /// real and hides ones that are.
    #[test]
    fn entries_are_checked_under_the_name_omh_staged() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("ship.md"), "x").unwrap();
        std::fs::create_dir_all(commands.join("nested")).unwrap();

        assert_eq!(entry_names(&[commands]), vec!["nested".to_string(), "ship.md".to_string()]);
    }

    // ── probe ───────────────────────────────────────────────────────────────

    #[test]
    fn the_probe_reports_one_line_per_check() {
        let fx = fixture();
        let cs = checks(&fx.profile, &adapter("claude"));
        let script = probe_script(&cs);
        for c in &cs {
            assert!(
                script.contains(&c.guest.to_string_lossy().to_string()),
                "probe never looks at {:?}",
                c.guest
            );
        }
    }

    #[test]
    fn probe_output_parses_into_outcomes() {
        let out = "ok\trules\t/work/CLAUDE.md\nfail\tmcp\tmissing codegraph\n";
        let parsed = parse(out);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].ok);
        assert_eq!(parsed[0].name, "rules");
        assert!(!parsed[1].ok);
        assert_eq!(parsed[1].detail, "missing codegraph");
    }

    /// Noise from the harness or the runtime must not be mistaken for results.
    #[test]
    fn unrecognised_lines_are_ignored_not_guessed_at() {
        let parsed = parse("Unable to find image\nok\trules\t/work/CLAUDE.md\nrandom noise\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "rules");
    }

    /// A probe that produced nothing means the container never ran the script.
    /// Reporting that as a pass would make doctor worse than useless.
    #[test]
    fn no_output_is_never_a_pass() {
        assert!(!passed(&parse("")));
    }

    #[test]
    fn a_single_failure_fails_the_verdict() {
        let outcomes = parse("ok\ta\t-\nfail\tb\tmissing\n");
        assert!(!passed(&outcomes));
    }

    #[test]
    fn all_ok_passes() {
        assert!(passed(&parse("ok\ta\t-\nok\tb\t-\n")));
    }
}
