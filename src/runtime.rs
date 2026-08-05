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
    fn name(&self) -> &'static str;
    /// Executable to invoke.
    fn program(&self) -> &'static str;
    fn caps(&self) -> Caps;
    /// Arguments after the program name.
    fn args(&self, plan: &Plan) -> Vec<String>;
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
        Caps { file_mounts: true, free_guest_paths: true }
    }

    fn args(&self, plan: &Plan) -> Vec<String> {
        let mut a: Vec<String> = vec!["run".into(), "--rm".into(), "-it".into()];
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
        Caps { file_mounts: false, free_guest_paths: false }
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
        anyhow::bail!("no container runtime found — install one of: {}", NAMES.join(", "));
    }

    let Some(runtime) = build(preference) else {
        anyhow::bail!("unknown runtime `{preference}` — expected one of: {}", NAMES.join(", "));
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
        assert!(!c.free_guest_paths, "sbx mounts workspaces at their host path");
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
        for caps in [Docker.caps(), Caps { file_mounts: false, free_guest_paths: true }] {
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

        plan.validate(&Docker.caps()).expect("docker supports file mounts");

        let err = plan.validate(&Sbx.caps()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(".mcp.json"), "must name the offending mount: {msg}");
    }

    #[test]
    fn a_plan_relocating_guest_paths_is_refused_when_paths_are_not_free() {
        let caps = Caps { file_mounts: true, free_guest_paths: false };
        let err = sample_plan().validate(&caps).unwrap_err();
        assert!(format!("{err:#}").contains("/work"), "got: {err:#}");
    }

    // ── argument construction ───────────────────────────────────────────────

    #[test]
    fn docker_passes_every_mount_and_preserves_read_only() {
        let plan = sample_plan();
        let args = Docker.args(&plan).join(" ");
        for m in &plan.mounts {
            assert!(args.contains(&m.host.display().to_string()), "missing {m:?}");
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
            assert!(args.contains(&m.host.display().to_string()), "missing {m:?}");
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
}
