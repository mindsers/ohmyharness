//! `omh doctor` — the only thing that can validate an adapter.
//!
//! Adapters assert facts about *external software*: that Claude Code reads
//! `/work/.mcp.json`, that opencode reads `~/.config/opencode/command`. A green
//! unit suite proves omh mounts a path faithfully; it proves nothing about
//! whether anything reads it. Until this command runs, every adapter path is an
//! unverified claim and the most likely place for omh to be confidently wrong.
//!
//! That is not hypothetical. This module's own doc claimed Claude Code reads
//! `~/.mcp.json`; it does not, and never did. The binding said so, the renderer
//! produced a valid document, the launcher mounted it at exactly the declared
//! path, `Expect::Mentions` confirmed the document, `Expect::Speaks` confirmed
//! the server behind it — and no session ever loaded a single MCP server.
//! `Expect::Loaded` is the check that was missing, and the one that would have
//! caught it on day one.
//!
//! So doctor launches the real image with the real mounts and inspects the
//! **guest** paths the adapter declares. Checking anything host-side would test
//! the staging directory omh just wrote, which is circular.

use crate::adapter::{expand, Adapter, Capability, Render};
use crate::profile::Profile;
use anyhow::Result;
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
    /// `guest` is a JavaScript module that parses, and names each of these.
    ///
    /// The one render that emits a **program** rather than a configuration
    /// file, so it is the one that can be well-formed bytes and still be
    /// nonsense. `NonEmptyFile` passed for a module with a syntax error, for
    /// one where every hook had been dropped, and for one that threw on every
    /// event — while CONTRIBUTING puts this command above the suite as the only
    /// thing that verifies an adapter.
    Parses(Vec<String>),
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
    /// The **harness's own** listing names each of these, on a line that also
    /// says it is running.
    ///
    /// `Speaks` asks omh's server whether it works; `Mentions` asks whether the
    /// document says what it should. Both passed for a year against a binding
    /// that pointed at a path Claude Code does not read, because neither one
    /// asks the only question that matters: did the harness load it. This is
    /// the check that can answer, and the only one that goes red when a harness
    /// changes where it looks.
    ///
    /// `ready` is matched on the same line as the name rather than anywhere in
    /// the output, because every other line of a listing is another server that
    /// may well be fine.
    ///
    /// `guest` is the **directory** the document lives in, and the probe runs
    /// `command` from there. A harness that finds its config by project root
    /// answers about whatever project it was asked from, so a probe run in the
    /// wrong directory is a confident answer to a question nobody asked.
    Loaded {
        command: String,
        names: Vec<String>,
        ready: String,
    },
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
pub fn checks(
    profile: &Profile,
    adapter: &Adapter,
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
    resolves: &std::collections::BTreeMap<String, bool>,
) -> Result<Vec<Check>> {
    let mut out = Vec::new();
    for capability in Capability::ALL {
        let sources = profile.sources(capability)?;
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
            Render::Dir => Expect::Entries(entry_names(&sources, capability, repo)),
            Render::McpJson | Render::CodexToml | Render::OpencodeJson => {
                Expect::Mentions(server_names(&sources, repo))
            }
            Render::ClaudeSettings => Expect::NonEmptyFile,
            // A program gets a stronger check than a config file, not a weaker
            // one: that it parses, and that the hooks omh did not drop are in it.
            Render::OpencodePlugin => Expect::Parses(
                hook_names(&sources, own, repo, binding, &adapter.tools, resolves)
                    .unwrap_or_default(),
            ),
        };

        out.push(Check {
            name: capability.to_string(),
            guest,
            expect,
            dir: binding.render == Render::Dir,
        });

        // Asking the harness itself, where one says how. Additive rather than a
        // replacement: `Mentions` still answers *is the document what omh
        // meant*, and telling those two apart is what makes a failure
        // actionable — the document being wrong and the harness never reading
        // it look identical from any single check.
        //
        // Skipped where an adapter declares no `verify`, which is the same
        // trade the rest of this function makes: no check beats one that fails
        // forever and blames the harness for a question omh never asked.
        if let (Some(verify), Some(ready)) = (&binding.verify, &binding.ready) {
            let names = server_names(&sources, repo);
            let ask_from = expand(&binding.path, GUEST_HOME)
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            if !names.is_empty() {
                out.push(Check {
                    name: format!("{capability}-loaded"),
                    guest: ask_from,
                    expect: Expect::Loaded {
                        command: verify.clone(),
                        names,
                        ready: ready.clone(),
                    },
                    dir: true,
                });
            }
        }
    }
    Ok(out)
}

