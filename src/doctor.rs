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
    /// The harness's own answer to "are you logged in" says `ready`.
    ///
    /// `Loaded` without the names: there is nothing to match *per item*, so
    /// `ready` is looked for anywhere in the output rather than on a line with
    /// something else. The distinction `Loaded` draws — every other line is
    /// another server that may well be fine — has no analogue here, because a
    /// login is one fact.
    ///
    /// This exists because `AtomicWrite` cannot be asked of a harness that
    /// keeps no token file. omp keeps credentials in SQLite, and the database
    /// is created on first start by settings and telemetry, so the strongest
    /// host-side statement available is "a file exists that would exist
    /// anyway".
    Answers { command: String, ready: String },
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

/// What must be true of **the host's** git, which is where the harvest runs.
///
/// Every other check in this module runs inside the sandbox, because that is
/// where the answers omh cannot see from outside live. These are the opposite
/// case: `omh sNN commit --keep` fetches, replants and stamps on the host, as
/// the user, with the user's git — so a git that cannot do what omh asks is
/// invisible to a probe that runs in the container.
///
/// Capabilities rather than a version comparison. omh cannot check a version
/// it cannot name, and the release that introduced `cherry-pick --empty=` was
/// not verifiable when the dependency was added; asking the binary answers for
/// whatever git is actually installed and keeps answering as git grows. The
/// version is still reported, because it is the first thing anyone asks for in
/// a bug report.
///
/// Only what omh uses **today**. `merge-tree --write-tree` belongs here when
/// `sync` ships and not before: a doctor that fails over a capability nothing
/// calls is one people learn to ignore.
pub fn git_checks() -> Vec<Outcome> {
    match version_of(std::process::Command::new("git").arg("--version").output()) {
        Ok(version) => git_checks_from(
            version,
            crate::shadow::git_supports("cherry-pick", "--empty"),
            crate::shadow::git_supports("merge-tree", "--write-tree"),
        ),
        Err(why) => vec![Outcome {
            name: "git on the host".into(),
            ok: false,
            detail: format!("{why} — every way work leaves a session runs git here"),
        }],
    }
}

/// What `git --version` said, or why it does not count as an answer.
///
/// Over the process result rather than the process, so all four states are a
/// table. Three of them were unreachable by any test while this was inline,
/// and two mutations proved it: a `git` that exits 0 saying nothing rendered a
/// green tick with a blank cell, and ignoring a non-zero exit did the same.
fn version_of(asked: std::io::Result<std::process::Output>) -> Result<String, String> {
    let out = asked.map_err(|e| format!("omh could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git is on PATH and would not answer `--version`: {}",
            crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim())
        ));
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        // A `git` that exits 0 and says nothing is a wrapper, not git. The
        // same guard `user_pager` carries in `shadow.rs`, missing here until a
        // review found it.
        "" => Err("git answered `--version` with nothing at all".to_string()),
        said => Ok(said.to_string()),
    }
}

