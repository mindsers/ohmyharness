//! Runtime backends.
//!
//! Isolation is not ours to build — but it is also not one vendor's to own. A
//! `Plan` is a pure description (mounts, env, argv), so a backend is just a
//! translation of that into one process invocation.
//!
//! Backends differ in ways that would otherwise break a plan mysteriously, so
//! each **declares** its capabilities and a plan is validated against them
//! before launch. An `sbx` sandbox that cannot mount a single file must say so,
//! not silently drop the profile.

use crate::container::Plan;
use anyhow::Result;

/// What a backend can actually do. Unknowns default to `false`: a distribution
/// that guesses optimistically about isolation is worse than one that refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Can bind-mount an individual file, not only a directory.
    pub file_mounts: bool,
    /// omh chooses the guest path. When false, mounts land at their host path
    /// and conventions like `/work` do not survive.
    pub free_guest_paths: bool,
}

pub trait Runtime: std::fmt::Debug {
    /// Backend identity, for error messages that name which runtime refused a
    /// plan. Implemented by both backends and unused until `sbx` is selectable.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
    /// Executable to invoke.
    fn program(&self) -> &'static str;
    fn caps(&self) -> Caps;
    /// Arguments after the program name.
    fn args(&self, plan: &Plan) -> Vec<String>;

    /// Start the session container detached, publishing sshd on loopback.
    fn up_args(&self, plan: &Plan, name: &str, port: u16, pubkey: &str) -> Vec<String>;

    /// Run something inside an already-running session.
    fn exec_args(&self, name: &str, argv: &[String], tty: bool) -> Vec<String>;

    /// Run something inside the session that must outlive the caller.
    ///
    /// A process started by a `docker exec` we spawn and abandon dies with the
    /// client — verified: the backgrounded server survived and the foreground
    /// bridge did not.
    /// Detached exec into a running session. Both backends implement it and
    /// the launcher does not yet call it — harnesses go through `dtach`, which
    /// owns detachment today.
    #[allow(dead_code)]
    fn exec_detached_args(&self, name: &str, argv: &[String]) -> Vec<String>;
}

#[derive(Debug)]
pub struct Docker;

#[derive(Debug)]
pub struct Sbx;

pub const NAMES: [&str; 2] = ["docker", "sbx"];

impl Runtime for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }
    fn program(&self) -> &'static str {
        "docker"
    }
    fn caps(&self) -> Caps {
        Caps {
            file_mounts: true,
            free_guest_paths: true,
        }
    }

    fn args(&self, plan: &Plan) -> Vec<String> {
        let mut a: Vec<String> = vec!["run".into(), "--rm".into()];
        if plan.tty {
            a.push("-it".into());
        }
        for m in &plan.mounts {
            a.push("-v".into());
            a.push(format!(
                "{}:{}{}",
                m.host.display(),
                m.guest.display(),
                if m.read_only { ":ro" } else { "" }
            ));
        }
        for (k, v) in &plan.env {
            a.push("-e".into());
            a.push(format!("{k}={v}"));
        }
        a.extend(["--network".into(), plan.network.clone()]);
        a.extend(["-w".into(), plan.workdir.clone()]);
        a.push(plan.image.clone());
        a.extend(plan.argv.iter().cloned());
        a
    }

    fn up_args(&self, plan: &Plan, name: &str, port: u16, pubkey: &str) -> Vec<String> {
        // Detached and unnamed by --rm: a session must outlive the terminal
        // that started it, or `omh code` has nothing to attach to.
        let mut a: Vec<String> = vec!["run".into(), "-d".into(), "--name".into(), name.into()];
        // Loopback only. On 0.0.0.0 this publishes a shell inside the sandbox
        // to the local network.
        // Loopback only. On 0.0.0.0 this publishes a shell inside the sandbox
        // to the local network.
        a.push("-p".into());
        a.push(format!("127.0.0.1:{port}:22"));
        for m in &plan.mounts {
            a.push("-v".into());
            a.push(format!(
                "{}:{}{}",
                m.host.display(),
                m.guest.display(),
                if m.read_only { ":ro" } else { "" }
            ));
        }
        for (k, v) in &plan.env {
            a.push("-e".into());
            a.push(format!("{k}={v}"));
        }
        a.push("-e".into());
        a.push(format!("OMH_PUBKEY={pubkey}"));
        a.extend(["--network".into(), plan.network.clone()]);
        a.extend(["-w".into(), plan.workdir.clone()]);
        a.push(plan.image.clone());
        a.push("omh-session".into());
        a
    }

    fn exec_detached_args(&self, name: &str, argv: &[String]) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "exec".into(),
            "-d".into(),
            "-u".into(),
            "agent".into(),
            "-w".into(),
            crate::container_workdir().into(),
        ];
        a.push(name.into());
        a.extend(argv.iter().cloned());
        a
    }

    fn exec_args(&self, name: &str, argv: &[String], tty: bool) -> Vec<String> {
        let mut a: Vec<String> = vec!["exec".into()];
        if tty {
            a.push("-it".into());
        }
        // Never root: the session's PID 1 needs privilege to run sshd, the
        // agent does not.
        a.extend([
            "-u".into(),
            "agent".into(),
            "-w".into(),
            crate::container_workdir().into(),
        ]);
        a.push(name.into());
        a.extend(argv.iter().cloned());
        a
    }
}