/// Entry names the harness should be able to see — which is what the launcher
/// *stages*, not what the catalogue declares.
///
/// An entry this repo did not select is deliberately absent, for the reason
/// `server_names` gives one capability over: demanding it makes `omh doctor`
/// fail forever and blame the harness for obeying. That argument was applied to
/// `disabled_servers` and not carried across when `[use]` landed, so a doctor
/// run in any curated repo reported `missing: <name>` — a false alarm in the
/// one command CONTRIBUTING puts above the test suite.
///
/// The **literal** filename is what gets asserted, because that is what omh
/// symlinks; the selection is matched on `entry_name`, which is the name a
/// `[use]` list holds. Comparing the same string on both sides would be wrong
/// in one direction or the other for every capability whose entries are files.
fn entry_names(
    sources: &[PathBuf],
    cap: Capability,
    repo: &crate::settings::RepoPolicy,
) -> Vec<String> {
    let mut names: Vec<String> = sources
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| {
            entries
                .flatten()
                // The literal staged name. Stripping extensions would assert a
                // guess about how the harness names things instead of asserting
                // what omh actually mounted.
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        })
        .filter(|name| {
            repo.selection
                .allows(cap, &crate::profile::entry_name(name))
        })
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The hooks that actually reached the generated module.
///
/// Rendered rather than listed, so a hook omh dropped is not demanded — the
/// reason `server_names` gives one capability over: demanding what omh
/// deliberately left out makes doctor fail forever and blame the harness.
fn hook_names(
    sources: &[PathBuf],
    own: &crate::base::Own,
    repo: &crate::settings::RepoPolicy,
    binding: &crate::adapter::Binding,
    tools: &std::collections::BTreeMap<crate::hook::Tool, String>,
    resolves: &std::collections::BTreeMap<String, bool>,
) -> Result<Vec<String>> {
    let doc = crate::render::document(
        Capability::Hooks,
        binding,
        sources,
        own,
        repo,
        tools,
        resolves,
    )?;
    let dropped: Vec<&str> = doc.dropped.iter().map(|d| d.name.as_str()).collect();
    let mut names: Vec<String> = own
        .hooks
        .iter()
        .map(|h| h.name.to_string())
        .filter(|n| !dropped.contains(&n.as_str()))
        .collect();
    names.sort();
    Ok(names)
}

