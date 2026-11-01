//! What the workflows promise about themselves, held where a reviewer cannot
//! forget to check it.
//!
//! A tag is a name its owner can move. `release.yml` builds the binaries
//! `SHA256SUMS` vouches for, hands `HOMEBREW_TAP_TOKEN` to a checkout, and runs
//! `cargo publish` with `CARGO_REGISTRY_TOKEN`; `docs.yml` publishes the site;
//! `ci.yml` is the required checks, so what it runs decides what merges. Every
//! action any of them uses is pinned to a full commit, with the version in a
//! trailing comment so Dependabot can move the pin for review.

use serde_json::Value;
use std::path::Path;

/// Every workflow, by file name. Read from the directory, so a new one is held
/// to the same rules without anyone listing it here.
fn workflows() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("listing .github/workflows").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            (name, body)
        })
        .filter(|(name, _)| name.ends_with(".yml") || name.ends_with(".yaml"))
        .collect();
    out.sort();
    // A directory read that found less than it should would pass every rule
    // below with nothing checked.
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    for expected in ["ci.yml", "docs.yml", "nightly.yml", "release.yml"] {
        assert!(names.contains(&expected), "{expected} not read: {names:?}");
    }
    out
}

fn parsed(name: &str, body: &str) -> Value {
    serde_saphyr::from_str(body).unwrap_or_else(|e| panic!("{name} is not YAML this reads: {e}"))
}

fn workflow(name: &str) -> Value {
    let (_, body) = workflows()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no workflow {name}"));
    parsed(name, &body)
}

fn jobs(doc: &Value) -> &serde_json::Map<String, Value> {
    doc["jobs"]
        .as_object()
        .expect("a workflow with no jobs table")
}

fn steps(job: &Value) -> Vec<&Value> {
    job["steps"]
        .as_array()
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

fn run_of(step: &Value) -> &str {
    step["run"].as_str().unwrap_or("")
}

/// `owner/repo@<40 hex> # vX.Y.Z`, or the reason it is not.
fn pinned(uses: &str) -> Result<(), String> {
    let (target, comment) = match uses.split_once('#') {
        Some((target, comment)) => (target.trim(), comment.trim()),
        None => (uses.trim(), ""),
    };
    let Some((_, reference)) = target.split_once('@') else {
        return Err("names no version at all".into());
    };
    if reference.len() != 40 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("`@{reference}` is not a full commit"));
    }
    if !comment.starts_with('v') {
        return Err("has no `# vX.Y.Z` comment for Dependabot to move".into());
    }
    Ok(())
}

