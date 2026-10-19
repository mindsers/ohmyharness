//! The workflows that publish run nothing a tag can move.
//!
//! A tag is a name its owner can move. `release.yml` builds the binaries
//! `SHA256SUMS` vouches for, hands `HOMEBREW_TAP_TOKEN` to a checkout, and runs
//! `cargo publish` with `CARGO_REGISTRY_TOKEN`; `docs.yml` publishes the site.
//! Every action either one uses is pinned to a full commit, with the version in
//! a trailing comment so Dependabot can move the pin for review.

use std::path::Path;

/// The workflows that publish something.
const PUBLISHING: [&str; 2] = ["release.yml", "docs.yml"];

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

#[test]
fn every_action_a_publishing_workflow_runs_is_pinned_to_a_commit() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut seen = 0;
    let mut wrong = Vec::new();
    for name in PUBLISHING {
        let body = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("reading .github/workflows/{name}: {e}"));
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
    assert!(
        seen >= 10,
        "read only {seen} `uses:` lines across {PUBLISHING:?}"
    );
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