/// The report, given the answers — so each is assertable on a machine that can
/// only give one of them.
///
/// Injected for the reason `plan_delivery` gives: the part that can be wrong
/// silently is the decision, and the shelling-out part is the part that fails
/// loudly. The first version probed inline and its test compared the result
/// against the same call, which is a tautology — it passed against an
/// `ok: true` hardcoded in place of the probe.
///
/// **One outcome, and it fails only when git cannot be used at all.** The
/// capabilities ride in the detail rather than as red lines: a `doctor` that
/// goes red over something the user never calls is one they stop running. A
/// user who never names checkpoints and never syncs is not broken, and
/// `doctor`'s exit code is what `troubleshooting.md` tells them means *the
/// adapter is wrong*.
///
/// Two capabilities rather than one because they fail apart. `--keep 1,3` and
/// `sync` are different commands on different gits — `cherry-pick --empty`
/// arrived in 2.34 and `merge-tree --write-tree` in 2.38 — so a single line
/// saying *git is too old* would send a user with 2.35 looking for a problem
/// with a command that works.
fn git_checks_from(
    version: String,
    keeps_a_selection: Result<bool, anyhow::Error>,
    merges_on_the_host: Result<bool, anyhow::Error>,
) -> Vec<Outcome> {
    let selections = match keeps_a_selection {
        Ok(true) => "takes a `--keep` selection".to_string(),
        Ok(false) => "no `cherry-pick --empty`, so `--keep <selection>` cannot run here — \
             `--keep` on its own is unaffected"
            .to_string(),
        // Could not ask. Not the same as *cannot do it*, and the reason is
        // git's own — a shim, a bad config line, a version manager with no
        // version set.
        Err(e) => format!("omh could not tell whether `--keep <selection>` works here: {e}"),
    };
    // Named for what the user loses, not for the flag. `sync` merges on the
    // host precisely so no commit from the checkout enters the sandbox; there
    // is no fallback that keeps that property, so a git without it means the
    // command is unavailable rather than slower.
    let syncs = match merges_on_the_host {
        Ok(true) => "syncs".to_string(),
        Ok(false) => {
            "no `merge-tree --write-tree` (git 2.38), so `omh sNN sync` cannot run here".to_string()
        }
        Err(e) => format!("omh could not tell whether `sync` works here: {e}"),
    };
    vec![Outcome {
        name: "git on the host".into(),
        ok: true,
        detail: format!("{version} — {selections}; {syncs}"),
    }]
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
    let files = adapter.token.iter().map(|template| Check {
        name: "token".into(),
        guest: expand(template.trim_end_matches('/'), GUEST_HOME),
        expect: Expect::AtomicWrite,
        dir: template.ends_with('/'),
    });
    // No `token`-is-empty filter here. There was one, and it was a rule for
    // which of the two wins written in a single consumer and nowhere else —
    // `auth::decided_by_files` needed the same fact and could not see it. The
    // pair is refused by `Adapter::check_login` now, so an adapter declaring
    // both never reaches this function and a guard here would be dead.
    //
    // `guest` is the home directory rather than the worktree: unlike `Loaded`,
    // where a harness resolves its config from the project it was asked from, a
    // login is an account fact and the same from anywhere. Naming `/work` would
    // imply a project-scoped answer that is not on offer.
    let probe = adapter.token_probe.iter().map(|p| Check {
        name: "login".into(),
        guest: PathBuf::from(GUEST_HOME),
        expect: Expect::Answers {
            command: p.run.clone(),
            ready: p.ready.clone(),
        },
        dir: true,
    });
    files.chain(probe).collect()
}

/// What a TLS handshake said about who signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspection {
    /// omh could not tell, and says so. **Offline is this, never `Private`.**
    /// A laptop on a plane must not be told it is behind a corporate proxy.
    Unknown(String),
    /// The chain terminates in a root this platform ships, so a container —
    /// which trusts the same public set — will accept it too.
    Public,
    /// The chain terminates in a root this platform does *not* ship. The host
    /// trusts it because somebody installed it; a container will not.
    Private,
}

/// Read `openssl s_client -CAfile <the platform's public roots>`.
///
/// The question is not "is this certificate valid" — on the host it always is,
/// which is why nothing noticed. It is "would a container accept it", and the
/// way to ask that is to verify against the *public* root set alone, which is
/// what a Debian container's `ca-certificates` is.
pub fn inspection(output: Option<&str>) -> Inspection {
    let Some(out) = output else {
        return Inspection::Unknown("openssl did not run".into());
    };
    // `Verify return code:` is the only line that answers the question. Its
    // absence means the handshake never got far enough to have an opinion —
    // no network, a refused connection, a tool that is not there — and that is
    // an `Unknown`, not a verdict. A plane is not a proxy.
    let Some(code) = out
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("Verify return code:"))
    else {
        return Inspection::Unknown(
            "the handshake did not complete, so there is nothing to read".into(),
        );
    };
    let code = code.trim();
    if code.starts_with("0 ") || code == "0" {
        return Inspection::Public;
    }
    // The verify failures a re-signing proxy produces. Anything else — an
    // expired certificate, a hostname mismatch — is a real problem with that
    // host and is not what this check is about, so it stays `Unknown`.
    const RESIGNED: &[&str] = &[
        "unable to get local issuer certificate",
        "self signed certificate in certificate chain",
        "self-signed certificate in certificate chain",
        "unable to verify the first certificate",
    ];
    if RESIGNED.iter().any(|r| code.contains(r)) {
        Inspection::Private
    } else {
        Inspection::Unknown(format!(
            "openssl answered `{code}`, which is not a signing problem"
        ))
    }
}

