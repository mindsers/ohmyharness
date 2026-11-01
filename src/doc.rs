//! A TOML or YAML file's text as a document, or where it stops being one.
//!
//! Preferences files and provider files are both read with these, so a
//! `config.toml` or a `config.yml` is read the same way whichever of them it
//! is.

use anyhow::Result;
use serde_json::Value;

/// `raw` as a TOML table, or where it stops being one — by line and column
/// only: the parser's own message quotes the line, and the line can be a token.
pub(crate) fn parse_toml(raw: &str) -> Result<toml::Table> {
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

/// `raw` as YAML, or where it stops being YAML omh reads — by line and column
/// only: the parser's own message quotes the line, and the line can be a token.
///
/// Read the way omp's reader does where readers differ — only `true` and
/// `false` are booleans, as Bun's YAML has it — and refused where a file could
/// say one thing two ways: a merge key (`<<`), a duplicate key, a second
/// document. The parser's budget bounds nodes, nesting and alias replay, so a
/// file cannot make a launch's parse unbounded.
pub(crate) fn parse_yaml(raw: &str) -> Result<Value> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::Error,
        with_snippet: false,
    };
    serde_saphyr::from_str_with_options::<Value>(raw, options).map_err(|e| match e.location() {
        Some(at) => anyhow::anyhow!(
            "is not YAML omh reads, at line {}, column {}",
            at.line(),
            at.column()
        ),
        None => anyhow::anyhow!("is not YAML omh reads"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_file_that_is_not_yaml_is_named_by_position_and_never_quoted() {
        let raw = "providers:\n  x:\n    apiKey: sk-secret-value\n   bad: [\n";
        let e = format!("{:#}", parse_yaml(raw).unwrap_err());
        assert!(e.contains("line") && e.contains("column"), "{e}");
        assert!(!e.contains("sk-secret"), "{e}");
    }

    #[test]
    fn a_file_cannot_make_a_parse_unbounded() {
        let mut bomb =
            String::from("a: &a [\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\"]\n");
        let mut prev = "a".to_string();
        for n in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            bomb.push_str(&format!(
                "{n}: &{n} [{}]\n",
                vec![format!("*{prev}"); 9].join(",")
            ));
            prev = n.to_string();
        }
        assert!(parse_yaml(&bomb).is_err(), "an alias bomb parsed");
        let deep = "[".repeat(100_000) + &"]".repeat(100_000);
        assert!(parse_yaml(&deep).is_err(), "nesting without a bound parsed");
        // A second document would be a second file omp reads one of.
        assert!(parse_yaml("providers: {}\n---\nproviders: {}\n").is_err());
        assert!(
            parse_yaml("providers:\n  x: {}\n  x: {}\n").is_err(),
            "a duplicate key"
        );
        assert!(
            parse_yaml("a: &a {x: 1}\nproviders:\n  <<: *a\n").is_err(),
            "a merge key"
        );
    }

    /// Only `true` and `false` are booleans, as omp's reader has it — Bun's
    /// YAML reads `yes` and `on` as the strings they are (measured, 1.3.14).
    #[test]
    fn a_word_is_a_string_as_omps_reader_reads_it() {
        assert_eq!(
            parse_yaml("a: yes\nb: on\nc: true\n").unwrap(),
            json!({"a": "yes", "b": "on", "c": true})
        );
    }
}