/// What the document is expected to mention — which is what the launcher
/// renders, not what the layers declare.
///
/// A server whose feature is off here is deliberately left out of that
/// document. Demanding it makes `omh doctor` fail forever and blame the
/// harness for obeying, which is the opposite of what this command is for.
fn server_names(sources: &[PathBuf], repo: &crate::settings::RepoPolicy) -> Vec<String> {
    crate::render::parse_layers(sources)
        .map(|servers| {
            servers
                .into_keys()
                .filter(|name| !repo.disabled_servers.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Shell run inside the sandbox. Emits one `ok|fail<TAB>name<TAB>detail` line
/// per check.
/// A probe that reports, for each program, whether it resolves where the script
/// runs.
///
/// A second builder rather than a fifth `Expect`: a `Check` is path-shaped —
/// `guest` is documented as a path inside the sandbox — and a toolchain has no
/// path, only a name. Widening `Check` to carry either would touch every check
/// that already works, to express a subject the existing ones never have.
///
/// What is shared is the thing that matters: the wire protocol. These lines go
/// through the same [`parse`] as every other probe, so there is one format and
/// one reader, and `doctor` can concatenate this script with its own.
///
/// `command -v` rather than `which`: it is POSIX, it is a shell builtin so it
/// needs nothing installed to answer, and it resolves builtins and functions
/// as well as files on PATH. `which` is not in POSIX and is absent from some
/// minimal images — a probe that needs a package installed to report a missing
/// package is a probe that reports on itself.
///
/// **This must run where the hook will run.** Whether `cargo` resolves is a
/// fact about one machine, and the machine that matters is the sandbox — not
/// the host, and not a login shell whose profile has added to PATH.
pub fn probe_programs(programs: &[&str]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for p in programs {
        let q = single_quote(p);
        out.push_str(&format!(
            "if command -v {q} >/dev/null 2>&1; then printf 'ok\\t%s\\tresolves\\n' {q}; \
             else printf 'fail\\t%s\\tnot installed in the sandbox\\n' {q}; fi\n"
        ));
    }
    out
}

/// Wrap a word so the shell reads it as one literal, whatever is in it.
///
/// Program names reach here from commands a person wrote, so they are not
/// omh's to trust: a stray quote would otherwise end the literal early and the
/// rest of the name would be read as shell. Single quotes suspend every
/// expansion, and the one character they cannot contain is closed, escaped and
/// reopened — the standard `'\''` idiom.
pub(crate) fn single_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Run a generated probe through a real `/bin/sh`, in `cwd`, and hand back its
/// stdout.
///
/// The older probes are asserted by searching their source text for a
/// substring, which cannot distinguish a script that works from one that merely
/// mentions the right word. These are POSIX `sh` and need no container, so they
/// can be *run* — and a probe is a program, so running it is the only assertion
/// that means anything.
///
/// Shared with `stack`'s predicate tests rather than copied, so both halves of
/// the wire format are exercised by the same runner.
#[cfg(test)]
pub(crate) fn run_probe_in(script: &str, cwd: &std::path::Path) -> String {
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .expect("a probe must be a script /bin/sh can run");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

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
            // `node --check` inside the sandbox, for the same reason
            // `Expect::Speaks` runs a handshake there: the question is whether
            // the thing omh generated will load where it has to load.
            //
            // Copied to `.mjs` first, and that is load-bearing rather than
            // tidy: `node --check` on a `.ts` path **accepts anything** — it
            // took `export default (async () => ({ oops` without complaint —
            // so the obvious spelling of this probe would have been one more
            // check that cannot fail. The staged file keeps its `.ts` name
            // because that is what opencode loads.
            Expect::Parses(names) => out.push_str(&format!(
                "cp '{path}' /tmp/omh-probe.mjs 2>/dev/null; \
                 if ! err=$(node --check /tmp/omh-probe.mjs 2>&1); then \
                   printf 'fail\\t{name}\\tdoes not parse: %s\\n' \"$err\"; \
                 else missing=''; for n in {}; do grep -q -- \"$n\" '{path}' || missing=\"$missing $n\"; done; \
                   if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t{path}\\n'; \
                   else printf 'fail\\t{name}\\tmissing:%s\\n' \"$missing\"; fi; fi\n",
                shell_list(names)
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
            // Run from the document's own directory, which is the check rather
            // than incidental: a harness that finds its config by project root
            // answers about whatever root it was asked from.
            //
            // `grep` twice down a pipe rather than one pattern, because the
            // name and the ready word share a line in an order omh does not get
            // to decide. Line-wise rather than over the whole output for the
            // reason a note in this repo already records: a listing that names
            // every server means `contains` cannot tell *this* server is
            // running from *another* one being fine.
            Expect::Loaded {
                command,
                names,
                ready,
            } => out.push_str(&format!(
                "out=$( cd '{path}' 2>/dev/null && {command} 2>&1 ); missing=''; \
                 for n in {}; do printf '%s\\n' \"$out\" | grep -- \"$n\" | grep -q -- '{ready}' || missing=\"$missing $n\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t{name}\\t{path} ({command})\\n'; \
                 else printf 'fail\\t{name}\\t{command} in {path} does not report as {ready}:%s\\n' \"$missing\"; fi\n",
                shell_list(names),
                ready = ready.replace('\'', ""),
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
    use std::collections::BTreeMap;
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
        let catalogue = &paths.root;
        write(catalogue.join("rules/tdd.md"), "rules");
        write(catalogue.join("skills/graphify/SKILL.md"), "s");
        write(catalogue.join("subagents/explorer.md"), "a");
        write(
            catalogue.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"}}}"#,
        );
        Fx {
            _dir: dir,
            profile: Profile::resolve(&paths),
        }
    }

    /// Called twice per `checks` call, once for each half, which is why the
    /// manifest behind it is read once and leaked.
    fn decided() -> (crate::base::Own, crate::settings::RepoPolicy) {
        decided_with(Default::default())
    }

    fn base_manifest() -> &'static crate::base::Manifest {
        static CELL: std::sync::OnceLock<crate::base::Manifest> = std::sync::OnceLock::new();
        CELL.get_or_init(|| {
            crate::base::Manifest::load_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/base")))
                .unwrap()
        })
    }

    /// Every server the manifest names counts as installed: `own` also
    /// switches a feature off when its server is gone from the profile, and a
    /// fixture declaring none would disable everything for the wrong reason.
    ///
    /// The pair together, because a fixture that named a feature off without
    /// the servers it owns would let a check pass on a plan omh cannot build.
    fn decided_with(
        off: std::collections::BTreeSet<String>,
    ) -> (crate::base::Own, crate::settings::RepoPolicy) {
        let manifest = base_manifest();
        let installed = manifest.servers().into_keys().collect();
        let own = crate::base::own(manifest, &off, &installed).unwrap();
        (
            own,
            crate::settings::RepoPolicy::switching_off(manifest, off),
        )
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
        let names: Vec<String> = checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
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
        let (own, off) = decided_with(["codegraph".to_string()].into());

        let mcp = checks(
            &fx.profile,
            &adapter("claude"),
            &own,
            &off,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .find(|c| c.name == "mcp")
        .expect("claude stages mcp");
        assert_eq!(
            mcp.expect,
            Expect::Mentions(vec![]),
            "the only server in this profile is codegraph, and it is off here"
        );
    }

    /// An entry this repo did not select is deliberately absent from the
    /// directory the harness is given, so a check demanding it fails forever
    /// and blames the harness for obeying — the same argument
    /// `a_server_this_repo_switched_off_is_not_demanded` makes one capability
    /// over, and the one this PR forgot to carry across.
    ///
    /// It matters more than the server case, because `omh init` now writes a
    /// `[use]` list into every repo: any entry added to the catalogue
    /// afterwards is unselected, so `omh doctor` would fail on an ordinary,
    /// correctly configured checkout. A doctor that cries wolf on a normal
    /// configuration is a doctor nobody reads when an adapter path really
    /// breaks — and CONTRIBUTING puts this command above the test suite
    /// precisely because nothing else can catch that class of bug.
    #[test]
    fn an_entry_this_repo_did_not_select_is_not_demanded() {
        let fx = fixture();
        let (own, mut repo) = decided();
        repo.selection
            .apply(
                &BTreeMap::from([("skills".to_string(), Vec::new())]),
                Path::new("settings.toml"),
            )
            .unwrap();

        let skills = checks(
            &fx.profile,
            &adapter("claude"),
            &own,
            &repo,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .find(|c| c.name == "skills")
        .expect("claude stages skills");
        assert_eq!(
            skills.expect,
            Expect::Entries(vec![]),
            "the only skill in this profile is graphify, and this repo did not name it"
        );
    }

    #[test]
    fn every_declared_capability_is_checked() {
        let fx = fixture();
        let got: Vec<_> = checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
        assert_eq!(
            got,
            vec!["rules", "skills", "mcp", "mcp-loaded", "subagents", "hooks"],
            "hooks are checked with no hooks layer, because omh generates them; \
             and `mcp` is checked twice — the document, then the harness that \
             had to read it, which is the pair no single check can tell apart"
        );
    }

    /// A capability the harness cannot express is skipped rather than failed —
    /// it was already reported as dropped at launch, and checking it would fail
    /// forever and blame the harness for obeying.
    ///
    /// opencode used to be the subject: it declared no hooks. It declares them
    /// now — as a plugin — so the case needs a harness that genuinely lacks one,
    /// and `rules` on an adapter that omits it is the smallest honest example.
    #[test]
    fn capabilities_the_harness_cannot_express_are_not_checked() {
        let fx = fixture();
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::read_to_string(Path::new(ADAPTERS).join("opencode.toml")).unwrap();
        let at = real
            .find("[capabilities.rules]")
            .expect("opencode has rules");
        let next = real[at + 1..]
            .find("[capabilities.")
            .expect("a capability follows")
            + at
            + 1;
        std::fs::write(
            dir.path().join("terse.toml"),
            format!("{}{}", &real[..at], &real[next..])
                .replace("name    = \"opencode\"", "name    = \"terse\""),
        )
        .unwrap();
        let terse = Adapter::find(dir.path(), "terse").unwrap();

        let caps: Vec<String> = checks(
            &fx.profile,
            &terse,
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
        assert!(
            !caps.iter().any(|c| c == "rules"),
            "a capability this harness cannot express is not checked: {caps:?}"
        );
        assert!(caps.iter().any(|c| c == "skills"));
        // And one it *does* have is checked, which is the other half: a
        // capability omh silently declines to check is a capability nobody
        // ever finds out is broken.
        assert!(caps.iter().any(|c| c == "subagents"));

        // opencode itself now has every capability checked, hooks included.
        let all: Vec<String> = checks(
            &fx.profile,
            &adapter("opencode"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
        assert!(all.iter().any(|c| c == "hooks"), "got: {all:?}");
    }

    /// The entire point: doctor must inspect where the *harness* looks, not
    /// where omh staged. Checking the host would be circular.
    #[test]
    fn checks_target_guest_paths_only() {
        let fx = fixture();
        for check in checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
        {
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
        let cs = checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap();

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
            entry_names(&[commands], Capability::Commands, &decided().1),
            vec!["nested".to_string(), "ship.md".to_string()]
        );
    }

    // ── probe ───────────────────────────────────────────────────────────────

    #[test]
    fn the_probe_reports_one_line_per_check() {
        let fx = fixture();
        let cs = checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap();
        let script = probe_script(&cs);
        for c in &cs {
            assert!(
                script.contains(&c.guest.to_string_lossy().to_string()),
                "probe never looks at {:?}",
                c.guest
            );
        }
    }

    // ── the toolchain probe ─────────────────────────────────────────────────

    /// Run a generated probe through a real `/bin/sh` and hand back its stdout.
    ///
    /// The existing probes are asserted by searching their source text for a
    /// substring, which cannot distinguish a script that works from one that
    /// merely mentions the right word. This one is POSIX `sh` and needs no
    /// container, so it can be *run* — and a probe is a program, so running it
    /// is the only assertion that means anything.
    fn run_probe(script: &str) -> String {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .output()
            .expect("a probe must be a script /bin/sh can run");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// End to end: build the probe, run it, parse it back through the shared
    /// protocol. `sh` is present in every environment omh could possibly run
    /// in, and a name of that shape is present in none — so the two directions
    /// are both asserted without depending on what this machine happens to have
    /// installed.
    #[test]
    fn the_probe_answers_for_every_program_it_was_given() {
        let outcomes = parse(&run_probe(&probe_programs(&[
            "sh",
            "omh-no-such-program-b7f3",
        ])));

        let by = |name: &str| {
            outcomes
                .iter()
                .find(|o| o.name == name)
                .unwrap_or_else(|| panic!("the probe said nothing about {name}: {outcomes:?}"))
        };
        assert!(by("sh").ok, "sh resolves everywhere: {outcomes:?}");
        assert!(
            !by("omh-no-such-program-b7f3").ok,
            "and this resolves nowhere: {outcomes:?}"
        );
    }

    /// Program names are read out of commands a person wrote, so they are not
    /// omh's to trust. Interpolated bare, a name carrying a quote ends the
    /// shell literal early and everything after it is read as shell — which
    /// would both run it and destroy the probe's answers for every *other*
    /// program in the same script.
    #[test]
    fn a_program_name_with_a_quote_cannot_corrupt_the_probe() {
        // Every shape of shell expansion, not one. Quote-breaking is the
        // obvious payload and the least dangerous, because it is the only one
        // double quotes happen to stop — an escaping that neutralises it while
        // leaving `$(…)` live would have passed the version of this test that
        // checked a single payload.
        let hostile = [
            "x'; echo pwned; :'",
            "x$(echo pwned)",
            "x`echo pwned`",
            "x${IFS}pwned",
            "x\"; echo pwned; \"",
        ];
        let mut asked: Vec<&str> = hostile.to_vec();
        asked.push("sh");
        let out = run_probe(&probe_programs(&asked));
        let outcomes = parse(&out);

        // Line-wise, not `contains`: the marker is *inside the name*, so the
        // report echoes it back as data on every run. Only execution can put it
        // on a line of its own, and an assertion that cannot tell those apart
        // fails against correct code — as this one first did.
        assert!(
            !out.lines().any(|l| l.trim() == "pwned"),
            "the probe ran shell out of a program name: {out}"
        );

        // The real invariant, and the one that covers every expansion at once:
        // a name comes back **exactly** as it went in. Command substitution,
        // backticks and parameter expansion all change it, so this catches
        // them without needing to know which of them a broken quoting allows.
        for name in hostile {
            assert!(
                outcomes.iter().any(|o| o.name == name && !o.ok),
                "{name:?} came back changed, or not at all — something expanded \
                 it: {outcomes:?}"
            );
        }
        assert!(
            outcomes.iter().any(|o| o.name == "sh" && o.ok),
            "and one hostile name must not cost the answers for the rest: {outcomes:?}"
        );
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

    // ── loaded ──────────────────────────────────────────────────────────────

    /// Run the generated probe against a stubbed harness. Returns the outcome
    /// of the single check, so a test can say what the harness said and let the
    /// script decide — asserting on the script's *text* is how a probe that
    /// cannot fail gets written.
    fn probe_against(listing: &str, names: &[&str]) -> Outcome {
        let stub = tempfile::tempdir().unwrap();
        let at = stub.path().join("harness");
        std::fs::write(&at, format!("#!/bin/sh\ncat <<'EOF'\n{listing}\nEOF\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = probe_script(&[Check {
            name: "mcp-loaded".into(),
            guest: stub.path().to_path_buf(),
            expect: Expect::Loaded {
                command: "harness list".into(),
                names: names.iter().map(|n| n.to_string()).collect(),
                ready: "Connected".into(),
            },
            dir: true,
        }]);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    stub.path().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .stdin(std::process::Stdio::null())
            .output()
            .expect("sh must run");
        let outcomes = parse(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(outcomes.len(), 1, "one check, one line: {script}");
        outcomes.into_iter().next().unwrap()
    }

    /// The bug this check exists for: omh's document was valid, mounted, and at
    /// the path the adapter declared, and the harness read none of it. Nothing
    /// host-side can see that, so the only honest question is the one the
    /// harness answers itself.
    #[test]
    fn a_server_the_harness_never_loaded_fails_the_check() {
        let out = probe_against("some-other-server: x - Connected", &["memory"]);
        assert!(!out.ok, "a listing that never names it must fail: {out:?}");
    }

    /// The half that a name match alone would wave through, and the state a
    /// project-scoped document actually lands in when nothing has approved it:
    /// listed in full, loaded not at all. `Mentions` was already green here.
    #[test]
    fn a_server_listed_but_not_running_fails_the_check() {
        let out = probe_against("memory: omh memory serve - Pending approval", &["memory"]);
        assert!(
            !out.ok,
            "listed is not loaded — this is the state the fix had to clear: {out:?}"
        );
    }

    #[test]
    fn a_server_the_harness_reports_running_passes() {
        let out = probe_against("memory: omh memory serve - Connected", &["memory"]);
        assert!(out.ok, "{out:?}");
    }

    /// Line-wise, not over the whole output. Every listing names every server,
    /// so a check that greps the output as one blob passes whenever *any*
    /// server is healthy — which is the failure mode most likely to be hit,
    /// since the remote servers in a real listing are always connected.
    #[test]
    fn one_healthy_server_does_not_vouch_for_a_broken_one() {
        let out = probe_against(
            "codegraph: c - Connected\nmemory: omh memory serve - Pending approval",
            &["codegraph", "memory"],
        );
        assert!(
            !out.ok,
            "`memory` is not running and the check must say so: {out:?}"
        );
        assert!(
            out.detail.contains("memory") && !out.detail.contains("codegraph"),
            "and must name which one: {out:?}"
        );
    }

    /// The probe asks in the directory the document lives in, because a harness
    /// that finds config by project root answers about the root it was asked
    /// from. Running it anywhere else is a confident answer to another
    /// question.
    #[test]
    fn the_check_asks_where_the_document_is() {
        let fx = fixture();
        let cs = checks(
            &fx.profile,
            &adapter("claude"),
            &decided().0,
            &decided().1,
            &Default::default(),
        )
        .unwrap();
        let loaded = cs
            .iter()
            .find(|c| matches!(c.expect, Expect::Loaded { .. }))
            .expect("an adapter declaring `verify` must be asked");
        let mcp = cs
            .iter()
            .find(|c| c.name == "mcp")
            .expect("and the document itself is still checked");
        assert_eq!(
            Some(loaded.guest.as_path()),
            mcp.guest.parent(),
            "the probe must run where the document is"
        );
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
