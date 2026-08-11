//! Getting `omh` itself into the sandbox.
//!
//! MCP servers are spawned by the harness, and the harness runs *inside* the
//! container — so the base set declaring `command = "omh"` only means anything
//! if an `omh` exists in there. The code graph sidesteps this by curling a
//! published release into the image; omh has no release pipeline, and baking
//! the source into the base image would rebuild it on every edit.
//!
//! So: cross-build once into `~/.omh/bin`, bind-mount it read-only. The build
//! is a `cargo build` inside a throwaway `rust` container, cached by a digest
//! of the sources, which means it reruns exactly when the code changes and not
//! otherwise.
//!
//! Publishing static musl releases is the eventual answer. This is the one
//! that works without a release pipeline.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Where the sandbox expects to find it. Not under the agent's home, because
/// it is a program rather than state, and `/usr/local/bin` is already on PATH.
pub const GUEST_BIN: &str = "/usr/local/bin/omh";

/// What has to happen before the server can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The running binary is already a Linux binary for the right
    /// architecture: mount it and change nothing.
    HostBinary(PathBuf),
    /// A cross-build from an earlier launch, still good.
    Cached(PathBuf),
    /// Nothing usable yet.
    MustBuild(PathBuf),
}

/// Where a cross-built binary for `arch` lives.
///
/// Keyed by architecture, and that is not decoration: an x86 binary mounted
/// into an arm64 container fails with `exec format error`, which the harness
/// reports as the MCP server crashing — a cause nobody would guess from the
/// message.
pub fn cached_at(root: &Path, arch: &str) -> PathBuf {
    root.join("bin").join(format!("omh-linux-{arch}"))
}

/// Decide, without touching anything.
///
/// `exists` is injected rather than probed so the whole decision is a table
/// test — this is the part that can be wrong silently, and the shelling-out
/// part is the part that fails loudly.
pub fn plan_delivery(
    os: &str,
    arch: &str,
    current_exe: &Path,
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Delivery {
    if os == "linux" {
        return Delivery::HostBinary(current_exe.to_path_buf());
    }
    let cached = cached_at(root, arch);
    match exists(&cached) {
        true => Delivery::Cached(cached),
        false => Delivery::MustBuild(cached),
    }
}

/// The architecture a Linux container will run here, in Rust's target
/// vocabulary. Derived from the host's, because a container without an
/// explicit platform matches it.
pub fn target_arch(host_arch: &str) -> Result<&'static str> {
    match host_arch {
        "aarch64" | "arm64" => Ok("aarch64"),
        "x86_64" | "amd64" => Ok("x86_64"),
        other => bail!("no linux build target known for `{other}`"),
    }
}

/// The binary to mount, if there is one.
///
/// `None` rather than a path that does not exist: the caller mounts what this
/// returns, and docker turns a missing bind source into a directory.
///
/// Two of the ways this answers `None` are not "not built yet" but "omh cannot
/// tell": an unsupported host architecture, and a `current_exe` that will not
/// resolve. Both stay `None`, because a session without memory is still a
/// session — but both say so first. Discarding them silently leaves a session
/// with no memory server and no explanation, including under `omh doctor`,
/// which is the one command whose job is to notice.
pub fn available(paths: &crate::profile::Paths) -> Option<PathBuf> {
    let arch = match target_arch(std::env::consts::ARCH) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("omh: no memory server here — {e:#}");
            return None;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("omh: no memory server here — cannot locate the running omh: {e}");
            return None;
        }
    };
    let plan = plan_delivery(
        std::env::consts::OS,
        arch,
        &exe,
        &paths.root,
        &|p: &Path| p.exists(),
    );
    match plan {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Some(p),
        Delivery::MustBuild(_) => None,
    }
}

/// Build it if it is not there yet. Called before launch, like `image::ensure`.
pub fn ensure(program: &str, paths: &crate::profile::Paths, crate_dir: &Path) -> Result<PathBuf> {
    let arch = target_arch(std::env::consts::ARCH)?;
    let exe = std::env::current_exe().context("locating the running omh")?;
    match plan_delivery(
        std::env::consts::OS,
        arch,
        &exe,
        &paths.root,
        &|p: &Path| p.exists(),
    ) {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Ok(p),
        Delivery::MustBuild(out) => {
            // Cross-building needs omh's own sources, which a released binary
            // does not carry. Said plainly rather than failing inside docker
            // with a message about a missing Cargo.toml.
            if !crate_dir.join("Cargo.toml").exists() {
                bail!(
                    "no omh sources at {} — a released build cannot cross-build \
                     itself, so the memory server needs a published linux binary",
                    crate_dir.display()
                );
            }
            build(program, crate_dir, &out, arch)?;
            Ok(out)
        }
    }
}

