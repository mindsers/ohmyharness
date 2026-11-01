//! Quoting, for the languages omh writes programs in: one place each.
//!
//! There were seven spellings, and they disagreed. `doctor::single_quote` and
//! `hook::shell_single_quote` were the same function twice; `shell_list`
//! deleted a quote rather than escaping it, so a skill called `don't-panic`
//! was checked as `dont-panic`; and a JavaScript string was written with
//! `{:?}`, Rust's debug syntax, which writes a NUL followed by a digit as
//! `"\01"` — a syntax error in strict mode. A value that reaches a shell
//! script, a JavaScript module or a Dockerfile goes through here.

/// A single-quoted shell word: the shell reads it as one literal, whatever is
/// in it.
///
/// Values reach generated scripts from commands a person wrote and from
/// adapters, so they are not omh's to trust: a stray quote would end the
/// literal early and the rest would be read as shell. Single quotes suspend
/// every expansion, and the one character they cannot contain is closed,
/// escaped and reopened — the standard `'\''` idiom.
pub fn sh(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Several words, each [`sh`]-quoted, space-separated: the list a `for … in`
/// loop walks. Empty is one empty word, so a loop over it still runs once and
/// finds nothing, as an absent list should.
pub fn sh_words(words: &[String]) -> String {
    if words.is_empty() {
        return sh("");
    }
    words.iter().map(|w| sh(w)).collect::<Vec<_>>().join(" ")
}

/// A Rust string as a JavaScript string literal. Through `serde_json` because
/// JSON string syntax is a subset of JavaScript's, and hand-rolling the escapes
/// for a hook body — which holds quotes, backslashes and newlines by design —
/// is how a generated program stops parsing.
///
/// U+2028 and U+2029 are escaped too. `serde_json` leaves them raw, which is
/// legal inside a string literal since ES2019 — but they end a line anywhere
/// else, so the literal is safe wherever it lands.
pub fn js(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the shell reads back is what went in, byte for byte — through a
    /// real `/bin/sh`, not a reading of the quoting rules.
    #[cfg(unix)]
    #[test]
    fn a_shell_word_reads_back_as_itself() {
        let words = [
            "plain",
            "don't-panic",
            "a b",
            "$HOME `id` $(id)",
            "back\\slash",
            "new\nline",
            "'",
            "",
        ];
        let script: String = words
            .iter()
            .map(|w| format!("printf '%s\\0' {}\n", sh(w)))
            .collect();
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        let read: Vec<&str> = std::str::from_utf8(&out.stdout)
            .unwrap()
            .split('\0')
            .collect();
        assert_eq!(&read[..words.len()], &words[..]);
    }

    /// A NUL followed by a digit is `\u0000` then the digit — `{:?}` wrote
    /// `\0` then the digit, which JavaScript reads as a legacy octal escape
    /// and strict mode refuses.
    #[test]
    fn a_js_literal_never_writes_an_octal_escape() {
        let lit = js("a\u{0}1");
        assert_eq!(lit, "\"a\\u00001\"");
        assert_eq!(serde_json::from_str::<String>(&lit).unwrap(), "a\u{0}1");
        assert!(js("x\u{2028}y").contains("\\u2028"));
    }
}
