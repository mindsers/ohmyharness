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

    /// How to list this runtime's named volumes, if it has such a notion.
    ///
    /// `None` for anything omh has not measured — the same posture `Sbx::caps`
    /// takes, which answers `false` for capabilities nobody has verified. Not
    /// literally the same answer: `false` there refuses a plan, `None` here
    /// skips a check.
    ///
    /// Docker's volumes are the ones `risks.md` recorded as orphaned and
    /// unmentioned after a migration — omh creates `omh-cache-<repo-id>` and,
    /// until this method, listed it nowhere.
    fn volume_args(&self) -> Option<Vec<String>> {
        None
    }

    /// List every container, running or not.
    ///
    /// Separate from `running_args`, which answers "is this one up". A stopped
    /// container is exactly the leftover this exists to find, so a listing that
    /// only shows running ones would report a machine as clean while six sit
    /// there. Same contract: **one name per line, or a non-zero exit because
    /// the question could not be answered.**
    fn running_all_args(&self) -> Option<Vec<String>> {
        None
    }

    /// List the names of every network.
    ///
    /// `None` for anything omh has not measured, as above — a runtime whose
    /// networks omh has never looked at must report *did not look*, never an
    /// empty listing.
    fn network_args(&self) -> Option<Vec<String>> {
        None
    }

    /// List the ids of omh images that record which checkout they were built
    /// for.
    ///
    /// Only stack images carry `omh.repo` — a base or harness image is shared
    /// across checkouts and belongs to no one of them, so attributing it to a
    /// checkout would be an invention. Two calls rather than one because
    /// docker's `--format` cannot read a label on an image: `{{.Label "x"}}`
    /// fails with *can't evaluate field Label in type imageContext*. Measured.
    fn image_args(&self) -> Option<Vec<String>> {
        None
    }

    /// List the names of every container that is running.
    ///
    /// On the trait because how a runtime spells this is its business, and the
    /// contract is the one `image::running_from` reads: **exit zero and print
    /// one name per line, or exit non-zero because the question could not be
    /// answered.** Nothing else.
    ///
    /// It lists rather than asks about one, and that is the correction rather
    /// than the design. Asking about one meant handing the name to
    /// `--filter name=`, which is a *regex* — measured, not assumed: docker
    /// permits `.` in a container name and `Paths::container` builds one from
    /// the checkout's directory basename unsanitised, so a repo in `~/src/
    /// omh.rs` probes with `^omh-omh.rs-s01$` and matches `omh-omhXrs-s01`
    /// too. Worse, a filter that does not parse as a regex exits **0 with
    /// nothing on stdout** — indistinguishable from *not running*, which is
    /// the collapse this whole type exists to remove, reintroduced by the
    /// probe meant to fix it.
    ///
    /// Escaping the name would close both, and would leave the correctness of
    /// every future call resting on somebody remembering to escape. Listing
    /// and comparing in Rust has no pattern language in it at all.
    fn running_args(&self) -> Vec<String>;

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

/// What a session container may do, as Linux capabilities.
///
/// Everything is dropped and these come back, each for a reason the entrypoint
/// or the agent's ordinary work needs. Docker's default grants fourteen; the
/// six it grants that are not here — `FSETID`, `SETPCAP`, `NET_RAW`, `SYS_CHROOT`,
/// `MKNOD`, `SETFCAP` — have no caller in the image, and `NET_RAW` in particular
/// is what raw-socket tooling would need to spoof traffic on the session
/// network. Anything not on the list is a request to add it, with its reason,
/// here.
pub const SESSION_CAPS: &[&str] = &[
    // `sudo` in the entrypoint: becoming root to start sshd, and sshd itself
    // dropping to `agent` for the session.
    "SETUID",
    "SETGID",
    // The entrypoint's `chmod`/`chown` on files the bind mounts land with host
    // ownership, and `install -o root` for the host key.
    "CHOWN",
    "FOWNER",
    "DAC_OVERRIDE",
    // sshd signals its own children; `omh down` and the idle reaper stop
    // processes the agent left running.
    "KILL",
    // sshd binds 22.
    "NET_BIND_SERVICE",
    // PAM writes the login record on an ssh session; without it the attach
    // connects and is dropped with `pam_loginuid` in the log.
    "AUDIT_WRITE",
];

