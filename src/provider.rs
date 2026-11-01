//! An account's own model provider, kept where no session can write it.
//!
//! A harness that reads its providers from a file in the account would let
//! one session point every later session's traffic, and the credentials it
//! carries, at an endpoint of its choosing. So each keeps its provider in a
//! file of its own at the account root, which no mount reaches, and omh
//! renders it where the session reads and cannot write: [`codex`] into
//! Codex's System layer, [`omp`] over its `models.yml`.

use anyhow::{Context, Result};
use std::path::Path;

pub mod codex;
pub mod omp;

/// The provider file a harness of `format` keeps at the account root.
pub fn file(format: crate::adapter::ProviderFormat) -> &'static str {
    match format {
        crate::adapter::ProviderFormat::CodexConfig => codex::FILE,
        crate::adapter::ProviderFormat::OmpModels => omp::FILE,
    }
}

/// `name` at the account root, parsed and held to its format: `None` when
/// the account has none. Each format's `read`.
fn read_checked<T>(
    account_dir: &Path,
    name: &str,
    parse: fn(&str) -> Result<T>,
    validate: fn(&T) -> Result<()>,
) -> Result<Option<T>> {
    let path = account_dir.join(name);
    let Some(raw) = crate::fsx::read_or_absent(&path)? else {
        return Ok(None);
    };
    let doc = parse(&raw).with_context(|| format!("{}", path.display()))?;
    validate(&doc).with_context(|| format!("{}", path.display()))?;
    Ok(Some(doc))
}

/// `body` as the account's provider file `name`, readable by the user alone.
/// Each format's `write`, once it has held the document to its format.
fn write_private(account_dir: &Path, name: &str, body: &[u8]) -> Result<()> {
    std::fs::create_dir_all(account_dir)?;
    let path = account_dir.join(name);
    crate::held::Dir::open(account_dir)?
        .replace_private(std::ffi::OsStr::new(name), body)
        .with_context(|| format!("writing {}", path.display()))
}

/// Whether `url` names this machine — which, in a sandbox, is the sandbox.
pub(crate) fn loopback(url: &str) -> bool {
    let lower = host(url).to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.starts_with("127.")
        || ["[::1]", "[::]", "0.0.0.0"].contains(&lower.as_str())
}

/// The host `url` names: after its scheme and userinfo, before its port. For
/// the loopback refusal, which saves a provider that could not answer, not a
/// boundary: a form it misses imports a provider that fails at its first turn.
pub(crate) fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    match host.strip_prefix('[') {
        Some(v6) => &host[..v6.find(']').map_or(host.len(), |end| end + 2)],
        None => host
            .split(':')
            .next()
            .unwrap_or_default()
            .trim_end_matches('.'),
    }
}

/// `url` without what can hold a credential: its userinfo and its query each
/// become `<redacted>`, and a fragment goes. Erring wide, since parsers
/// disagree on where an authority ends: everything before the last `@` is
/// redacted, and a value with no scheme is redacted whole.
pub(crate) fn redacted_url(url: &str) -> String {
    let url = url.split('#').next().unwrap_or_default();
    let (url, query) = match url.split_once('?') {
        Some((url, _)) => (url, "?<redacted>"),
        None => (url, ""),
    };
    let Some((scheme, rest)) = url.split_once("://") else {
        return "<redacted>".into();
    };
    match rest.rsplit_once('@') {
        Some((_, after)) => format!("{scheme}://<redacted>@{after}{query}"),
        None => format!("{scheme}://{rest}{query}"),
    }
}