/// Cross-build `omh` for Linux, inside a container, into the cache.
///
/// Progress goes straight to the terminal for the same reason `image::build`
/// does it: a multi-minute silent step reads as a hang.
pub fn build(program: &str, crate_dir: &Path, out: &Path, arch: &str) -> Result<()> {
    std::fs::create_dir_all(out.parent().context("cache has no parent")?)?;
    let target = format!("{arch}-unknown-linux-gnu");

    eprintln!("omh: cross-building the memory server for linux/{arch} (first run only)");
    let status = std::process::Command::new(program)
        .args([
            "run",
            "--rm",
            "--platform",
            &format!("linux/{}", docker_arch(arch)),
        ])
        .arg("-v")
        .arg(format!("{}:/src:ro", crate_dir.display()))
        .arg("-v")
        .arg(format!("{}:/out", out.parent().unwrap().display()))
        // A named volume for the build cache, so a second build is incremental
        // rather than a second cold compile.
        .args(["-v", "omh-selfbuild:/cargo"])
        .args(["-e", "CARGO_HOME=/cargo"])
        .args(["-w", "/build"])
        .arg("rust:1-bookworm")
        .args([
            "sh",
            "-c",
            &format!(
                "cp -r /src/src /src/Cargo.toml /src/Cargo.lock /build/ 2>/dev/null; \
                 cd /build && cargo build --release --locked \
                 && cp target/release/omh /out/{}",
                out.file_name().unwrap().to_string_lossy()
            ),
        ])
        .status()
        .with_context(|| format!("running {program} for the cross-build"))?;

    if !status.success() {
        bail!("cross-building omh for linux/{arch} failed");
    }
    let _ = target; // named for the error message above, not passed to cargo
    Ok(())
}

fn docker_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEVER: &dyn Fn(&Path) -> bool = &|_: &Path| false;
    const ALWAYS: &dyn Fn(&Path) -> bool = &|_: &Path| true;

    /// On Linux the running binary already *is* the thing the sandbox needs.
    /// Cross-building there would be a container spun up to produce a copy of
    /// the file that started it.
    #[test]
    fn on_linux_the_running_binary_is_mounted_as_it_is() {
        let plan = plan_delivery(
            "linux",
            "aarch64",
            Path::new("/usr/bin/omh"),
            Path::new("/home/x/.omh"),
            NEVER,
        );
        assert_eq!(plan, Delivery::HostBinary(PathBuf::from("/usr/bin/omh")));
    }

    /// A darwin binary cannot run in a Linux container, so the host's own
    /// executable is never a candidate there — however convenient.
    #[test]
    fn on_macos_the_hosts_own_binary_is_never_used() {
        for exists in [NEVER, ALWAYS] {
            let plan = plan_delivery(
                "macos",
                "aarch64",
                Path::new("/opt/homebrew/bin/omh"),
                Path::new("/home/x/.omh"),
                exists,
            );
            let chosen = match plan {
                Delivery::HostBinary(p) | Delivery::Cached(p) | Delivery::MustBuild(p) => p,
            };
            assert_ne!(
                chosen,
                PathBuf::from("/opt/homebrew/bin/omh"),
                "a darwin binary in a linux container is `exec format error`"
            );
        }
    }

    #[test]
    fn a_cross_build_is_reused_when_it_is_already_there() {
        let root = Path::new("/home/x/.omh");
        assert_eq!(
            plan_delivery("macos", "aarch64", Path::new("/bin/omh"), root, ALWAYS),
            Delivery::Cached(cached_at(root, "aarch64"))
        );
        assert_eq!(
            plan_delivery("macos", "aarch64", Path::new("/bin/omh"), root, NEVER),
            Delivery::MustBuild(cached_at(root, "aarch64"))
        );
    }

    /// An x86 binary mounted into an arm64 container fails with `exec format
    /// error`, which the harness reports as the MCP server crashing — a cause
    /// nobody would guess from the message. One cache path per architecture is
    /// what stops the two ever being confused.
    #[test]
    fn each_architecture_caches_to_its_own_path() {
        let root = Path::new("/home/x/.omh");
        assert_ne!(cached_at(root, "aarch64"), cached_at(root, "x86_64"));
        for arch in ["aarch64", "x86_64"] {
            assert!(
                cached_at(root, arch).to_string_lossy().contains(arch),
                "the path must name the architecture it holds"
            );
        }
    }

    /// A host omh cannot guess a target for is a loud failure, not a build
    /// that produces something unrunnable.
    #[test]
    fn an_unknown_host_architecture_is_an_error_not_a_guess() {
        assert_eq!(target_arch("aarch64").unwrap(), "aarch64");
        assert_eq!(target_arch("arm64").unwrap(), "aarch64");
        assert_eq!(target_arch("x86_64").unwrap(), "x86_64");
        let err = target_arch("riscv64").unwrap_err().to_string();
        assert!(err.contains("riscv64"), "got: {err}");
    }

    /// A released omh carries no sources, so it cannot build its own Linux
    /// counterpart. That has to be a sentence somebody can act on, not a
    /// failure inside docker about a missing Cargo.toml.
    #[test]
    fn a_build_with_no_sources_says_what_is_actually_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        // Only meaningful where a cross-build would be attempted at all.
        if std::env::consts::OS == "linux" {
            return;
        }
        let err = ensure("docker", &paths, dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("published linux binary"), "got: {err}");
        assert!(
            err.contains(&dir.path().display().to_string()),
            "got: {err}"
        );
    }

    /// The guest path has to be somewhere already on PATH, or the base set's
    /// bare `command = "omh"` resolves to nothing.
    #[test]
    fn the_guest_path_is_on_the_default_path() {
        assert!(GUEST_BIN.starts_with("/usr/local/bin/"));
        assert!(
            !GUEST_BIN.starts_with(crate::image::GUEST_HOME),
            "a program is not state, and the home is mounted over"
        );
    }
}
