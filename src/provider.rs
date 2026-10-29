//! An account's own model provider, kept where no session can write it.
//!
//! Codex keeps a custom provider — `model_provider` and its
//! `[model_providers.<id>]` table — in `~/.codex/config.toml`, the account's
//! own file, which every session on the account can rewrite: a session that
//! pointed `base_url` elsewhere would send every later session's traffic, and
//! the credentials it carries, to an endpoint of its choosing (#197 §6). So
//! the provider lives in `provider.toml` at the account root, which no mount
//! reaches, and omh renders it into the System layer, `/etc/codex/config.toml`,
//! which the session cannot write.
//!
//! **An allowlist, in Codex's own schema.** The System layer is the one
//! `allow_managed_hooks_only` treats as managed, so the file holds provider
//! keys and nothing else: a `[hooks]` table, a `notify` or a
//! `developer_instructions` there would be a managed hook or a standing
//! prompt. Within a provider, only fields that are data pass: what reads the
//! host's environment (`env_key`, `env_http_headers`), runs a command
//! (`auth`), or reaches a profile or a login flow (`aws`, `gateway_oauth`)
//! cannot be satisfied in a sandbox — `omh auth codex --provider` resolves the
//! environment ones once into the file, and refuses the rest by name.
//!
//! The fields are `ModelProviderInfo`'s at the pinned tag
//! (`codex-rs/model-provider-info/src/lib.rs`, `rust-v0.162.1`).

use anyhow::{Context, Result};
use std::path::Path;

pub mod omp;

/// At the account root, beside its harness directories and in none of them.
pub const FILE: &str = "provider.toml";

/// The provider file a harness of `format` keeps at the account root.
pub fn file(format: crate::adapter::ProviderFormat) -> &'static str {
    match format {
        crate::adapter::ProviderFormat::CodexConfig => FILE,
        crate::adapter::ProviderFormat::OmpModels => omp::FILE,
    }
}

/// `model_providers.<id>` fields that are data: kept as written.
const DATA: &[&str] = &[
    "name",
    "base_url",
    "model_catalog_url",
    "experimental_bearer_token",
    "wire_api",
    "capabilities",
    "query_params",
    "http_headers",
    "request_max_retries",
    "stream_max_retries",
    "stream_idle_timeout_ms",
    "websocket_connect_timeout_ms",
    "requires_openai_auth",
    "supports_websockets",
    "supports_standalone_web_search",
];

/// Fields whose values are secrets: never printed, and redacted on eject.
pub const SECRET: &[&str] = &["experimental_bearer_token", "http_headers", "query_params"];

/// Fields holding a URL, which can carry a secret in its userinfo or query:
/// those parts are redacted, the rest kept to say where the provider is.
const URLS: &[&str] = &["base_url", "model_catalog_url"];

/// Check `doc` is a provider file and nothing else.
pub fn validate(doc: &toml::Table) -> Result<()> {
    for key in doc.keys() {
        anyhow::ensure!(
            key == "model_provider" || key == "model_providers",
            "`{key}` is not a provider key — a provider file holds `model_provider` and \
             `[model_providers.<id>]` only, since omh renders it into the layer Codex \
             treats as managed"
        );
    }
    let providers = match doc.get("model_providers") {
        None => toml::Table::new(),
        Some(toml::Value::Table(t)) => t.clone(),
        Some(_) => anyhow::bail!("`model_providers` is not a table"),
    };
    for (id, provider) in &providers {
        let fields = provider
            .as_table()
            .with_context(|| format!("`model_providers.{id}` is not a table"))?;
        for field in fields.keys() {
            anyhow::ensure!(
                DATA.contains(&field.as_str()),
                "`model_providers.{id}.{field}` {} — `omh auth codex --provider <file>` \
                 resolves an environment variable into the file, and refuses a command",
                match field.as_str() {
                    "env_key" | "env_http_headers" | "env_key_instructions" =>
                        "reads the environment, which a sandbox does not have",
                    "auth" | "gateway_oauth" => "runs a command or a login flow",
                    "aws" => "reads an AWS profile, which a sandbox does not have",
                    _ => "is not a provider field omh passes",
                }
            );
        }
    }
    if let Some(selected) = doc.get("model_provider") {
        let selected = selected
            .as_str()
            .context("`model_provider` is not a string")?;
        anyhow::ensure!(
            providers.contains_key(selected),
            "`model_provider = \"{selected}\"` names no `[model_providers.{selected}]` in \
             this file"
        );
    }
    Ok(())
}