/// How many processes a session may hold. A fork bomb — or a test runner that
/// has lost track of its children — stops at the sandbox wall instead of the
/// host's. Two thousand is room for a large build with a watcher beside it;
/// it is a chosen constant, not a measured limit, and it is here so that the
/// day it is wrong the fix is one line with a reason next to it.
pub const PIDS_LIMIT: u32 = 2048;

/// The confinement every Docker `run` carries: the capability set, the process
/// ceiling, and whatever `sandbox_memory`/`sandbox_cpus` ask for.
///
/// Not `--security-opt=no-new-privileges`: the entrypoint's `sudo` needs to
/// gain privilege, and the decision was to keep sudo. A test pins that reason.
fn confinement(plan: &Plan) -> Vec<String> {
    let mut a: Vec<String> = vec!["--cap-drop=ALL".into()];
    a.extend(SESSION_CAPS.iter().map(|c| format!("--cap-add={c}")));
    a.extend(["--pids-limit".into(), PIDS_LIMIT.to_string()]);
    if let Some(memory) = &plan.limits.memory {
        a.extend(["--memory".into(), memory.clone()]);
    }
    if let Some(cpus) = &plan.limits.cpus {
        a.extend(["--cpus".into(), cpus.clone()]);
    }
    a
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

    /// Measured 2026-08-24 against docker 29.7.2. `ps` without `-a` is the
    /// running set,
    /// which is the question; `{{.Names}}` is one name per line, and a docker
    /// container name cannot contain a newline — the legal charset is
    /// `[a-zA-Z0-9][a-zA-Z0-9_.-]*`, which docker enforces at creation.
    fn running_args(&self) -> Vec<String> {
        vec!["ps".into(), "--format".into(), "{{.Names}}".into()]
    }

    /// `{{.Name}}` for the reason `running_args` uses `{{.Names}}`: one
    /// per line, and a volume name cannot contain a newline either.
    fn volume_args(&self) -> Option<Vec<String>> {
        Some(vec![
            "volume".into(),
            "ls".into(),
            "--format".into(),
            "{{.Name}}".into(),
        ])
    }

    /// `-a`, which is the whole point: a stopped container is the leftover.
    fn running_all_args(&self) -> Option<Vec<String>> {
        Some(vec![
            "ps".into(),
            "-a".into(),
            "--format".into(),
            "{{.Names}}".into(),
        ])
    }

    /// `{{.Name}}`, one per line, as for volumes.
    fn network_args(&self) -> Option<Vec<String>> {
        Some(vec![
            "network".into(),
            "ls".into(),
            "--format".into(),
            "{{.Name}}".into(),
        ])
    }

    fn image_args(&self) -> Option<Vec<String>> {
        Some(vec![
            "images".into(),
            "--filter".into(),
            "label=omh.repo".into(),
            "--format".into(),
            "{{.ID}}".into(),
        ])
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
        // Never fetch: see `a_launch_never_fetches_an_image_it_could_not_find`.
        let mut a: Vec<String> = vec!["run".into(), "--rm".into(), "--pull=never".into()];
        a.extend(confinement(plan));
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
        // that started it, or `omh s attach` has nothing to attach to.
        let mut a: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            // Never fetch: see
            // `a_launch_never_fetches_an_image_it_could_not_find`.
            "--pull=never".into(),
            // A real init as PID 1. The entrypoint ends in `exec sleep
            // infinity`, which reaps nothing, so every `dtach` that exited left
            // a zombie behind for the life of the session. Harmless one at a
            // time and unbounded over days — and it makes the process table
            // useless for asking what is still running, which is why
            // `persist::live` reads sockets instead.
            "--init".into(),
            "--name".into(),
            name.into(),
        ];
        a.extend(confinement(plan));
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
        // What this container is made of, so a later launch can tell whether it
        // is still the session being asked for. See `Plan::labels`.
        for (key, value) in plan.labels() {
            a.push("--label".into());
            a.push(format!("{key}={value}"));
        }
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

    /// PROVISIONAL, like everything else here — sbx is not installed on the
    /// machine this was written on, so the docker spelling is assumed rather
    /// than measured.
    ///
    /// An earlier draft claimed the failure direction is safe, which is true
    /// of exactly one failure: an unrecognised flag exits non-zero and reads
    /// as *could not tell*. The others are not safe and the claim should not
    /// have been made. If sbx exits 0 while naming containers some other way,
    /// every sandbox reads as **not running** — the original bug, on the
    /// backend `select` prefers. If it prints a header, every sandbox reads as
    /// running and `sync` is wedged.
    ///
    /// Left as the docker spelling rather than made to refuse outright,
    /// because refusing would take sbx from *unverified* to *unusable*. What
    /// closes it is measuring against a real sbx, and `omh doctor` is where
    /// that check belongs once there is one to measure.
    fn running_args(&self) -> Vec<String> {
        vec!["ps".into(), "--format".into(), "{{.Names}}".into()]
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
        // PROVISIONAL like the rest of this backend, but carried rather than
        // dropped: a plan's stamp is information, and no backend may lose it.
        for (key, value) in plan.labels() {
            a.push("--label".into());
            a.push(format!("{key}={value}"));
        }
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

/// The backend the settings ask for, or the one measured backend under `auto`.
pub fn select(preference: &str, available: &dyn Fn(&str) -> bool) -> Result<Backend> {
    let build = |name: &str| -> Option<Box<dyn Runtime>> {
        match name {
            "docker" => Some(Box::new(Docker)),
            "sbx" => Some(Box::new(Sbx)),
            _ => None,
        }
    };

    if preference == "auto" {
        // Only the measured backend. `auto` used to prefer `sbx` when it was
        // present, and pick it silently — but the `Sbx` backend is
        // provisional and unmeasured (its `caps` are guessed, its staging
        // never run against a real `sbx`), so a machine that happened to have
        // Docker Sandboxes installed launched every session on a backend
        // nobody had exercised. `sbx` is an explicit opt-in until the spike in
        // `docs/design` measures it, so `auto` never chooses a backend nobody
        // has run.
        if available("docker") {
            return Ok(Backend::real(
                build("docker").expect("name from the known list"),
            ));
        }
        anyhow::bail!(
            "no measured container runtime found — install docker, or set \
             `runtime = \"sbx\"` to opt into the provisional Docker Sandboxes \
             backend (`omh doctor` to check it first)"
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
    Ok(Backend::real(runtime))
}

/// Real availability check, for the non-test path.
pub fn installed(program: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A selected runtime together with the way its program is run.
///
/// `Runtime` stays pure — a plan in, an argv out — and this is the one place
/// where an argv becomes a process. Production goes through `real`, which is
/// `Command::new(program).args(argv).output()` and nothing else. Tests go
/// through `scripted`, which answers each argv from a table and records what
/// was asked, so the launch path can be exercised on a machine with no
/// container runtime at all — which is how the two `attach` defects in the
/// 0.9.0 audit went unwritten: there was no place to put the test.
///
/// It derefs to the runtime it wraps, so `backend.exec_args(..)` and
/// `backend.program()` read as they always did. Interactive spawns — a tty
/// attach, a build that pipes a Dockerfile on stdin — still take `program()`
/// and build their own `Command`; everything that waits for an answer goes
/// through `output`.
pub struct Backend {
    rt: Box<dyn Runtime>,
    run: Box<Run>,
}

/// The runtime's argv, run and waited for.
type Run = dyn Fn(&[String]) -> std::io::Result<std::process::Output>;

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("rt", &self.rt)
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for Backend {
    type Target = dyn Runtime;
    fn deref(&self) -> &Self::Target {
        self.rt.as_ref()
    }
}

impl Backend {
    /// The runtime's program, run for real.
    pub fn real(rt: Box<dyn Runtime>) -> Backend {
        let program = rt.program();
        Backend {
            rt,
            run: Box::new(move |args| std::process::Command::new(program).args(args).output()),
        }
    }

    /// Run the runtime's program with these arguments and wait for it.
    pub fn output<S: AsRef<str>>(&self, args: &[S]) -> std::io::Result<std::process::Output> {
        let args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
        (self.run)(&args)
    }

    /// The runtime's argv, run through some other program — a shell script
    /// standing in for docker. For tests that want a real process to record
    /// what was asked and answer from a file, where `scripted` would have to
    /// know the argv in advance.
    #[cfg(test)]
    pub fn shimmed(rt: Box<dyn Runtime>, program: &std::path::Path) -> Backend {
        let program = program.to_path_buf();
        Backend {
            rt,
            run: Box::new(move |args| std::process::Command::new(&program).args(args).output()),
        }
    }

    /// A backend whose program is a table.
    ///
    /// Each entry is an argv prefix and the answer to give when the argv asked
    /// starts with it; the first match wins, so a specific script goes before
    /// a general one. Every argv is appended to the returned log, matched or
    /// not. An argv nothing scripted is a **panic naming it**, never a
    /// default answer: a test that runs a command it did not expect has found
    /// a behaviour it did not know about, and a silent success there is the
    /// exact collapse `Running::Unknown` exists to keep out of production.
    #[cfg(test)]
    pub fn scripted(
        rt: Box<dyn Runtime>,
        answers: Vec<(Vec<&str>, std::process::Output)>,
    ) -> (Backend, std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let asked = log.clone();
        let answers: Vec<(Vec<String>, std::process::Output)> = answers
            .into_iter()
            .map(|(prefix, out)| (prefix.into_iter().map(str::to_string).collect(), out))
            .collect();
        let run = move |args: &[String]| {
            asked.borrow_mut().push(args.to_vec());
            let hit = answers
                .iter()
                .find(|(prefix, _)| args.starts_with(prefix))
                .unwrap_or_else(|| panic!("nothing scripted for `{}`", args.join(" ")));
            Ok(std::process::Output {
                status: hit.1.status,
                stdout: hit.1.stdout.clone(),
                stderr: hit.1.stderr.clone(),
            })
        };
        (
            Backend {
                rt,
                run: Box::new(run),
            },
            log,
        )
    }
}

/// An answer for `Backend::scripted`: exit code, stdout, stderr.
#[cfg(test)]
pub fn answered(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
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
            limits: Default::default(),
            dropped: vec![],
            dropped_hooks: vec![],
            rules: Default::default(),
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

    /// `auto` never picks a backend nobody has measured. `sbx` present is not
    /// enough — the `Sbx` backend is provisional, so `auto` chooses docker or
    /// refuses, and `sbx` is reached only by asking for it.
    #[test]
    fn auto_never_picks_a_backend_nobody_has_measured() {
        // Both present: docker, not the unmeasured sbx.
        let r = select("auto", &|_| true).unwrap();
        assert_eq!(r.name(), "docker", "auto does not silently pick sbx");
        // Docker present: docker.
        let r = select("auto", &only("docker")).unwrap();
        assert_eq!(r.name(), "docker");
    }

    /// With only the unmeasured backend installed, `auto` refuses and names
    /// the explicit opt-in rather than choosing it silently.
    #[test]
    fn auto_with_only_sbx_refuses_and_names_the_opt_in() {
        let err = select("auto", &only("sbx")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("sbx") && msg.contains("doctor"),
            "it names the opt-in and how to check it: {msg}"
        );
    }

    #[test]
    fn auto_fails_clearly_when_nothing_is_installed() {
        let err = select("auto", &|_| false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("docker"), "got: {msg}");
    }

    /// The opinion must be escapable — that is the whole reason backends stay
    /// plural rather than omh just adopting the better one.
    #[test]
    fn an_explicit_choice_overrides_detection() {
        let r = select("docker", &|_| true).unwrap();
        assert_eq!(r.name(), "docker");
    }

    /// The whole thesis of the auto change: `sbx` is still reachable, just
    /// only by asking. A mutation that folded explicit `sbx` into the
    /// auto-refusal would leave the backend unreachable and survive every
    /// other test here — this is the one that catches it.
    #[test]
    fn an_explicit_sbx_still_selects_it() {
        let r = select("sbx", &only("sbx")).unwrap();
        assert_eq!(r.name(), "sbx", "sbx is reachable by naming it");
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

    /// The session runs with the capabilities it needs and no others.
    ///
    /// Docker's default set is fourteen capabilities chosen for containers in
    /// general; a sandbox whose whole job is to hold an agent that runs
    /// arbitrary code wants the list written down with a reason per entry.
    /// `SESSION_CAPS` is that list, and this pins the launch to it in both
    /// directions — nothing dropped that the entrypoint needs, nothing kept
    /// that it does not. The same for the one-off `run` doctor and auth use.
    #[test]
    fn the_session_drops_every_capability_it_does_not_name() {
        for args in [
            Docker.up_args(&sample_plan(), "n", 1, "k"),
            Docker.args(&sample_plan()),
        ] {
            assert!(
                args.iter().any(|a| a == "--cap-drop=ALL"),
                "everything goes first: {args:?}"
            );
            let added: std::collections::BTreeSet<&str> = args
                .iter()
                .filter_map(|a| a.strip_prefix("--cap-add="))
                .collect();
            assert_eq!(
                added,
                SESSION_CAPS.iter().copied().collect(),
                "then exactly the named set comes back"
            );
            for never in [
                "SYS_ADMIN",
                "NET_ADMIN",
                "SYS_PTRACE",
                "SYS_MODULE",
                "MKNOD",
                "NET_RAW",
                "SYS_RAWIO",
                "SYS_BOOT",
                "ALL",
            ] {
                assert!(!added.contains(never), "{never} must not be granted");
            }
            assert!(
                args.windows(2)
                    .any(|w| w[0] == "--pids-limit" && w[1] == PIDS_LIMIT.to_string()),
                "a fork bomb stops at the sandbox wall: {args:?}"
            );
        }
    }

    /// Why `no-new-privileges` is not on by default, pinned to the reason.
    ///
    /// The entrypoint runs sshd through `sudo`, and `no-new-privileges` is
    /// exactly the flag that makes a setuid binary not gain privilege. The two
    /// cannot both be true, and the decision was to keep sudo. This test goes
    /// red the day sudo leaves the entrypoint — at which point the flag should
    /// go on and this test should go.
    #[test]
    fn sudo_in_the_entrypoint_is_why_no_new_privileges_cannot_be_default() {
        let df = crate::image::base_dockerfile(None);
        let entrypoint: Vec<&str> = df.lines().filter(|l| l.starts_with("  '")).collect();
        assert!(
            entrypoint.iter().any(|l| l.contains("sudo ")),
            "sudo has left the entrypoint: add `--security-opt=no-new-privileges` to \
             `up_args` and `args`, and delete this test"
        );
        for args in [
            Docker.up_args(&sample_plan(), "n", 1, "k"),
            Docker.args(&sample_plan()),
        ] {
            assert!(
                !args.iter().any(|a| a.contains("no-new-privileges")),
                "sudo needs to gain privilege, so the flag would break the entrypoint: {args:?}"
            );
        }
    }

    /// A memory or CPU limit from the settings reaches the launch, and the
    /// stamp — so a changed limit is drift and the sandbox restarts, rather
    /// than running on under the old one with the new one in the file.
    #[test]
    fn a_resource_limit_reaches_the_launch_and_its_label() {
        let bare = Docker.up_args(&sample_plan(), "n", 1, "k");
        assert!(
            !bare.iter().any(|a| a == "--memory" || a == "--cpus"),
            "unset means the runtime's default, not a number omh invented: {bare:?}"
        );

        let mut plan = sample_plan();
        plan.limits = crate::container::Limits {
            memory: Some("4g".into()),
            cpus: Some("2".into()),
        };
        let args = Docker.up_args(&plan, "n", 1, "k");
        assert!(
            args.windows(2).any(|w| w[0] == "--memory" && w[1] == "4g"),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w[0] == "--cpus" && w[1] == "2"),
            "{args:?}"
        );

        let stamped: std::collections::BTreeMap<String, String> =
            sample_plan().labels().into_iter().collect();
        let changed = crate::container::drift(&plan.labels(), &stamped);
        assert!(
            changed.iter().any(|c| c.contains("limits")),
            "a container launched without the limit is not this plan: {changed:?}"
        );
    }

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
    /// The session's PID 1 was `sleep infinity`, which reaps nothing. Every
    /// `dtach` that exits therefore left a zombie for the life of the session
    /// — observed directly: after the wrapped program ended, the socket was
    /// gone while `pgrep -a dtach` still matched `[dtach] <defunct>`.
    ///
    /// Only the long-lived container needs this. A one-shot `docker run --rm`
    /// takes its process tree down with it.
    #[test]
    fn the_session_runs_under_an_init_that_reaps() {
        let args = Docker.up_args(&sample_plan(), "n", 1, "k");
        assert!(args.contains(&"--init".to_string()), "got: {args:?}");
        assert!(
            !Docker.args(&sample_plan()).contains(&"--init".to_string()),
            "a throwaway has nothing to reap"
        );
    }

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

    /// A container is reusable only while it still matches the plan that built
    /// it — and the comparison has nothing to read unless the launch writes it
    /// down. Asserted for every backend: the invariant is about not losing
    /// information, which is not Docker's alone to keep.
    #[test]
    fn the_session_records_what_it_was_built_from() {
        let plan = sample_plan();
        for backend in [&Docker as &dyn Runtime, &Sbx as &dyn Runtime] {
            let args = backend.up_args(&plan, "n", 1, "k");
            for (key, value) in plan.labels() {
                assert!(
                    args.iter().any(|a| *a == format!("{key}={value}")),
                    "{} does not record {key}",
                    backend.name()
                );
            }
        }
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
    /// `docker run <tag>` defaults to `--pull=missing`, so a tag with no local
    /// image is fetched from Docker Hub. omh's tags carry no registry prefix,
    /// and `omh/claude` is a name every user of a given omh version shares and
    /// anyone can precompute from this repository — so a squatted `omh/*` would
    /// be pulled and run, with the user's credential directory mounted and its
    /// `ENTRYPOINT` ahead of anything omh asked for.
    ///
    /// `probe_args` has carried this flag since it shipped, and its comment
    /// claimed the probe was "the one path that runs an image `ensure*` has not
    /// just built". Reaping falsified that: an image can now be removed between
    /// `exists` returning true and `docker run` firing — two checkouts launching
    /// seconds apart is enough, because `--rm` with no `--name` means nothing
    /// references the image until the container actually starts.
    ///
    /// A missing tag must be `No such image`, never a fetch.
    #[test]
    fn a_launch_never_fetches_an_image_it_could_not_find() {
        let mut plan = sample_plan();
        plan.image = "omh/claude:hash".into();

        for args in [
            Docker.args(&plan),
            Docker.up_args(&plan, "omh-x-s01", 2222, "ssh-ed25519 AAAA"),
        ] {
            let tag_at = args.iter().position(|a| a == "omh/claude:hash").unwrap();
            assert!(
                args[1..tag_at].iter().any(|a| a == "--pull=never"),
                "a tag reaped out from under this launch must fail, not pull: {args:?}"
            );
        }
    }
}

#[cfg(test)]
mod workdir_tests {
    /// `container_workdir`'s docstring says the path is named once, so the note
    /// store and the launch plan cannot disagree about it. It was not: the plan
    /// spelled it twice and the note store's path folding a third time.
    ///
    /// Introspects the source, the way the dispatch guard introspects the CLI
    /// definition, because the obvious test cannot fail. Asserting that the
    /// plan's workdir equals `container_workdir()` passes just as well when the
    /// plan holds the literal — both sides are the same string, so it pins
    /// nothing. Only counting the spellings can tell them apart.
    ///
    /// It counted three too few for a year. The cut it made at the first
    /// `#[cfg(test)]` stopped at `base.rs`'s test-only constant on line 118,
    /// and the graph indexer's two spellings, nine hundred lines further
    /// down, were never read. `testsrc` makes the cut now, and found them.
    #[test]
    fn only_one_place_spells_the_container_workdir() {
        let mut offenders = Vec::new();
        // Fixtures may say it; the shipped path may not. `testsrc` draws
        // that line — the split on `#[cfg(test)]` this used to make stopped
        // at the first test-only helper and read nothing after it.
        {
            for (path, production) in crate::testsrc::production() {
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