/// The platform's **public** root set, as a file openssl can verify against.
///
/// macOS keeps Apple's shipped roots in their own keychain, separate from
/// anything an administrator installed later. That separation is the whole
/// test: a corporate root lands in the System or login keychain, never in
/// `SystemRootCertificates`, so verifying against this file alone asks the
/// same question a container asks.
///
/// `None` rather than a guess anywhere it cannot be answered — on Linux the
/// system store *is* the public set plus whatever was added, with no line
/// between them, so this check does not apply there and says so.
fn public_roots() -> Option<std::path::PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("security")
        .args([
            "find-certificate",
            "-a",
            "-p",
            "/System/Library/Keychains/SystemRootCertificates.keychain",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let pem = String::from_utf8(out.stdout).ok()?;
    if !pem.contains("BEGIN CERTIFICATE") {
        return None;
    }
    let at = std::env::temp_dir().join("omh-public-roots.pem");
    std::fs::write(&at, pem).ok()?;
    Some(at)
}

/// Ask whether a container would accept what this host is being served.
///
/// `host` is one omh actually fetches from during a build, so a proxy that
/// inspects selectively is asked about traffic omh depends on rather than
/// traffic in general.
pub fn inspection_of(host: &str) -> Inspection {
    inspection_at(&format!("{host}:443"), host)
}

/// `inspection_of` with the endpoint spelled out, so a test can point it at a
/// server it controls rather than at the internet.
pub fn inspection_at(endpoint: &str, servername: &str) -> Inspection {
    let Some(roots) = public_roots() else {
        return Inspection::Unknown(
            "omh can only tell a shipped root from an installed one on macOS".into(),
        );
    };
    let out = std::process::Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            endpoint,
            "-servername",
            servername,
            "-CAfile",
            &roots.display().to_string(),
        ])
        // EOF at once: `s_client` waits for something to send otherwise, and a
        // check that hangs is a check nobody runs twice.
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(out) => {
            let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
            said.push_str(&String::from_utf8_lossy(&out.stderr));
            inspection(Some(&said))
        }
        Err(e) => Inspection::Unknown(format!("openssl did not run: {e}")),
    }
}

