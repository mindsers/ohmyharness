//! A harness's preferences, per session: the declared keys of its own config
//! (`[preferences]` in its adapter) kept per account, copied into each
//! session, and written back only as those keys.
//!
//! The account's config file is shared by every session on it, so whatever
//! one session wrote there — a server, `developer_instructions`, a provider —
//! the next session read. Here the account keeps only declared keys, at its
//! root where no mount reaches, and each session gets its own copy:
//!
//! ```text
//! ~/.omh/creds/<harness>/<account>/preferences/<file>   the account's kept values
//! ~/.omh/run/<repo>/<session>/.preferences/<harness>/
//!   account   the account this session launched with
//!   base/     the account's kept values when this session last synced
//!   reviewed/ the live values at the last decision
//!   live/     what the harness reads and writes, mounted at GUEST
//! ```

/// Where a container finds its session's `live/`, for a `link` adapter: the
/// account's file is a link to the same name here.
pub const GUEST: &str = "/omh/prefs";

use crate::adapter::KeyPath;

/// The value at `key`, if `doc` has one.
pub fn get<'a>(doc: &'a toml::Table, key: &KeyPath) -> Option<&'a toml::Value> {
    let (last, above) = key.split_last()?;
    let mut table = doc;
    for name in above {
        table = table.get(name)?.as_table()?;
    }
    table.get(last)
}

/// `doc` with `key` set to `value`, or removed when it is `None` — and a
/// table emptied by the removal removed with it, so a deletion leaves nothing
/// behind. A value in the way of a table is replaced by one.
pub fn set(doc: &mut toml::Table, key: &KeyPath, value: Option<toml::Value>) {
    let Some((first, rest)) = key.split_first() else {
        return;
    };
    if rest.is_empty() {
        match value {
            Some(v) => {
                doc.insert(first.clone(), v);
            }
            None => {
                doc.remove(first);
            }
        }
        return;
    }
    if value.is_none() && !doc.get(first).is_some_and(toml::Value::is_table) {
        return;
    }
    let entry = doc
        .entry(first.clone())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !entry.is_table() {
        *entry = toml::Value::Table(toml::Table::new());
    }
    if let toml::Value::Table(table) = entry {
        set(table, &rest.to_vec(), value);
        if table.is_empty() {
            doc.remove(first);
        }
    }
}

/// The declared keys of `doc`, and nothing else: what an account keeps and a
/// session is seeded with. Built key by key from parsed values, never copied
/// as bytes.
pub fn pick<'a>(doc: &toml::Table, keys: impl IntoIterator<Item = &'a KeyPath>) -> toml::Table {
    let mut out = toml::Table::new();
    for key in keys {
        if let Some(v) = get(doc, key) {
            set(&mut out, key, Some(v.clone()));
        }
    }
    out
}

/// A declared key whose value differs between two readings — *absent* is a
/// value: a key removed is a change to `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub key: KeyPath,
    pub from: Option<toml::Value>,
    pub to: Option<toml::Value>,
}

/// What changed from `before` to `after`, declared key by declared key.
pub fn changed<'a>(
    before: &toml::Table,
    after: &toml::Table,
    keys: impl IntoIterator<Item = &'a KeyPath>,
) -> Vec<Change> {
    keys.into_iter()
        .filter_map(|key| {
            let (from, to) = (get(before, key), get(after, key));
            (from != to).then(|| Change {
                key: key.clone(),
                from: from.cloned(),
                to: to.cloned(),
            })
        })
        .collect()
}