impl Runtime for Sbx {
    fn name(&self) -> &'static str {
        "sbx"
    }
    fn program(&self) -> &'static str {
        "sbx"
    }

    fn caps(&self) -> Caps {
        // Both unverified. Docker's docs describe workspaces mounting at their
        // host path and say nothing about single files, so assume neither until
        // the spike proves otherwise. A wrong `true` here would start a sandbox
        // with the profile silently missing.
        Caps {
            file_mounts: false,
            free_guest_paths: false,
        }
    }

    fn args(&self, plan: &Plan) -> Vec<String> {
        // PROVISIONAL. The exact sbx flag names are an open question; only the
        // information carried is asserted by tests, not this shape. Egress and
        // credential handling are deliberately absent — sbx owns both, which is
        // the reason to use it.
        let mut a: Vec<String> = vec!["run".into()];
        for m in &plan.mounts {
            a.push("--workspace".into());
            a.push(format!(
                "{}{}",
                m.host.display(),
                if m.read_only { ":ro" } else { "" }
            ));
        }
        for (k, v) in &plan.env {
            a.push("--env".into());
            a.push(format!("{k}={v}"));
        }
        a.push("--".into());
        a.extend(plan.argv.iter().cloned());
        a
    }

    fn up_args(&self, plan: &Plan, name: &str, port: u16, pubkey: &str) -> Vec<String> {
        // PROVISIONAL — sbx session semantics are part of the open spike.
        let mut a: Vec<String> = vec![
            "run".into(),
            "--detach".into(),
            "--name".into(),
            name.into(),
        ];
        a.push("--publish".into());
        a.push(format!("127.0.0.1:{port}:22"));
        for m in &plan.mounts {
            a.push("--workspace".into());
            a.push(format!(
                "{}{}",
                m.host.display(),
                if m.read_only { ":ro" } else { "" }
            ));
        }
        a.push("--env".into());
        a.push(format!("OMH_PUBKEY={pubkey}"));
        a.push("--".into());
        a.push("omh-session".into());
        a
    }

    fn exec_detached_args(&self, name: &str, argv: &[String]) -> Vec<String> {
        // PROVISIONAL, like the rest of the sbx backend.
        let mut a: Vec<String> = vec!["exec".into(), "--detach".into(), name.into(), "--".into()];
        a.extend(argv.iter().cloned());
        a
    }

    fn exec_args(&self, name: &str, argv: &[String], tty: bool) -> Vec<String> {
        let mut a: Vec<String> = vec!["exec".into()];
        if tty {
            a.push("-it".into());
        }
        a.push(name.into());
        a.push("--".into());
        a.extend(argv.iter().cloned());
        a
    }
}

