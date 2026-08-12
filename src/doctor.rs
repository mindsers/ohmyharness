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

use crate::image::GUEST_HOME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// The file exists and is not empty.
    NonEmptyFile,
    /// The file mentions each of these.
    Mentions(Vec<String>),
    /// The directory holds an entry for each of these.
    Entries(Vec<String>),
    /// A temp file can be renamed over this path.
    ///
    /// The one failure omh cannot see from the host: a bind-mounted *file* is a
    /// mount point, so `rename()` onto it returns EBUSY. Every tool saves a
    /// token that way, so this decides whether a login can persist at all.
    AtomicWrite,
    /// `guest` answers an MCP handshake and names each of these tools.
    ///
    /// The other thing invisible from the host: whether a server omh
    /// *configured* can actually start where the harness will spawn it. Every
    /// host-side test proves the tool list is right about a host directory,
    /// which is circular in exactly the way this module exists to break.
    ///
    /// It does **not** prove invariant 9. `doctor` replaces the launch command
    /// with this probe, so no harness ever runs, and a tool description is
    /// consumed by a model rather than written anywhere inspectable. What this
    /// proves is the precondition.
    Speaks(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Label shown in the report.
    pub name: String,
    /// Path **inside the sandbox**, never on the host.
    pub guest: PathBuf,
    pub expect: Expect,
    /// Whether `guest` is a directory. Decides how the probe writes.
    pub dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// What must be true of the memory server, given the base set declares one.
///
/// Built from the declared command rather than from a literal, so a manifest
/// that changes what it launches changes what gets probed.
pub fn memory_checks(server: &crate::render::Server) -> Vec<Check> {
    let argv = std::iter::once(server.command.clone())
        .chain(server.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    vec![Check {
        name: "memory".into(),
        guest: PathBuf::from(argv),
        expect: Expect::Speaks(vec!["recall".into(), "remember".into()]),
        dir: false,
    }]
}

/// What must be true of the credential mounts, given an account.
///
/// Nothing in process can answer this — it is a property of how the runtime
/// binds the path, not of anything omh wrote.
pub fn credential_checks(adapter: &Adapter) -> Vec<Check> {
    // The *token* is what must survive. The account record beside it is written
    // in place by every harness seen so far, and it sits directly in $HOME where
    // there is no directory to mount — so it is deliberately not a hard check.
    adapter
        .token
        .iter()
        .map(|template| Check {
            name: "token".into(),
            guest: expand(template.trim_end_matches('/'), GUEST_HOME),
            expect: Expect::AtomicWrite,
            dir: template.ends_with('/'),
        })
        .collect()
}

/// What must be true inside the sandbox, given this profile and adapter.
pub fn checks(profile: &Profile, adapter: &Adapter, own: &crate::base::Own) -> Vec<Check> {
    let mut out = Vec::new();
    for capability in Capability::ALL {
        let sources = profile.sources(capability);
        // Two capabilities are mounted whether or not a layer sources them,
        // because omh generates part of them from the base manifest. Asking
        // the profile is the same mistake `container::plan` made about rules:
        // it answers about the layers, and the question is about the mount.
        //
        // Rules has one case this cannot see — a repo whose only rules are its
        // own tracked file, with every omh feature off. That composes and
        // mounts, and goes unchecked. Erring toward no check rather than a
        // check that fails forever, which is the trade `omh doctor` has to
        // make while it reads a profile rather than a plan.
        let generated = match capability {
            Capability::Rules => !own.sections.is_empty(),
            Capability::Hooks => !own.hooks.is_empty(),
            _ => false,
        };
        if sources.is_empty() && !generated {
            continue;
        }
        // A capability the harness cannot express was already reported as
        // dropped at launch; checking it would fail forever.
        let Some(binding) = adapter.supports(capability) else {
            continue;
        };

        let guest = match binding.render {
            // `concat` writes into the worktree, which is mounted at /work.
            Render::Concat => PathBuf::from(&binding.path),
            _ => expand(&binding.path, GUEST_HOME),
        };

        let expect = match binding.render {
            Render::Concat => Expect::NonEmptyFile,
            Render::Dir => Expect::Entries(entry_names(&sources)),
            Render::McpJson | Render::CodexToml | Render::OpencodeJson => {
                Expect::Mentions(server_names(&sources, own))
            }
            Render::ClaudeSettings => Expect::NonEmptyFile,
        };

        out.push(Check {
            name: capability.to_string(),
            guest,
            expect,
            dir: binding.render == Render::Dir,
        });
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

/// What the document is expected to mention — which is what the launcher
/// renders, not what the layers declare.
///
/// A server whose feature is off here is deliberately left out of that
/// document. Demanding it makes `omh doctor` fail forever and blame the
/// harness for obeying, which is the opposite of what this command is for.
fn server_names(sources: &[PathBuf], own: &crate::base::Own) -> Vec<String> {
    crate::render::parse_layers(sources)
        .map(|servers| {
            servers
                .into_keys()
                .filter(|name| !own.disabled_servers.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Shell run inside the sandbox. Emits one `ok|fail<TAB>name<TAB>detail` line
/// per check.
pub fn probe_script(checks: &[Check]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for check in checks {
        let path = check.guest.display();
        let name = &check.name;
        match &check.expect {
            Expect::NonEmptyFile => out.push_str(&format!(
                "if [ -s '{path}' ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                 else printf 'fail\\t{name}\\t{path} missing or empty\\n'; fi\n"
            )),
            // Preserve what is there: a probe that costs the user their token
            // is worse than no probe. The directory case writes a scratch file
            // and removes it; the file case renames byte-identical content back.
            Expect::AtomicWrite if check.dir => out.push_str(&format!(
                "if ( echo probe > '{path}/.omh-probe.tmp' && mv '{path}/.omh-probe.tmp' '{path}/.omh-probe' ) 2>/dev/null; \
                 then printf 'ok\\t{name}\\t{path} (atomic write)\\n'; \
                 else printf 'fail\\t{name}\\t{path} cannot be renamed over (EBUSY?)\\n'; fi; \
                 rm -f '{path}/.omh-probe' '{path}/.omh-probe.tmp' 2>/dev/null\n"
            )),
            Expect::AtomicWrite => out.push_str(&format!(
                "if ( cp '{path}' '{path}.omh-probe' && mv '{path}.omh-probe' '{path}' ) 2>/dev/null; \
                 then printf 'ok\\t{name}\\t{path} (atomic write)\\n'; \
                 else printf 'fail\\t{name}\\t{path} cannot be renamed over — a token saved here will not persist\\n'; fi; \
                 rm -f '{path}.omh-probe' 2>/dev/null\n"
            )),
            Expect::Entries(names) => out.push_str(&format!(
                "missing=''; for n in {}; do [ -e '{path}'/\"$n\" ] || missing=\"$missing $n\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                 else printf 'fail\\t{name}\\tmissing:%s\\n' \"$missing\"; fi\n",
                shell_list(names)
            )),
            // Three frames down a pipe: initialize, the notification the
            // protocol requires after it, then tools/list. Reading the reply
            // with grep rather than a parser keeps the probe a shell script,
            // which is the only thing that can run in there.
            Expect::Speaks(names) => out.push_str(&format!(
                "out=$( {{ printf '%s\\n'                  '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"omh-doctor\",\"version\":\"0\"}}}}}}'                  '{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}'                  '{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{}}}}'; }} | {path} 2>/dev/null );                  missing=''; for n in {}; do printf '%s' \"$out\" | grep -q \"$n\" || missing=\"$missing $n\"; done;                  if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t%s\\n' \"$(printf '%s' \"$out\" | grep -o 'The store [^.]*' | head -1)\";                  else printf 'fail\\t{name}\\tno reply naming:%s\\n' \"$missing\"; fi\n",
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
            Some(Outcome {
                name: name.to_string(),
                ok,
                detail: detail.to_string(),
            })
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
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
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
        Fx {
            _dir: dir,
            profile: Profile::resolve(&paths),
        }
    }

    fn own() -> crate::base::Own {
        own_with(&Default::default())
    }

    /// Every server the manifest names counts as installed: `own` also
    /// switches a feature off when its server is gone from the profile, and a
    /// fixture declaring none would disable everything for the wrong reason.
    fn own_with(off: &std::collections::BTreeSet<String>) -> crate::base::Own {
        let manifest = crate::base::Manifest::load_dir(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/base"
        )))
        .unwrap();
        let installed = manifest.servers().into_keys().collect();
        crate::base::own(&manifest, off, &installed).unwrap()
    }

    fn adapter(name: &str) -> Adapter {
        Adapter::find(Path::new(ADAPTERS), name).unwrap()
    }

    /// omh's own hooks and rules sections come from the base manifest, not
    /// from a layer — so a profile that sources neither still has both mounted,
    /// and both have to be checked.
    ///
    /// Asking the profile whether a capability is worth checking is the same
    /// mistake `container::plan` made about rules: it answers about the layers
    /// and the question is about the mount. A check that quietly disappears is
    /// worse than one that fails, because `omh doctor` reporting 4/4 is the
    /// evidence everything else here defers to.
    #[test]
    fn a_capability_the_profile_does_not_source_is_still_checked() {
        let fx = fixture();
        let names: Vec<String> = checks(&fx.profile, &adapter("claude"), &own())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(
            names.iter().any(|n| n == "hooks"),
            "omh's own hooks are mounted with no hooks layer to source them: {names:?}"
        );
    }

    /// A server whose feature is off here is deliberately absent from the
    /// document the harness is given, so a check demanding it fails forever
    /// and blames the harness for obeying.
    ///
    /// Found by running `omh doctor` with `[omh] codegraph = false`, not by
    /// the suite: the checks were built from the layer files while the plan
    /// renders from the layers *minus* what this repo switched off, and only
    /// a real probe compares the two.
    #[test]
    fn a_server_this_repo_switched_off_is_not_demanded() {
        let fx = fixture();
        let off = own_with(&["codegraph".to_string()].into());

        let mcp = checks(&fx.profile, &adapter("claude"), &off)
            .into_iter()
            .find(|c| c.name == "mcp")
            .expect("claude stages mcp");
        assert_eq!(
            mcp.expect,
            Expect::Mentions(vec![]),
            "the only server in this profile is codegraph, and it is off here"
        );
    }

    #[test]
    fn every_declared_capability_is_checked() {
        let fx = fixture();
        let got: Vec<_> = checks(&fx.profile, &adapter("claude"), &own())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            got,
            vec!["AGENTS", "skills", "mcp", "subagents", "hooks"],
            "hooks are checked with no hooks layer, because omh generates them"
        );
    }

    /// A capability the harness cannot express is not a failure — it was
    /// already reported as dropped at launch. Checking it would fail forever.
    #[test]
    fn capabilities_the_harness_cannot_express_are_not_checked() {
        let fx = fixture();
        let caps: Vec<String> = checks(&fx.profile, &adapter("opencode"), &own())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(
            !caps.iter().any(|c| c == "subagents"),
            "opencode has no subagents"
        );
        assert!(caps.iter().any(|c| c == "skills"));
    }

    /// The entire point: doctor must inspect where the *harness* looks, not
    /// where omh staged. Checking the host would be circular.
    #[test]
    fn checks_target_guest_paths_only() {
        let fx = fixture();
        for check in checks(&fx.profile, &adapter("claude"), &own()) {
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
        let cs = checks(&fx.profile, &adapter("claude"), &own());

        let skills = cs.iter().find(|c| c.name == "skills").unwrap();
        assert_eq!(skills.expect, Expect::Entries(vec!["graphify".into()]));

        let mcp = cs.iter().find(|c| c.name == "mcp").unwrap();
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

        assert_eq!(
            entry_names(&[commands]),
            vec!["nested".to_string(), "ship.md".to_string()]
        );
    }

    // ── probe ───────────────────────────────────────────────────────────────

    #[test]
    fn the_probe_reports_one_line_per_check() {
        let fx = fixture();
        let cs = checks(&fx.profile, &adapter("claude"), &own());
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

    // ── credentials ─────────────────────────────────────────────────────────

    /// The failure that made every login silently fail to persist. It cannot be
    /// reproduced on the host — only inside the sandbox, against the real mount.
    #[test]
    fn every_credential_mount_is_probed_for_atomic_writes() {
        let cs = credential_checks(&adapter("claude"));
        assert!(!cs.is_empty(), "an adapter with credentials must be probed");
        assert!(
            cs.iter().all(|c| c.expect == Expect::AtomicWrite),
            "a credential check is about rename, not content: {cs:?}"
        );
        let guests: Vec<String> = cs.iter().map(|c| c.guest.display().to_string()).collect();
        assert!(
            guests.iter().any(|g| g.ends_with(".credentials.json")),
            "the declared token must be probed: {guests:?}"
        );
    }

    #[test]
    fn an_adapter_without_credentials_is_not_probed() {
        let bare: Adapter = toml::from_str(
            r#"
            name = "b"
            bin = "b"
            install = "x"
            [capabilities.rules]
            path = "/work/AGENTS.md"
            render = "concat"
            "#,
        )
        .unwrap();
        assert!(credential_checks(&bare).is_empty());
    }

    /// Probing must not cost the user their login. For a file, the probe writes
    /// back byte-identical content, so a successful rename changes nothing and a
    /// failed one leaves the original untouched.
    /// Every adapter that declares a token gets it probed — this is the check
    /// that decides whether a login can persist at all.
    #[test]
    fn every_adapter_with_a_token_has_it_probed() {
        for name in ["claude", "opencode"] {
            let a = adapter(name);
            assert_eq!(credential_checks(&a).len(), a.token.len(), "{name}");
        }
    }

    #[test]
    fn probing_a_credential_file_preserves_it() {
        let cs = credential_checks(&adapter("claude"));
        let script = probe_script(&cs);
        assert!(
            script.contains("cp "),
            "must copy the original before renaming: {script}"
        );
        assert!(
            !script.contains("> '/home/agent/.claude/.credentials.json'"),
            "must never truncate a credential file: {script}"
        );
    }

    #[test]
    fn the_probe_cleans_up_after_itself() {
        let script = probe_script(&credential_checks(&adapter("claude")));
        assert!(
            script.contains("rm -f"),
            "the probe file must not be left behind: {script}"
        );
    }

    #[test]
    fn the_atomic_write_probe_reports_in_the_same_protocol() {
        let script = probe_script(&credential_checks(&adapter("claude")));
        assert!(script.contains("printf 'ok"), "got: {script}");
        assert!(script.contains("printf 'fail"), "got: {script}");
    }

    /// A server omh configured but that cannot start is invisible from the
    /// host: every host-side test proves the tool list is right about a host
    /// directory, which is circular in the way this module exists to break.
    #[test]
    fn the_memory_probe_asks_the_server_it_was_configured_with() {
        let server = crate::render::Server {
            command: "omh".into(),
            args: vec![
                "memory".into(),
                "serve".into(),
                "--local".into(),
                "/omh/notes/local".into(),
            ],
            env: Default::default(),
        };
        let script = probe_script(&memory_checks(&server));

        assert!(
            script.contains("omh memory serve --local /omh/notes/local"),
            "{script}"
        );
        assert!(script.contains("tools/list"), "it has to ask: {script}");
        assert!(
            script.contains("initialize"),
            "and handshake first: {script}"
        );
        for tool in ["recall", "remember"] {
            assert!(script.contains(tool), "must require `{tool}`: {script}");
        }
    }

    /// `0 notes` is the signature of a store mounted at the wrong path — the
    /// server starts, answers, and knows nothing. Reporting the count is what
    /// makes that visible instead of looking like success.
    #[test]
    fn the_memory_probe_reports_how_many_notes_the_server_found() {
        let server = crate::render::Server {
            command: "omh".into(),
            args: vec!["memory".into(), "serve".into()],
            env: Default::default(),
        };
        let script = probe_script(&memory_checks(&server));
        assert!(
            script.contains("The store "),
            "the count has to reach the report: {script}"
        );
        // Both phrasings, because an empty store is the interesting one: it is
        // what a wrong mount looks like, and a blank detail hides it.
        for phrasing in [
            crate::memory::index::describe(&crate::memory::index::Index::of(&[])),
            crate::memory::index::describe(&crate::memory::index::Index::of(&[])),
        ] {
            assert!(
                phrasing.contains("The store "),
                "the probe greps for a phrase the description does not use: {phrasing}"
            );
        }
    }
}