/// The account's provider file, checked — `None` when it has none.
pub fn read(account_dir: &Path) -> Result<Option<toml::Table>> {
    let path = account_dir.join(FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let doc = parse(&raw).with_context(|| format!("{}", path.display()))?;
    validate(&doc).with_context(|| format!("{}", path.display()))?;
    Ok(Some(doc))
}

/// `raw` as a TOML table, or where it stops being one — by line and column
/// only: the parser's own message quotes the line, and the line can be a token.
pub(crate) fn parse(raw: &str) -> Result<toml::Table> {
    toml::from_str(raw).map_err(|e: toml::de::Error| {
        let at = e.span().map_or(0, |s| s.start).min(raw.len());
        let before = &raw.as_bytes()[..at];
        let line = before.iter().filter(|b| **b == b'\n').count() + 1;
        let column = at
            - before
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1)
            + 1;
        anyhow::anyhow!("is not TOML at line {line}, column {column}")
    })
}

/// The provider tables of a Codex `config.toml` — the host's, or a file the
/// user names — resolved for a sandbox: `env_key` and `env_http_headers` read
/// once from `env` into the file, everything that cannot be satisfied there
/// refused by name. `None` when it defines no provider.
pub fn from_codex_config(
    raw: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<toml::Table>> {
    let config = parse(raw)?;
    let mut doc = toml::Table::new();
    if let Some(selected) = config.get("model_provider") {
        doc.insert("model_provider".into(), selected.clone());
    }
    let Some(providers) = config.get("model_providers").and_then(|p| p.as_table()) else {
        if doc.is_empty() {
            return Ok(None);
        }
        validate(&doc)?;
        return Ok(Some(doc));
    };
    let var = |id: &str, name: &str| {
        env(name).with_context(|| {
            format!(
                "`model_providers.{id}` reads `{name}` from the environment, and it is not \
                 set here — set it and import again, or write the value into a provider \
                 file and name it"
            )
        })
    };
    let mut out = toml::Table::new();
    for (id, provider) in providers {
        let mut p = provider
            .as_table()
            .with_context(|| format!("`model_providers.{id}` is not a table"))?
            .clone();
        if let Some(name) = p.remove("env_key") {
            let name = name.as_str().context("`env_key` is not a string")?;
            p.insert("experimental_bearer_token".into(), var(id, name)?.into());
        }
        p.remove("env_key_instructions");
        if let Some(headers) = p.remove("env_http_headers") {
            let headers = headers
                .as_table()
                .context("`env_http_headers` is not a table")?;
            let mut resolved = p
                .get("http_headers")
                .and_then(|h| h.as_table())
                .cloned()
                .unwrap_or_default();
            for (header, name) in headers {
                let name = name
                    .as_str()
                    .context("an `env_http_headers` value is not a string")?;
                resolved.insert(header.clone(), var(id, name)?.into());
            }
            p.insert("http_headers".into(), resolved.into());
        }
        for refused in ["auth", "gateway_oauth", "aws"] {
            anyhow::ensure!(
                !p.contains_key(refused),
                "`model_providers.{id}.{refused}` runs a command, a login flow or reads a \
                 profile on the host, which a sandbox cannot — nothing was imported. Write \
                 the token it produces into a provider file and name it"
            );
        }
        if let Some(url) = p.get("base_url").and_then(|u| u.as_str()) {
            anyhow::ensure!(
                !loopback(url),
                "`model_providers.{id}.base_url = \"{}\"` is `{}`, which in a sandbox is \
                 the sandbox itself — nothing was imported. Name an address the sandbox can \
                 reach (on Docker Desktop, `host.docker.internal`)",
                redacted_url(url),
                host(url)
            );
        }
        out.insert(id.clone(), p.into());
    }
    doc.insert("model_providers".into(), out.into());
    validate(&doc)?;
    Ok(Some(doc))
}

/// Write `doc` as the account's provider file, readable by the user alone.
pub fn write(account_dir: &Path, doc: &toml::Table) -> Result<()> {
    validate(doc)?;
    std::fs::create_dir_all(account_dir)?;
    let path = account_dir.join(FILE);
    crate::held::Dir::open(account_dir)?
        .replace_private(std::ffi::OsStr::new(FILE), toml::to_string(doc)?.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// `doc` with every secret value replaced, for anything shown or ejected.
pub fn redacted(doc: &toml::Table) -> toml::Table {
    let mut doc = doc.clone();
    if let Some(providers) = doc
        .get_mut("model_providers")
        .and_then(|p| p.as_table_mut())
    {
        for (_, provider) in providers.iter_mut() {
            let Some(provider) = provider.as_table_mut() else {
                continue;
            };
            for (field, value) in provider.iter_mut() {
                if URLS.contains(&field.as_str()) {
                    if let toml::Value::String(url) = value {
                        *url = redacted_url(url);
                    }
                    continue;
                }
                if !SECRET.contains(&field.as_str()) {
                    continue;
                }
                match value {
                    toml::Value::Table(t) => {
                        for (_, v) in t.iter_mut() {
                            *v = "<redacted>".into();
                        }
                    }
                    v => *v = "<redacted>".into(),
                }
            }
        }
    }
    doc
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

    fn table(raw: &str) -> toml::Table {
        toml::from_str(raw).unwrap()
    }

    const AZURE: &str = r#"
model_provider = "azure"

[model_providers.azure]
name = "Azure"
base_url = "https://example.openai.azure.com/openai"
wire_api = "responses"
experimental_bearer_token = "sk-secret"
query_params = { api-version = "2025-04-01-preview" }
"#;

    #[test]
    fn a_provider_file_of_provider_keys_is_accepted() {
        validate(&table(AZURE)).unwrap();
    }

    /// Only provider keys reach the System layer, which Codex treats as
    /// managed: each other key is refused by name.
    #[test]
    fn anything_but_a_provider_is_refused_by_name() {
        for (extra, named) in [
            ("[hooks]\nStop = []\n", "hooks"),
            ("notify = [\"/bin/sh\"]\n", "notify"),
            ("developer_instructions = \"x\"\n", "developer_instructions"),
            ("model = \"o3\"\n", "model"),
        ] {
            let doc = table(&format!("{extra}\n{AZURE}"));
            let err = validate(&doc).unwrap_err().to_string();
            assert!(err.contains(named), "{named}: {err}");
        }
    }

    /// Within a provider, what the sandbox cannot satisfy — the host's
    /// environment, a command, a profile, a login flow — is refused by name,
    /// and so is a field Codex does not have.
    #[test]
    fn a_provider_field_the_sandbox_cannot_satisfy_is_refused_by_name() {
        for field in [
            "env_key = \"AZURE_KEY\"",
            "env_http_headers = { X = \"Y\" }",
            "auth = { command = \"/bin/token\" }",
            "aws = { profile = \"p\" }",
            "gateway_oauth = {}",
            "nonsense = 1",
        ] {
            let doc = table(&format!("{AZURE}{field}\n"));
            let name = field.split(' ').next().unwrap();
            let err = validate(&doc).unwrap_err().to_string();
            assert!(err.contains(name), "{name}: {err}");
        }
    }

    #[test]
    fn a_selected_provider_must_be_defined_here() {
        let doc = table("model_provider = \"elsewhere\"\n");
        let err = validate(&doc).unwrap_err().to_string();
        assert!(err.contains("elsewhere"), "{err}");
    }

    /// Absent is none; present is checked; one that does not parse names the
    /// file — Codex would refuse every start with it.
    #[test]
    fn the_account_file_is_read_checked_and_named_when_broken() {
        let d = tempfile::tempdir().unwrap();
        assert!(read(d.path()).unwrap().is_none());
        std::fs::write(d.path().join(FILE), AZURE).unwrap();
        assert_eq!(read(d.path()).unwrap(), Some(table(AZURE)));
        std::fs::write(d.path().join(FILE), "model_provider = ").unwrap();
        let err = format!("{:#}", read(d.path()).unwrap_err());
        assert!(err.contains(FILE), "{err}");
        // By place, never by quoting the line: the line can be a token.
        let leaks = "[model_providers.p]\nname = \"P\"\nexperimental_bearer_token = \"sk-leak\n";
        std::fs::write(d.path().join(FILE), leaks).unwrap();
        let err = format!("{:#}", read(d.path()).unwrap_err());
        assert!(err.contains("line 3") && !err.contains("sk-leak"), "{err}");
        let env = |_: &str| None;
        let err = format!("{:#}", from_codex_config(leaks, &env).unwrap_err());
        assert!(err.contains("line 3") && !err.contains("sk-leak"), "{err}");
        std::fs::write(d.path().join(FILE), "notify = [\"x\"]\n").unwrap();
        let err = format!("{:#}", read(d.path()).unwrap_err());
        assert!(err.contains(FILE) && err.contains("notify"), "{err}");
    }

    #[test]
    fn a_provider_file_is_written_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &table(AZURE)).unwrap();
        let mode = std::fs::metadata(d.path().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(read(d.path()).unwrap(), Some(table(AZURE)));
    }

    /// Importing from a Codex config resolves the environment once into the
    /// file, and refuses what a sandbox cannot satisfy rather than importing
    /// it silently.
    #[test]
    fn a_codex_config_is_resolved_for_a_sandbox_or_refused_by_name() {
        let env = |k: &str| match k {
            "AZURE_KEY" => Some("sk-from-env".to_string()),
            "TEAM" => Some("blue".to_string()),
            _ => None,
        };
        let config = r#"
model = "o3"
developer_instructions = "not a provider"
model_provider = "azure"

[model_providers.azure]
name = "Azure"
base_url = "https://example.openai.azure.com/openai"
env_key = "AZURE_KEY"
env_key_instructions = "get one at …"
env_http_headers = { X-Team = "TEAM" }

[mcp_servers.x]
command = "x"
"#;
        let doc = from_codex_config(config, &env).unwrap().unwrap();
        validate(&doc).unwrap();
        let azure = &doc["model_providers"]["azure"];
        assert_eq!(
            azure["experimental_bearer_token"].as_str(),
            Some("sk-from-env")
        );
        assert_eq!(azure["http_headers"]["X-Team"].as_str(), Some("blue"));
        assert!(
            doc.get("model").is_none() && doc.get("mcp_servers").is_none(),
            "{doc}"
        );
        assert!(doc.get("developer_instructions").is_none(), "{doc}");

        assert!(from_codex_config("model = \"o3\"\n", &env)
            .unwrap()
            .is_none());

        for (provider, named) in [
            ("env_key = \"UNSET\"", "UNSET"),
            ("auth = { command = \"/bin/token\" }", "auth"),
            ("aws = { profile = \"p\" }", "aws"),
            ("base_url = \"http://127.0.0.1:11434/v1\"", "127.0.0.1"),
            ("base_url = \"http://localhost:8080\"", "localhost"),
            // Behind a userinfo, in capitals, anywhere in 127/8.
            ("base_url = \"http://u:pa55@127.0.0.1/v1\"", "127.0.0.1"),
            ("base_url = \"http://LOCALHOST:8080\"", "LOCALHOST"),
            ("base_url = \"http://127.0.0.2:8080\"", "127.0.0.2"),
            ("base_url = \"http://[::1]:8080\"", "[::1]"),
            ("base_url = \"http://localhost.:8080\"", "localhost"),
        ] {
            let config =
                format!("model_provider = \"p\"\n[model_providers.p]\nname = \"P\"\n{provider}\n");
            let err = from_codex_config(&config, &env).unwrap_err().to_string();
            assert!(err.contains(named), "{named}: {err}");
            assert!(!err.contains("pa55"), "the refusal printed a secret: {err}");
        }
        // A name that only starts like one is a host.
        let config = "model_provider = \"p\"\n[model_providers.p]\nname = \"P\"\n\
                      base_url = \"https://localhost.example.com/v1\"\n";
        from_codex_config(config, &env).unwrap();
    }

    /// Nothing shown or ejected carries a secret.
    #[test]
    fn a_redacted_provider_holds_no_secret() {
        let shown = toml::to_string(&redacted(&table(AZURE))).unwrap();
        assert!(!shown.contains("sk-secret"), "{shown}");
        assert!(shown.contains("example.openai.azure.com"), "{shown}");

        // A URL can carry a credential too: in its userinfo, or its query.
        let doc = table(
            "[model_providers.p]\nname = \"P\"\n\
             base_url = \"https://u:pa55@example.com/v1?api-key=kq#f\"\n\
             model_catalog_url = \"https://example.com/models?token=kc\"\n",
        );
        let shown = toml::to_string(&redacted(&doc)).unwrap();
        for secret in ["pa55", "u:", "kq", "kc"] {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        assert!(
            shown.contains("https://<redacted>@example.com/v1?<redacted>"),
            "{shown}"
        );
        assert!(
            shown.contains("https://example.com/models?<redacted>"),
            "{shown}"
        );

        // Read as a stricter parser might: a backslash ends an authority for
        // one parser and not another, so everything before the last `@` goes;
        // a value with no scheme goes whole.
        for (url, kept) in [
            (
                r"https://u:pb\@example.com/v1",
                "https://<redacted>@example.com/v1",
            ),
            ("u:pc@example.com/v1", "<redacted>"),
        ] {
            let doc = table(&format!(
                "[model_providers.p]\nname = \"P\"\nbase_url = '{url}'\n"
            ));
            let shown = redacted(&doc)["model_providers"]["p"]["base_url"].clone();
            assert_eq!(shown.as_str(), Some(kept), "{url}");
        }
    }
}