/// `auto` prefers the stronger isolation when it is installed.
pub fn select(preference: &str, available: &dyn Fn(&str) -> bool) -> Result<Box<dyn Runtime>> {
    let build = |name: &str| -> Option<Box<dyn Runtime>> {
        match name {
            "docker" => Some(Box::new(Docker)),
            "sbx" => Some(Box::new(Sbx)),
            _ => None,
        }
    };

    if preference == "auto" {
        // Strongest first.
        for name in ["sbx", "docker"] {
            if available(name) {
                return Ok(build(name).expect("name from the known list"));
            }
        }
        anyhow::bail!(
            "no container runtime found — install one of: {}",
            NAMES.join(", ")
        );
    }

    let Some(runtime) = build(preference) else {
        anyhow::bail!(
            "unknown runtime `{preference}` — expected one of: {}",
            NAMES.join(", ")
        );
    };
    if !available(preference) {
        anyhow::bail!("runtime `{preference}` is not installed");
    }
    Ok(runtime)
}

/// Real availability check, for the non-test path.
pub fn installed(program: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::Mount;
    use std::path::PathBuf;

    fn plan_with(mounts: Vec<Mount>) -> Plan {
        Plan {
            image: "omh/claude:latest".into(),
            mounts,
            env: vec![("OMH_SESSION".into(), "s01".into())],
            network: "omh-repo".into(),
            workdir: "/work".into(),
            argv: vec!["claude".into()],
            dropped: vec![],
            tty: true,
        }
    }

    fn dir_mount(host: &str, guest: &str, read_only: bool) -> Mount {
        Mount {
            host: PathBuf::from(host),
            guest: PathBuf::from(guest),
            read_only,
            file: false,
        }
    }

    fn sample_plan() -> Plan {
        plan_with(vec![
            dir_mount("/host/worktree", "/work", false),
            dir_mount("/host/skills", "/home/agent/.claude/skills", true),
        ])
    }

    // ── capabilities ────────────────────────────────────────────────────────

    #[test]
    fn docker_can_do_everything_the_current_design_assumes() {
        let c = Docker.caps();
        assert!(c.file_mounts);
        assert!(c.free_guest_paths);
    }

    /// Docker's docs describe workspace mounts landing at the host path and say
    /// nothing about single files. Until the spike proves otherwise, both are
    /// false — a wrong `true` here silently drops the profile.
    #[test]
    fn sbx_capabilities_stay_conservative_until_verified() {
        let c = Sbx.caps();
        assert!(!c.file_mounts, "unverified: assume no");
        assert!(
            !c.free_guest_paths,
            "sbx mounts workspaces at their host path"
        );
    }

    // ── selection ───────────────────────────────────────────────────────────

    fn only(present: &'static str) -> impl Fn(&str) -> bool {
        move |p: &str| p == present
    }

    #[test]
    fn auto_prefers_the_stronger_isolation() {
        let r = select("auto", &|_| true).unwrap();
        assert_eq!(r.name(), "sbx");
    }

    #[test]
    fn auto_falls_back_when_sbx_is_absent() {
        let r = select("auto", &only("docker")).unwrap();
        assert_eq!(r.name(), "docker");
    }

    #[test]
    fn auto_fails_clearly_when_nothing_is_installed() {
        let err = select("auto", &|_| false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("docker") && msg.contains("sbx"), "got: {msg}");
    }

    /// The opinion must be escapable — that is the whole reason backends stay
    /// plural rather than omh just adopting the better one.
    #[test]
    fn an_explicit_choice_overrides_detection() {
        let r = select("docker", &|_| true).unwrap();
        assert_eq!(r.name(), "docker");
    }

    #[test]
    fn an_explicit_choice_that_is_not_installed_is_an_error() {
        let err = select("sbx", &only("docker")).unwrap_err();
        assert!(err.to_string().contains("sbx"), "got: {err}");
    }

    #[test]
    fn an_unknown_runtime_lists_the_real_ones() {
        let err = select("podman", &|_| true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("docker") && msg.contains("sbx"), "got: {msg}");
    }

    // ── validation ──────────────────────────────────────────────────────────

    #[test]
    fn a_directory_only_plan_passes_on_any_backend() {
        for caps in [
            Docker.caps(),
            Caps {
                file_mounts: false,
                free_guest_paths: true,
            },
        ] {
            sample_plan().validate(&caps).unwrap();
        }
    }

    /// The loud-failure requirement: `sbx` must refuse a plan it cannot honour
    /// rather than starting a sandbox where the profile silently isn't there.
    #[test]
    fn a_plan_needing_file_mounts_is_refused_by_a_backend_without_them() {
        let plan = plan_with(vec![
            dir_mount("/host/worktree", "/work", false),
            Mount {
                host: PathBuf::from("/host/run/mcp.rendered"),
                guest: PathBuf::from("/home/agent/.mcp.json"),
                read_only: true,
                file: true,
            },
        ]);

        plan.validate(&Docker.caps())
            .expect("docker supports file mounts");

        let err = plan.validate(&Sbx.caps()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(".mcp.json"),
            "must name the offending mount: {msg}"
        );
    }

    #[test]
    fn a_plan_relocating_guest_paths_is_refused_when_paths_are_not_free() {
        let caps = Caps {
            file_mounts: true,
            free_guest_paths: false,
        };
        let err = sample_plan().validate(&caps).unwrap_err();
        assert!(format!("{err:#}").contains("/work"), "got: {err:#}");
    }

    // ── argument construction ───────────────────────────────────────────────

    /// doctor captures the probe's output, so it must not request a terminal —
    /// docker refuses `-it` when stdin is not a TTY, which would make the check
    /// fail for reasons unrelated to the adapter.
    #[test]
    fn a_plan_without_a_tty_does_not_ask_for_one() {
        let mut plan = sample_plan();
        plan.tty = false;
        let args = Docker.args(&plan);
        assert!(!args.contains(&"-it".to_string()), "got: {args:?}");

        plan.tty = true;
        assert!(Docker.args(&plan).contains(&"-it".to_string()));
    }

    #[test]
    fn docker_passes_every_mount_and_preserves_read_only() {
        let plan = sample_plan();
        let args = Docker.args(&plan).join(" ");
        for m in &plan.mounts {
            assert!(
                args.contains(&m.host.display().to_string()),
                "missing {m:?}"
            );
        }
        assert_eq!(args.matches(":ro").count(), 1);
        assert!(args.contains("-w /work"));
        assert!(args.ends_with("claude"), "harness argv comes last: {args}");
    }

    /// Asserted as properties, not as an exact command line: the precise `sbx`
    /// CLI shape is one of the open questions the spike resolves, and pinning a
    /// string here would encode a guess as a requirement.
    #[test]
    fn sbx_carries_the_same_information_as_docker() {
        let plan = sample_plan();
        let args = Sbx.args(&plan).join(" ");
        for m in &plan.mounts {
            assert!(
                args.contains(&m.host.display().to_string()),
                "missing {m:?}"
            );
        }
        assert!(args.contains(":ro"), "read-only mounts must stay read-only");
        assert!(args.ends_with("claude"), "harness argv comes last: {args}");
    }

    /// The security invariant has to hold on every backend, not just the one
    /// that happened to be written first.
    #[test]
    fn no_backend_may_widen_write_access() {
        let plan = sample_plan();
        let writable: Vec<_> = plan.mounts.iter().filter(|m| !m.read_only).collect();
        assert_eq!(writable.len(), 1);

        for backend in [&Docker as &dyn Runtime, &Sbx as &dyn Runtime] {
            let args = backend.args(&plan).join(" ");
            let ro = args.matches(":ro").count();
            assert_eq!(
                ro,
                plan.mounts.len() - 1,
                "{} dropped a read-only marker",
                backend.name()
            );
        }
    }

    // ── session containers ──────────────────────────────────────────────────

    /// sshd on 0.0.0.0 publishes a shell inside the sandbox to the local
    /// network — the exact inverse of what this project is for.
    #[test]
    fn the_session_publishes_ssh_on_loopback_only() {
        let joined = Docker
            .up_args(&sample_plan(), "omh-repo-s01", 49200, "ssh-ed25519 AAA")
            .join(" ");
        assert!(joined.contains("127.0.0.1:49200:22"), "got: {joined}");
        assert!(!joined.contains("0.0.0.0"));
    }

    #[test]
    fn published_ports_are_loopback_only() {
        let joined = Docker.up_args(&sample_plan(), "n", 49200, "k").join(" ");
        assert!(!joined.contains("0.0.0.0"), "got: {joined}");
        assert_eq!(
            joined.matches("127.0.0.1:").count(),
            1,
            "ssh only: {joined}"
        );
    }

    #[test]
    fn the_session_runs_detached_and_named() {
        let args = Docker.up_args(&sample_plan(), "omh-repo-s01", 49200, "k");
        assert!(
            args.contains(&"-d".to_string()),
            "must outlive the terminal: {args:?}"
        );
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--name" && w[1] == "omh-repo-s01"));
        assert!(
            !args.contains(&"--rm".to_string()),
            "a session must survive its launch"
        );
    }

    /// The session carries the same mounts as a one-shot launch, or the harness
    /// execed into it later sees no profile.
    #[test]
    fn the_session_carries_every_mount() {
        let plan = sample_plan();
        let joined = Docker.up_args(&plan, "n", 1, "k").join(" ");
        for m in &plan.mounts {
            assert!(
                joined.contains(&m.host.display().to_string()),
                "missing {m:?}"
            );
        }
        assert_eq!(joined.matches(":ro").count(), plan.mounts.len() - 1);
    }

    #[test]
    fn the_public_key_reaches_the_session() {
        let joined = Docker
            .up_args(&sample_plan(), "n", 1, "ssh-ed25519 AAAkey")
            .join(" ");
        assert!(joined.contains("ssh-ed25519 AAAkey"), "got: {joined}");
    }

    #[test]
    fn exec_targets_the_named_session_and_runs_unprivileged() {
        let args = Docker.exec_args("omh-repo-s01", &["claude".into()], true);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"omh-repo-s01".to_string()));
        assert!(
            args.windows(2).any(|w| w[0] == "-u" && w[1] == "agent"),
            "got: {args:?}"
        );
        assert_eq!(args.last().unwrap(), "claude");
    }

    /// Regression: a service started through a spawned-and-abandoned
    /// `docker exec` dies with the client. The server survived because it was
    /// backgrounded inside the shell; the foreground bridge did not.
    #[test]
    fn a_detached_exec_outlives_the_caller() {
        let args = Docker.exec_detached_args("omh-repo-s01", &["sh".into()]);
        assert!(args.contains(&"-d".to_string()), "got: {args:?}");
        assert!(args.contains(&"omh-repo-s01".to_string()));
    }

    #[test]
    fn exec_asks_for_a_terminal_only_when_there_is_one() {
        assert!(Docker
            .exec_args("n", &["x".into()], true)
            .contains(&"-it".to_string()));
        assert!(!Docker
            .exec_args("n", &["x".into()], false)
            .contains(&"-it".to_string()));
    }
}

#[cfg(test)]
mod workdir_tests {
    /// `container_workdir`'s docstring says the path is named once, so the note
    /// store and the launch plan cannot disagree about it. It was not: the plan
    /// spelled it twice and the note store's path folding a third time.
    ///
    /// Introspects the source, the way the `RESERVED` guard introspects the CLI
    /// definition, because the obvious test cannot fail. Asserting that the
    /// plan's workdir equals `container_workdir()` passes just as well when the
    /// plan holds the literal — both sides are the same string, so it pins
    /// nothing. Only counting the spellings can tell them apart.
    #[test]
    fn only_one_place_spells_the_container_workdir() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).unwrap();
                // Fixtures may say it; the shipped path may not.
                let production = body.split("#[cfg(test)]").next().unwrap_or("");
                for (i, line) in production.lines().enumerate() {
                    if line.contains("\"/work\"") {
                        let name = path.file_name().unwrap().to_string_lossy().to_string();
                        offenders.push(format!("{name}:{}", i + 1));
                    }
                }
            }
        }
        assert_eq!(
            offenders.len(),
            1,
            "exactly one place may spell it — `container_workdir` itself. Found: {offenders:?}"
        );
    }
}
