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

/// The preferences file a harness's provider file seeds, and the keys it
/// seeds there: omp's `config.yml`, beside the `models.yml` its providers
/// render over, where omp reads [`omp::FILTERS`]. `None` for a harness whose
/// provider seeds nothing — Codex's rides in its System layer.
pub fn seeded(
    adapter: &crate::adapter::Adapter,
) -> Option<(crate::adapter::Declared, Vec<crate::adapter::KeyPath>)> {
    let provider = adapter.provider.as_ref()?;
    match provider.format {
        crate::adapter::ProviderFormat::CodexConfig => None,
        crate::adapter::ProviderFormat::OmpModels => {
            let beside = format!(
                "{}/config.yml",
                provider.path.as_deref()?.rsplit_once('/')?.0
            );
            let declared = adapter
                .preferences
                .as_ref()?
                .declared()
                .into_iter()
                .find(|d| d.path == beside)?;
            let keys = omp::FILTERS
                .iter()
                .map(|k| crate::adapter::key_path(k).expect("a filter is one key"))
                .collect();
            Some((declared, keys))
        }
    }
}

/// What the account's provider file puts in a session's preferences copy
/// beside its preferences, as `prefs::Account::seed` takes it: the copy's
/// account path to the document. A session's copy is the account's
/// preferences around this, and a relaunch keeps the session's own until it
/// changes. Empty for a harness or an account with nothing to seed.
pub fn rests(
    adapter: &crate::adapter::Adapter,
    account_dir: &Path,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let Some((declared, _)) = seeded(adapter) else {
        return Ok(out);
    };
    let Some(doc) = omp::read(account_dir)? else {
        return Ok(out);
    };
    let filters = omp::filters(&doc);
    if !filters.is_empty() {
        out.insert(
            declared.account_rel().to_string(),
            serde_json::to_string_pretty(&serde_json::Value::Object(filters))?,
        );
    }
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str) -> crate::adapter::Adapter {
        crate::adapter::Adapter::find(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters"),
            name,
        )
        .unwrap()
    }

    /// omp's filters reach a session as the rest of the `config.yml` its
    /// preferences are kept in — keyed as `prefs::Account::seed` looks a
    /// rest up, or the seed would pass them over without a word.
    #[test]
    fn omps_filters_are_the_rest_of_its_preferences_file() {
        let omp = adapter("omp");
        let (declared, keys) = seeded(&omp).expect("omp's provider seeds its config.yml");
        assert_eq!(declared.path, "$HOME/.omp/agent/config.yml");
        assert_eq!(
            keys,
            omp::FILTERS
                .iter()
                .map(|k| crate::adapter::key_path(k).unwrap())
                .collect::<Vec<_>>()
        );
        let d = tempfile::tempdir().unwrap();
        assert!(rests(&omp, d.path()).unwrap().is_empty());
        std::fs::write(d.path().join(omp::FILE), "providers: {}\n").unwrap();
        assert!(rests(&omp, d.path()).unwrap().is_empty());
        std::fs::write(
            d.path().join(omp::FILE),
            "providers: {}\ndisabledProviders: [ollama]\n",
        )
        .unwrap();
        let got = rests(&omp, d.path()).unwrap();
        let body = &got[declared.account_rel()];
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"disabledProviders": ["ollama"]})
        );
        assert_eq!(got.len(), 1);
        // Codex's provider rides in its System layer and seeds nothing.
        assert!(seeded(&adapter("codex")).is_none());
    }
}