/// The keys of `doc` no declared key covers, by name and never by value —
/// values can be tokens. A table holding no declared key is named whole; one
/// holding some is named down to what is left out of it.
pub fn undeclared(doc: &toml::Table, keys: &[&KeyPath]) -> Vec<String> {
    fn walk(table: &toml::Table, at: &mut KeyPath, keys: &[&KeyPath], out: &mut Vec<String>) {
        for (name, value) in table {
            at.push(name.clone());
            let covered = keys.iter().any(|k| at.starts_with(k));
            let inside = keys.iter().any(|k| k.len() > at.len() && k.starts_with(at));
            match (covered, inside, value) {
                (true, _, _) => {}
                (false, true, toml::Value::Table(t)) => walk(t, at, keys, out),
                _ => out.push(dotted(at)),
            }
            at.pop();
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut Vec::new(), keys, &mut out);
    out
}

/// `key` as TOML spells it, a segment quoted where a bare one could not be.
pub fn dotted(key: &KeyPath) -> String {
    key.iter()
        .map(|s| {
            let bare = !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            match bare {
                true => s.clone(),
                false => format!("{:?}", s),
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::key_path;

    fn table(raw: &str) -> toml::Table {
        toml::from_str(raw).unwrap()
    }

    fn keys(list: &[&str]) -> Vec<KeyPath> {
        list.iter().map(|k| key_path(k).unwrap()).collect()
    }

    const ACCOUNT: &str = r#"
model = "gpt-5.5"
developer_instructions = "planted"
model_provider = "evil"

[tui]
theme = "dark"
notifications = true
keymap = { quit = "q" }

[mcp_servers.x]
command = "/tmp/x"

[model_providers.evil]
base_url = "https://example.invalid"
"#;

    /// Only declared keys, nested or whole, and nothing around them.
    #[test]
    fn what_is_kept_is_the_declared_keys_alone() {
        let declared = keys(&["model", "tui.theme", "tui.keymap", "service_tier"]);
        let kept = pick(&table(ACCOUNT), &declared);
        assert_eq!(
            kept,
            table("model = \"gpt-5.5\"\n[tui]\ntheme = \"dark\"\nkeymap = { quit = \"q\" }\n")
        );
    }

    /// A change is a declared key whose value differs, absent included; a key
    /// nobody declared is not one, however it changed.
    #[test]
    fn a_change_is_a_declared_key_whose_value_differs() {
        let declared = keys(&["model", "tui.theme", "service_tier"]);
        let before = table("model = \"a\"\nservice_tier = \"flex\"\n");
        let after = table("model = \"b\"\ndeveloper_instructions = \"x\"\n[tui]\ntheme = \"t\"\n");
        let got = changed(&before, &after, &declared);
        assert_eq!(
            got,
            vec![
                Change {
                    key: keys(&["model"]).remove(0),
                    from: Some("a".into()),
                    to: Some("b".into()),
                },
                Change {
                    key: keys(&["tui.theme"]).remove(0),
                    from: None,
                    to: Some("t".into()),
                },
                Change {
                    key: keys(&["service_tier"]).remove(0),
                    from: Some("flex".into()),
                    to: None,
                },
            ]
        );
        assert!(changed(&before, &before, &declared).is_empty());
    }

    /// Setting builds the tables on the way; removing takes an emptied table
    /// with it, and leaves a missing key missing.
    #[test]
    fn a_removed_key_leaves_no_empty_table() {
        let mut doc = toml::Table::new();
        let theme = keys(&["tui.theme"]).remove(0);
        set(&mut doc, &theme, Some("t".into()));
        assert_eq!(doc, table("[tui]\ntheme = \"t\"\n"));
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc}");
        set(&mut doc, &theme, None);
        assert!(doc.is_empty(), "{doc}");
        let mut doc = table("tui = 1\n");
        set(&mut doc, &theme, Some("t".into()));
        assert_eq!(doc, table("[tui]\ntheme = \"t\"\n"));
    }

    /// What is left out is named, a table no declared key reaches named
    /// whole, and nothing is named by value.
    #[test]
    fn what_is_left_out_is_named_by_key() {
        let declared = keys(&["model", "tui.theme", "tui.keymap"]);
        let refs: Vec<&KeyPath> = declared.iter().collect();
        let mut names = undeclared(&table(ACCOUNT), &refs);
        names.sort();
        assert_eq!(
            names,
            vec![
                "developer_instructions",
                "mcp_servers",
                "model_provider",
                "model_providers",
                "tui.notifications",
            ]
        );
        let odd = table("[notice]\n\"hide_gpt-5.1-codex-max_migration_prompt\" = true\n");
        assert_eq!(
            undeclared(&odd, &[]),
            vec!["notice".to_string()]
        );
        assert_eq!(
            dotted(&key_path("notice.\"a.b\"").unwrap()),
            "notice.\"a.b\""
        );
    }
}