/// Whether the corporate root actually reached the sandbox's trust store.
///
/// **Not whether the recipe says so.** `update-ca-certificates` prints
/// `WARNING: Skipping ...` and exits 0 for a certificate it cannot parse, so a
/// truncated or half-pasted PEM produces a green build and a sandbox that
/// verifies nothing — and the failure then surfaces in the next `pip install`,
/// a command away from the setting that caused it.
///
/// The check reads `/etc/ssl/certs/ca-certificates.crt`, which is what
/// `SSL_CERT_FILE`, `PIP_CERT`, `CARGO_HTTP_CAINFO` and `GIT_SSL_CAINFO` all
/// point at. Reading the file omh wrote instead would only confirm omh wrote
/// it, which the suite already does.
///
/// The needle is a body line rather than the marker: every certificate in the
/// store has a `BEGIN CERTIFICATE`, and a check that any of them is present
/// passes on an image that skipped this one.
pub fn ca_check(ca: Option<&str>) -> Option<Check> {
    let body = ca?
        .lines()
        .skip_while(|l| !l.contains("BEGIN CERTIFICATE"))
        .skip(1)
        .find(|l| l.len() > 16 && !l.contains("END CERTIFICATE"))?
        .to_string();
    Some(Check {
        name: "corporate root (ca_cert)".into(),
        guest: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        expect: Expect::Mentions(vec![body]),
        dir: false,
    })
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
        // Rules keys on what omh generates. Hooks does *not*, and the
        // difference is not a nicety: until 2026.08 `git-unavailable` was the
        // one shipped hook outside `codegraph`, so `own.hooks` was never empty
        // and this read as "always check hooks". Retiring it made every shipped
        // hook a `codegraph` one, and a repo with `codegraph = false` then had
        // its hooks check skipped in silence — including the repo's own hooks
        // layer, which is exactly the case `render::merge_hooks` reasons is
        // safe because "the document is never empty".
        //
        // So ask whether the *harness binds* the capability rather than whether
        // omh happens to contribute to it. A hooks module gets mounted either
        // way, and a mounted document nobody checks is what `doctor` exists to
        // stop.
        let generated = match capability {
            Capability::Rules => !own.sections.is_empty(),
            Capability::Hooks => true,
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
            //
            // Both plugin renders, because both emit plain JavaScript under a
            // `.ts` name — the extension is what each harness's loader expects,
            // not a claim that either module needs a TypeScript parser.
            Render::OpencodePlugin | Render::OmpPlugin => Expect::Parses(
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
            // One fact, so `ready` is looked for anywhere in stdout rather than
            // on a line with a name beside it.
            //
            // **A login is the exit status and the marker, never the marker
            // alone.** The first version asked only whether `ready` appeared
            // anywhere in the command's combined output, and a harness that
            // errored out was then judged by the words its error happened to
            // contain: `harness usage --json` exiting 1 with "run /login to
            // obtain an accountId" on stderr reported a successful login. That
            // is the exact false positive `token-probe` exists to remove,
            // arriving through the check meant to replace it.
            //
            // stdout and stderr are kept apart for the same reason. The marker
            // has to come from the answer, not from a warning printed beside
            // it; both are still quoted back on failure, because "no account"
            // and "no such subcommand" are indistinguishable from an exit code
            // and decide whether the user runs `omh auth` or fixes the adapter.
            //
            // `grep -F`: `ready` is a marker an adapter author wrote, not a
            // pattern. Read as a basic regex, `usage: omp [options]` matches
            // any line holding one of `o i t p n s`, which is a far looser
            // check than anything anyone typed.
            //
            // `command` reaches `printf` as an **argument**, never inside the
            // format string. Interpolated into the format, a `%` in a command
            // was read as a directive and a `'` closed the quote and broke the
            // whole concatenated probe — taking every other check with it.
            Expect::Answers { command, ready } => out.push_str(&format!(
                "e=$(mktemp 2>/dev/null || echo /tmp/omh-login.$$); \
                 out=$( cd '{path}' 2>/dev/null && {command} 2>\"$e\" ); code=$?; \
                 err=$(cat \"$e\" 2>/dev/null); rm -f \"$e\"; \
                 if [ \"$code\" -eq 0 ] && printf '%s' \"$out\" | grep -qF -- {ready}; \
                 then printf 'ok\\t{name}\\t%s reports %s\\n' {cmd} {ready}; \
                 else printf 'fail\\t{name}\\t%s exited %s without %s: %s\\n' \
                   {cmd} \"$code\" {ready} \
                   \"$(printf '%s %s' \"$out\" \"$err\" | head -c 200 | tr '\\n' ' ')\"; fi\n",
                cmd = single_quote(command),
                ready = single_quote(ready),
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

    /// **Both arms, against real TLS.** The unit test above fixes the reading;
    /// this one proves omh reads the right thing off a real handshake, which
    /// is the half that can be quietly wrong — a `-CAfile` that silently does
    /// not load leaves openssl verifying against nothing and answering `20`
    /// for the whole internet, which would report every user as proxied.
    ///
    /// The private arm runs `openssl s_server` with a root this test makes, so
    /// it is the same shape as a corporate proxy without needing one. The
    /// public arm needs the network and is the reason this is `#[ignore]`d
    /// along with the container tests; `./scripts/check.sh --all` runs it.
    #[test]
    #[ignore]
    fn a_private_root_reads_as_private_and_a_public_one_does_not() {
        let d = tempfile::tempdir().unwrap();
        let at = |n: &str| d.path().join(n).display().to_string();
        let sh = |c: &str| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(c)
                .output()
                .expect("shell")
        };

        // A root, and a leaf it signs for `testserver`.
        sh(&format!(
            "openssl req -x509 -newkey rsa:2048 -nodes -keyout {} -out {} -days 1 \
             -subj '/CN=omh Test Corp Root' 2>/dev/null",
            at("root.key"),
            at("root.pem")
        ));
        sh(&format!(
            "openssl req -newkey rsa:2048 -nodes -keyout {} -out {} \
             -subj '/CN=testserver' 2>/dev/null",
            at("leaf.key"),
            at("leaf.csr")
        ));
        std::fs::write(d.path().join("ext"), "subjectAltName=DNS:testserver\n").unwrap();
        sh(&format!(
            "openssl x509 -req -in {} -CA {} -CAkey {} -CAcreateserial -out {} \
             -days 1 -extfile {} 2>/dev/null",
            at("leaf.csr"),
            at("root.pem"),
            at("root.key"),
            at("leaf.pem"),
            at("ext")
        ));
        assert!(
            std::fs::read_to_string(d.path().join("leaf.pem"))
                .is_ok_and(|p| p.contains("BEGIN CERTIFICATE")),
            "the fixture must actually produce a leaf"
        );

        // Serve it, on a port nothing else is likely to hold.
        let mut server = std::process::Command::new("openssl")
            .args([
                "s_server",
                "-accept",
                "34443",
                "-cert",
                &at("leaf.pem"),
                "-key",
                &at("leaf.key"),
                "-www",
                "-quiet",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("openssl s_server");
        std::thread::sleep(std::time::Duration::from_millis(1500));

        let said = inspection_at("127.0.0.1:34443", "testserver");
        // Killed *and* reaped: a `kill` without a `wait` leaves a zombie for
        // as long as the test binary runs, and clippy is right to say so.
        let _ = server.kill();
        let _ = server.wait();
        assert_eq!(
            said,
            Inspection::Private,
            "a leaf signed by a root this platform does not ship is what a \
             TLS-inspecting proxy serves, and must read as Private"
        );

        // And the internet, which is not proxied on a developer's own machine
        // unless it is — in which case this test is telling the truth and the
        // person running it already knows.
        assert_eq!(
            inspection_of("registry.npmjs.org"),
            Inspection::Public,
            "npm's registry is signed by a public root; reading it as Private \
             means the `-CAfile` is not loading and every user would be told \
             they are behind a proxy"
        );
    }

    /// **Offline must never read as "you are behind a proxy".** That is the
    /// whole difficulty of asking this question before anything has failed: a
    /// check that cries wolf on a plane is worse than no check, because the
    /// remedy it names — installing a corporate root — is one a person can
    /// actually carry out and then be confused by forever.
    ///
    /// So the verdict is three-valued, and every way of not knowing collapses
    /// into `Unknown` with a reason rather than into either answer.
    ///
    /// The question asked is deliberately not "is this certificate valid".
    /// On the host it always is — the root is installed, which is exactly why
    /// nothing noticed. It is "would a *container* accept this", and that is
    /// asked by verifying against the platform's public root set alone, which
    /// is what a Debian container's `ca-certificates` package is.
    #[test]
    fn only_a_completed_handshake_can_say_a_root_is_private() {
        // Verified against the public set: a container would accept it.
        assert_eq!(
            inspection(Some(
                "depth=2 C = US, O = DigiCert Inc\nVerify return code: 0 (ok)\n"
            )),
            Inspection::Public
        );

        // The two shapes a re-signing proxy produces, measured in the control
        // arm behind `image::ca_layer`'s table.
        for said in [
            "Verify return code: 20 (unable to get local issuer certificate)",
            "Verify return code: 19 (self signed certificate in certificate chain)",
            "Verify return code: 21 (unable to verify the first certificate)",
        ] {
            assert_eq!(
                inspection(Some(said)),
                Inspection::Private,
                "this is what an inspecting proxy looks like: {said}"
            );
        }

        // **Every way of not knowing.** No connection at all, a tool that is
        // not there, and output that says nothing about verification.
        for nothing in [
            None,
            Some(""),
            Some("connect: Connection refused"),
            Some("s_client: unknown option"),
            Some("depth=0 CN = example.com\n"),
        ] {
            assert!(
                matches!(inspection(nothing), Inspection::Unknown(_)),
                "not knowing must be Unknown, never a verdict: {nothing:?}"
            );
        }

        // And the reason travels, because a check that says "unknown" without
        // saying why is a row somebody has to investigate by hand.
        let Inspection::Unknown(why) = inspection(None) else {
            panic!("None is unknown")
        };
        assert!(!why.is_empty(), "an unknown must carry its reason");
    }
    /// Four ways `git --version` can answer, and only one is a version.
    ///
    /// Every arm but the last was unreachable while this was inline, and two
    /// mutations proved it: a `git` exiting 0 with nothing to say rendered a
    /// green tick and a blank cell, counted as a pass.
    #[test]
    fn only_a_version_counts_as_a_version() {
        use std::os::unix::process::ExitStatusExt;
        let output = |code: i32, stdout: &str, stderr: &str| std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };

        assert_eq!(
            version_of(Ok(output(0, "git version 2.55.0\n", ""))).unwrap(),
            "git version 2.55.0",
            "trimmed, so the report stays a table"
        );

        // On PATH, and would not answer. macOS without the developer tools is
        // the everyday case, and it says so — omh must not overwrite that with
        // a guess about PATH.
        let err = version_of(Ok(output(1, "", "xcode-select: note: no developer tools")))
            .expect_err("a git that fails is not a git that answered");
        assert!(err.contains("developer tools"), "git's own words: {err}");

        let err = version_of(Ok(output(0, "  \n", "")))
            .expect_err("a wrapper that says nothing is not git");
        assert!(err.contains("nothing at all"), "{err}");

        let err = version_of(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file or directory",
        )))
        .expect_err("no git at all");
        assert!(err.contains("could not run git"), "{err}");
    }

    /// The host's git is reported where the harvest runs — and fails only
    /// when git cannot be used at all.
    ///
    /// Every other check in this module runs inside the sandbox; this one runs
    /// where `--keep` and `sync` do. The capabilities ride in the detail
    /// rather than as red lines: a `doctor` that goes red over something the
    /// user never calls is one they stop running.
    ///
    /// The two capabilities are asserted apart because they fail apart — the
    /// gits that can do one and not the other are three minor versions wide,
    /// and a report that folded them together would name the wrong command.
    #[test]
    fn the_hosts_git_is_reported_and_only_a_missing_git_fails() {
        let only = |checks: Vec<Outcome>| {
            assert_eq!(checks.len(), 1, "one row, not a cascade: {checks:?}");
            checks.into_iter().next().unwrap()
        };

        let able = only(git_checks_from(
            "git version 2.55.0".into(),
            Ok(true),
            Ok(true),
        ));
        assert!(able.ok);
        assert!(
            able.detail.starts_with("git version 2.55.0"),
            "the version verbatim — the first thing a bug report needs: {able:?}"
        );
        assert!(
            able.detail.contains("--keep"),
            "and what it means for the command: {able:?}"
        );
        assert!(
            able.detail.contains("syncs"),
            "and for the other command: {able:?}"
        );

        // A git too old for selections is still a working git. The user who
        // never names checkpoints must not be told their adapter is broken.
        let old = only(git_checks_from(
            "git version 2.30.0".into(),
            Ok(false),
            Ok(false),
        ));
        assert!(old.ok, "an old git is not a failed check: {old:?}");
        // A run of spaces means a line continuation left its indentation in
        // the string. `cargo fmt` joins those lines, so the padding ships —
        // caught here once already, in this very sentence.
        for row in [&able, &old] {
            assert!(
                !row.detail.contains("  "),
                "the detail carries a fold's indentation: {row:?}"
            );
        }
        assert!(old.detail.contains("--empty") && old.detail.contains("--keep"));

        // The middle ground, and the reason these are two answers: git 2.35
        // takes a `--keep` selection and cannot sync. One line for both would
        // send this user to read about the command that works.
        let between = only(git_checks_from(
            "git version 2.35.0".into(),
            Ok(true),
            Ok(false),
        ));
        assert!(
            between.detail.contains("takes a `--keep` selection")
                && between.detail.contains("sync` cannot run"),
            "each command gets its own verdict: {between:?}"
        );
        assert!(
            !between.detail.contains("  "),
            "the detail carries a fold's indentation: {between:?}"
        );

        // Could not ask is its own answer, and it carries git's reason.
        let unsure = only(git_checks_from(
            "git version 2.55.0".into(),
            Err(anyhow::anyhow!("bad config line 2 in .git/config")),
            Err(anyhow::anyhow!("bad config line 2 in .git/config")),
        ));
        assert!(
            unsure.detail.contains("bad config line 2"),
            "git's own words reach the user: {unsure:?}"
        );
        assert!(
            !unsure.detail.contains("no `cherry-pick --empty`")
                && !unsure.detail.contains("cannot run here"),
            "and omh does not turn *could not tell* into a verdict, for either \
             command: {unsure:?}"
        );
        assert_eq!(
            unsure.detail.matches("bad config line 2").count(),
            2,
            "both say why they could not tell, rather than one inheriting the \
             other's excuse: {unsure:?}"
        );

        // The real thing, against the real git: a version, read from stdout
        // and trimmed. What it says about selections is this machine's answer
        // and is asserted above for both.
        let here = only(git_checks());
        assert!(
            here.ok && here.detail.starts_with("git version"),
            "this machine has git: {here:?}"
        );
        assert!(
            !here.detail.contains('\n'),
            "trimmed, so the report stays a table: {here:?}"
        );
    }

    use super::*;
    use crate::profile::Paths;
    use std::collections::BTreeMap;
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    /// A harness whose credentials are not a file still gets its login checked.
    ///
    /// `credential_checks` reads `token`, and omp declares none — its
    /// credentials are rows in SQLite, and the database is created by boot
    /// noise. Left as-is, the harness with the *weakest* host-side evidence
    /// would be the one omh checked least, which is backwards: it is precisely
    /// the case where only the harness can answer.
    #[test]
    fn a_harness_that_keeps_no_token_file_is_asked_instead() {
        let omp = Adapter::find(Path::new(ADAPTERS), "omp").unwrap();
        assert!(
            omp.token.is_empty(),
            "this test is about the no-token case; omp grew a token file"
        );
        let checks = credential_checks(&omp);
        let login = checks
            .iter()
            .find(|c| c.name == "login")
            .unwrap_or_else(|| panic!("no login check: {checks:?}"));
        assert_eq!(
            login.expect,
            Expect::Answers {
                command: "omp usage --json".into(),
                ready: "accountId".into(),
            }
        );
    }

    /// The probe omh generates is a script `sh` will actually run.
    ///
    /// `probe_script` writes shell out of Rust format strings, and this arm
    /// nests a `$( … )` inside a quoted `printf` argument inside an `if`. A
    /// quoting mistake there is invisible in every assertion that greps the
    /// generated text for a substring — the script would be staged, run, and
    /// fail as though the *harness* were broken.
    #[test]
    fn the_login_probe_is_a_script_sh_can_parse() {
        let script = probe_script(&credential_checks(
            &Adapter::find(Path::new(ADAPTERS), "omp").unwrap(),
        ));
        assert!(script.contains("omp usage --json"), "{script}");
        let out = std::process::Command::new("/bin/sh")
            .arg("-n")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("sh is available");
        assert!(
            out.status.success(),
            "generated probe does not parse: {}\n--- script ---\n{script}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// And a harness that *does* keep one is not asked twice.
    ///
    /// The two are alternatives, not a belt and braces: a file check and a
    /// probe that disagree have no rule for which wins, and the adapter schema
    /// says so in `token_probe`'s own doc.
    #[test]
    fn a_harness_with_a_token_file_is_not_also_interrogated() {
        let claude = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let checks = credential_checks(&claude);
        assert!(
            checks.iter().all(|c| c.name != "login"),
            "claude has token files and should not be probed: {checks:?}"
        );
        assert!(
            checks.iter().any(|c| c.expect == Expect::AtomicWrite),
            "the token files are still checked: {checks:?}"
        );
    }

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

    /// **The one thing about `ca_cert` a green suite cannot say.**
    ///
    /// Every other guard for this feature asserts what is in the recipe. None
    /// of them can say the root reached the trust store, and the gap is not
    /// theoretical: `update-ca-certificates` prints a warning and **exits 0**
    /// when it skips a certificate it cannot parse, so a truncated or
    /// half-pasted PEM gives a green build and a sandbox that trusts nothing.
    /// AGENTS.md puts exactly this case on `doctor`.
    ///
    /// Asserting on the *bundle* rather than on the file omh wrote is what
    /// makes it a real check: the file omh wrote is omh's own output, and the
    /// bundle is what the toolchains read.
    #[test]
    fn a_corporate_root_is_checked_in_the_store_that_reads_it() {
        assert!(
            ca_check(None).is_none(),
            "nothing to check when nothing is set"
        );

        // A realistic body line: a PEM wraps base64 at 64 characters, and the
        // needle has to be long enough that finding it in a store of a hundred
        // certificates means something.
        let pem = "-----BEGIN CERTIFICATE-----\n\
                   DISTINCTIVEBODYaGVyZUlzU2l4dHlGb3VyQ2hhcmFjdGVyc09mQmFzZTY0RGF0\n\
                   -----END CERTIFICATE-----\n";
        let check = ca_check(Some(pem)).expect("a certificate is set, so it is checked");
        assert_eq!(
            check.guest,
            std::path::PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
            "the check has to read the bundle the toolchains read, not the \
             file omh itself wrote"
        );
        match &check.expect {
            Expect::Mentions(what) => assert!(
                what.iter().any(|w| w.contains("DISTINCTIVEBODY")),
                "the check must look for this certificate, not any certificate: {what:?}"
            ),
            other => panic!("a trust store is read, not {other:?}"),
        }
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

    /// A repo that turns `codegraph` off still gets its hooks checked.
    ///
    /// This was true by accident until 2026.08 and then silently stopped being
    /// true. `git-unavailable` was the one shipped hook outside `codegraph`, so
    /// `own.hooks` was never empty; retiring it made every shipped hook a
    /// `codegraph` one, and the gate keyed on `own.hooks` began skipping the
    /// whole capability for anyone with the feature off — including the repo's
    /// own hooks layer, checked by nothing and reported as a clean run.
    ///
    /// The old test asserted `hooks` is checked under *default* settings, which
    /// is exactly the configuration where the accident held.
    #[test]
    fn hooks_are_checked_even_with_every_omh_hook_switched_off() {
        let fx = fixture();
        let none = crate::base::Own {
            hooks: Vec::new(),
            ..decided().0
        };
        let caps: Vec<String> = checks(
            &fx.profile,
            &adapter("opencode"),
            &none,
            &decided().1,
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
        assert!(
            caps.iter().any(|c| c == "hooks"),
            "a mounted hooks document nobody checks is what doctor is for: {caps:?}"
        );
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

    /// A generated **program** is checked by parsing it, on every harness that
    /// emits one.
    ///
    /// `NonEmptyFile` passes for a module with a syntax error, for one where
    /// every hook was dropped, and for one that throws on every event — which
    /// is why `Parses` exists. A second plugin render was added to that arm and
    /// nothing asserted it had been: downgrading omp's entry to `NonEmptyFile`
    /// left the suite green, so omh had no evidence anywhere that it stages
    /// valid JavaScript for omp.
    #[test]
    fn every_generated_program_is_checked_by_parsing_it() {
        let fx = fixture();
        for harness in ["opencode", "omp"] {
            let cs = checks(
                &fx.profile,
                &adapter(harness),
                &decided().0,
                &decided().1,
                &Default::default(),
            )
            .unwrap();
            let hooks = cs
                .iter()
                .find(|c| c.name == "hooks")
                .unwrap_or_else(|| panic!("{harness} stages a hooks module: {cs:?}"));
            assert!(
                matches!(hooks.expect, Expect::Parses(_)),
                "{harness} emits a program, so it must be parsed: {:?}",
                hooks.expect
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

    /// A server omh provisioned but that cannot start is invisible from the
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

    /// Run the login probe against a fake harness with a chosen answer.
    ///
    /// `code` and `err` are separate arguments because the defect this fixture
    /// was written for lived exactly in their being conflated.
    fn login_probe_against(out: &str, err: &str, code: i32) -> Outcome {
        let stub = tempfile::tempdir().unwrap();
        let at = stub.path().join("harness");
        std::fs::write(
            &at,
            format!(
                "#!/bin/sh\ncat <<'EOF'\n{out}\nEOF\ncat >&2 <<'EOF'\n{err}\nEOF\nexit {code}\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = probe_script(&[Check {
            name: "login".into(),
            guest: stub.path().to_path_buf(),
            expect: Expect::Answers {
                command: "harness usage --json".into(),
                ready: "accountId".into(),
            },
            dir: true,
        }]);
        let sh = std::process::Command::new("sh")
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
        let outcomes = parse(&String::from_utf8_lossy(&sh.stdout));
        assert_eq!(outcomes.len(), 1, "one check, one line: {script}");
        outcomes.into_iter().next().unwrap()
    }

    #[test]
    fn a_harness_that_names_an_account_passes_the_login_probe() {
        let out = login_probe_against(r#"{"reports":[{"accountId":"me@example.com"}]}"#, "", 0);
        assert!(out.ok, "a real account must pass: {out:?}");
    }

    #[test]
    fn a_harness_with_no_accounts_fails_the_login_probe() {
        let out = login_probe_against(r#"{"reports":[]}"#, "", 0);
        assert!(!out.ok, "an empty report list is not a login: {out:?}");
    }

    /// The defect: a probe that **failed** was read as a login.
    ///
    /// The arm never looked at the exit status and folded stderr into the text
    /// it grepped, so a harness that errored out was judged by whatever words
    /// its error happened to contain — and `accountId` is exactly the word an
    /// authentication error or a usage message names. Reproduced before the
    /// fix: a command exiting 1 whose stderr read "run /login to obtain an
    /// accountId" reported `ok`.
    ///
    /// This is the false positive `token-probe` was added to remove, arriving
    /// through the check that replaced it.
    #[test]
    fn a_probe_that_failed_is_never_a_login() {
        let out = login_probe_against(
            "",
            "error: no authenticated account; run /login to obtain an accountId",
            1,
        );
        assert!(
            !out.ok,
            "the marker appeared in an error, on a failed run: {out:?}"
        );
    }

    /// A missing subcommand is a broken probe, not a missing login, and the
    /// report has to carry enough of the harness's own words to tell them
    /// apart — that being the whole justification for this arm quoting output.
    #[test]
    fn a_broken_probe_says_what_the_harness_said() {
        let out = login_probe_against("", "usage: harness [options]\nunknown flag: --json", 2);
        assert!(!out.ok, "a broken probe cannot report a login: {out:?}");
        assert!(
            out.detail.contains("usage:") || out.detail.contains("unknown flag"),
            "the harness's own words must survive into the report: {out:?}"
        );
    }

    /// A multi-line answer must not break the tab-separated wire format.
    ///
    /// `parse` reads one outcome per line, so an unflattened multi-line error
    /// would be read as extra checks — the failure `omp.toml` records for the
    /// `omp -p '/mcp list'` verify command, which swallowed the next check's
    /// line and made a seven-check run report six.
    #[test]
    fn a_multi_line_failure_stays_one_protocol_line() {
        let out = login_probe_against("", "line one\nline two\nline three", 1);
        assert!(!out.ok);
        assert!(
            !out.detail.contains('\n'),
            "the detail must be flattened: {out:?}"
        );
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