/// Read line by line rather than from the parsed document: the trailing
/// `# vX.Y.Z` that Dependabot moves is a comment, which no parser keeps.
#[test]
fn every_action_a_workflow_runs_is_pinned_to_a_commit() {
    let mut seen = 0;
    let mut wrong = Vec::new();
    for (name, body) in workflows() {
        for (i, line) in body.lines().enumerate() {
            let Some(uses) = line.trim_start().strip_prefix("uses:") else {
                continue;
            };
            seen += 1;
            if let Err(why) = pinned(uses) {
                wrong.push(format!("{name}:{}: `{}` {why}", i + 1, uses.trim()));
            }
        }
    }
    // A workflow that stopped naming `uses:` the way this reads would pass
    // with nothing checked.
    assert!(seen >= 30, "read only {seen} `uses:` lines");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_pin_reader_refuses_what_it_should() {
    assert!(pinned(" actions/checkout@v7").is_err());
    assert!(pinned(" actions/checkout@main").is_err());
    assert!(pinned(" actions/checkout@3d3c42e5").is_err());
    assert!(pinned(" actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1").is_err());
    assert!(pinned(" actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1").is_ok());
}

/// `ci.yml`'s job names are the required checks branch protection waits on.
/// One that carried the toolchain it builds on was renamed by the bump that
/// moved the floor — `build (msrv 1.85)` became `build (msrv 1.88)` — and the
/// rule went on waiting for a name nothing produced, so every merge after it
/// went through the admin bypass with no required check enforced at all.
#[test]
fn no_required_check_is_named_after_a_version() {
    let doc = workflow("ci.yml");
    let mut wrong = Vec::new();
    for (id, job) in jobs(&doc) {
        let name = job["name"].as_str().unwrap_or(id);
        let versioned = name
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .any(|word| word.contains('.') && word.chars().any(|c| c.is_ascii_digit()));
        if versioned {
            wrong.push(format!("{id}: `{name}`"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a required check named after a version is renamed by the next bump:\n{}",
        wrong.join("\n")
    );
}

/// Unset, a job runs for 360 minutes. A Docker test that hangs holds a
/// required check — or a release — for six hours before anyone is told.
#[test]
fn every_job_has_a_timeout() {
    let mut wrong = Vec::new();
    let mut seen = 0;
    for (name, body) in workflows() {
        let doc = parsed(&name, &body);
        for (id, job) in jobs(&doc) {
            seen += 1;
            if job.get("timeout-minutes").and_then(Value::as_u64).is_none() {
                wrong.push(format!("{name}: {id}"));
            }
        }
    }
    assert!(seen >= 15, "read only {seen} jobs");
    assert!(
        wrong.is_empty(),
        "jobs with no timeout:\n{}",
        wrong.join("\n")
    );
}

/// A cache is saved under the ref that made it, and a tag's run cannot read
/// another tag's. In `release.yml`, which runs on tags, a cache action can only
/// ever miss — third-party code in the release path that saves nothing.
#[test]
fn the_release_restores_no_cache() {
    let doc = workflow("release.yml");
    let cached: Vec<String> = jobs(&doc)
        .iter()
        .flat_map(|(id, job)| {
            steps(job)
                .into_iter()
                .filter(|s| s["uses"].as_str().is_some_and(|u| u.contains("rust-cache")))
                .map(move |_| id.clone())
        })
        .collect();
    assert!(
        cached.is_empty(),
        "release jobs restoring a cache: {cached:?}"
    );
}

/// `cargo publish` builds the crate before uploading it, which runs every build
/// script and proc-macro in the tree — beside the registry token, which cannot
/// take back a version once it is up. The build happens in `cargo package`,
/// with no token; the token's step only uploads what that step built.
#[test]
fn the_registry_token_builds_nothing() {
    let doc = workflow("release.yml");
    let mut holders = 0;
    for (id, job) in jobs(&doc) {
        let steps = steps(job);
        for (i, step) in steps.iter().enumerate() {
            let holds = step["env"]
                .as_object()
                .is_some_and(|env| env.contains_key("CARGO_REGISTRY_TOKEN"));
            if !holds {
                continue;
            }
            holders += 1;
            let run = run_of(step);
            assert!(
                run.contains("cargo publish") && run.contains("--no-verify"),
                "{id}: the step holding the token runs `{run}`, which builds"
            );
            let packaged = steps[..i].iter().any(|s| {
                run_of(s).contains("cargo package")
                    && !s["env"]
                        .as_object()
                        .is_some_and(|env| env.contains_key("CARGO_REGISTRY_TOKEN"))
            });
            assert!(
                packaged,
                "{id}: nothing before the upload verifies the package without the token"
            );
        }
    }
    assert_eq!(holders, 1, "expected one step holding CARGO_REGISTRY_TOKEN");
}

/// What GitHub publishes is attested: a build provenance statement for every
/// tarball, signed by the workflow that built it, which `gh attestation
/// verify` checks against this repository.
#[test]
fn the_release_attests_every_tarball() {
    let doc = workflow("release.yml");
    let release = &jobs(&doc)["release"];
    let attests = steps(release).into_iter().any(|s| {
        s["uses"]
            .as_str()
            .is_some_and(|u| u.starts_with("actions/attest-build-provenance@"))
            && s["with"]["subject-path"]
                .as_str()
                .is_some_and(|p| p.contains("*.tar.gz"))
    });
    assert!(attests, "the release job attests no tarball");
    let permissions = &release["permissions"];
    for (scope, level) in [("id-token", "write"), ("attestations", "write")] {
        assert_eq!(
            permissions[scope].as_str(),
            Some(level),
            "the release job needs `{scope}: {level}` to attest"
        );
    }
}

/// A binary statically links every crate in the lock, and their licences ask
/// for their notices to travel with it. Every tarball carries them.
#[test]
fn every_tarball_carries_the_third_party_notices() {
    let doc = workflow("release.yml");
    let build = &jobs(&doc)["build"];
    let pack = steps(build)
        .into_iter()
        .find(|s| s["name"].as_str() == Some("pack the tarball"))
        .expect("no `pack the tarball` step");
    assert!(
        run_of(pack).contains("THIRD-PARTY-NOTICES"),
        "the tarball is packed without the third-party notices:\n{}",
        run_of(pack)
    );
}

/// Every push checks the dependencies against the advisory databases: the
/// crates through `cargo deny` and `deny.toml`, the documentation site's
/// packages through `pnpm audit`. Dependabot alone sees neither whole: its
/// graph reads only the first document of the pnpm lockfile.
#[test]
fn every_push_checks_the_dependencies_for_advisories() {
    let doc = workflow("ci.yml");
    let all: Vec<&Value> = jobs(&doc).values().flat_map(steps).collect();
    let deny = all.iter().any(|s| {
        s["uses"]
            .as_str()
            .is_some_and(|u| u.starts_with("EmbarkStudios/cargo-deny-action@"))
    });
    assert!(deny, "no job runs cargo deny");
    let audit = all.iter().any(|s| run_of(s).contains("pnpm audit"));
    assert!(audit, "no job runs pnpm audit");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("deny.toml");
    assert!(config.is_file(), "cargo deny has no deny.toml to read");
}

/// `a_private_root_reads_as_private_and_a_public_one_does_not` is `#[ignore]`d
/// for the network it needs, and its real arm is macOS-only: Linux merges the
/// shipped roots with installed ones, so off macOS it only checks the refusal.
/// The Linux job runs the ignored set and the macOS job did not, so the arm
/// that reads a real handshake ran in no job at all.
#[test]
fn the_macos_only_tls_arm_runs_on_macos() {
    let doc = workflow("ci.yml");
    let macos = &jobs(&doc)["test-macos"];
    let runs = steps(macos).into_iter().any(|s| {
        let run = run_of(s);
        run.contains("--include-ignored")
            && run.contains(
                "--exact doctor::tests::a_private_root_reads_as_private_and_a_public_one_does_not",
            )
    });
    assert!(
        runs,
        "the macOS job never runs the macOS arm of the TLS check"
    );
}
