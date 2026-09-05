//! Which text under `src/` is omh, and which is its tests.
//!
//! Seven guards in this crate read its own source — for a `println!` behind
//! the output layer, a second `"/work"`, a setting nobody classified, a line
//! omh prints but does not accept. Each cut the tests off with a rule of its
//! own, and the two rules in use were wrong in opposite directions.
//! `split("#[cfg(test)]")` stopped at the first test-only *helper*: `base.rs`
//! keeps one at line 118, so nine hundred production lines after it were never
//! read by four of the scans. `find("\nmod tests {")` read every such helper as if
//! omh shipped it. One rule lives here, and it is tested against a tree built
//! to hold the shapes that fooled the other two.
//!
//! The rule: a file is a **test module** when the `mod` line that declares it
//! sits under `#[cfg(test)]`, and none of it is production. Otherwise the
//! production text is the file with every item under a column-zero
//! `#[cfg(test)]` blanked — the inline `mod tests { … }` and any helper
//! beside it. Blanked, not removed, so a line number a scan reports is the
//! line in the file.
//!
//! Test-only and reached only from tests. omh never reads its own source at
//! runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a `.rs` file under `src/` is reached from the crate root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Declared by a `mod` line the compiler always follows. What omh ships,
    /// plus whatever test text the file carries inline.
    Production,
    /// Declared under `#[cfg(test)]`. A file of tests and nothing else.
    TestModule,
    /// Declared by nothing. The compiler never opens it, so a scan that did
    /// would count words omh does not compile.
    Undeclared,
}

pub(crate) struct Source {
    pub path: PathBuf,
    pub body: String,
    pub role: Role,
}

/// Every `.rs` file under `root`, classified. `root` is the `src/` directory.
pub(crate) fn classify(root: &Path) -> Vec<Source> {
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.insert(path.clone(), std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let declared = declarations(&files);
    files
        .into_iter()
        .map(|(path, body)| {
            let role = if path == root.join("main.rs") || path == root.join("lib.rs") {
                Role::Production
            } else {
                match declared.get(&path) {
                    Some(true) => Role::TestModule,
                    Some(false) => Role::Production,
                    None => Role::Undeclared,
                }
            };
            Source { path, body, role }
        })
        .collect()
}

/// This crate's production files, each with its test text blanked.
pub(crate) fn production() -> Vec<(PathBuf, String)> {
    classify(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
        .into_iter()
        .filter(|s| s.role == Role::Production)
        .map(|s| (s.path, production_of(&s.body)))
        .collect()
}

/// Every `mod name;` in the tree, resolved to the file it opens, with whether
/// the declaration sits under `#[cfg(test)]`.
fn declarations(files: &BTreeMap<PathBuf, String>) -> BTreeMap<PathBuf, bool> {
    let mut out = BTreeMap::new();
    for (file, body) in files {
        let mut cfg_test = false;
        let mut path_attr: Option<String> = None;
        for raw in body.lines() {
            let line = raw.trim();
            if line == "#[cfg(test)]" {
                cfg_test = true;
                continue;
            }
            if let Some(quoted) = line
                .strip_prefix("#[path = \"")
                .and_then(|r| r.strip_suffix("\"]"))
            {
                path_attr = Some(quoted.to_string());
                continue;
            }
            if line.starts_with("//") || line.is_empty() || line.starts_with("#[") {
                continue;
            }
            let item = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub "))
                .unwrap_or(line);
            if let Some(name) = item.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) {
                let target = resolve(file, name, path_attr.as_deref());
                if let Some(target) = target.into_iter().find(|t| files.contains_key(t)) {
                    out.insert(target, cfg_test);
                }
            }
            cfg_test = false;
            path_attr = None;
        }
    }
    out
}

/// Where `mod name;` in `file` looks, in the order the compiler tries.
///
/// The crate root and `mod.rs` own their directory; any other file owns the
/// directory named after it. A `#[path]` is relative to the declaring file's
/// directory in both cases, which is the rule the reference gives for a
/// module that is not inside an inline block.
fn resolve(file: &Path, name: &str, path_attr: Option<&str>) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new(""));
    if let Some(rel) = path_attr {
        return vec![dir.join(rel)];
    }
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let children = if matches!(stem, "main" | "lib" | "mod") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    vec![
        children.join(format!("{name}.rs")),
        children.join(name).join("mod.rs"),
    ]
}

/// `body` with every item under a column-zero `#[cfg(test)]` blanked.
///
/// An item is the attribute, whatever attributes and doc comments follow it,
/// and then either one line ending in `;` with no block opened, or everything
/// up to the next column-zero `}`. rustfmt puts both at column zero, which is
/// what makes a textual rule sound here without parsing Rust.
pub(crate) fn production_of(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut keep = vec![true; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i] != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        // Attributes and docs between the cfg and the item it gates.
        while i < lines.len() && (lines[i].starts_with("#[") || lines[i].starts_with("//")) {
            i += 1;
        }
        if i < lines.len() {
            let head = lines[i];
            let one_liner = head.trim_end().ends_with(';') && !head.contains('{');
            if one_liner {
                i += 1;
            } else {
                // To the closing brace at column zero, inclusive. A `};`
                // closes a multi-line `use` the same way. Exactly a brace,
                // not a line that opens with one: a raw string inside a test
                // can hold `}} catch (e) {{`, and did.
                while i < lines.len() && !(lines[i] == "}" || lines[i] == "};") {
                    i += 1;
                }
                i = (i + 1).min(lines.len());
            }
        }
        for slot in &mut keep[start..i] {
            *slot = false;
        }
    }
    let mut out = String::with_capacity(body.len());
    for (line, keep) in lines.iter().zip(keep) {
        if keep {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    fn role_of(sources: &[Source], root: &Path, rel: &str) -> Role {
        sources
            .iter()
            .find(|s| s.path == root.join(rel))
            .unwrap_or_else(|| panic!("{rel} was not read"))
            .role
    }

    /// A file is a test module because of how it is *declared*, never
    /// because of what it is called.
    #[test]
    fn a_module_declared_under_cfg_test_is_a_test_source() {
        let dir = tree(&[
            (
                "main.rs",
                "mod report;\n#[cfg(test)]\n#[path = \"main_tests.rs\"]\nmod tests;\n",
            ),
            ("report.rs", "pub fn a() {}\n#[cfg(test)]\nmod tests;\n"),
            ("report/tests.rs", "#[test]\nfn t() {}\n"),
            ("main_tests.rs", "#[test]\nfn t() {}\n"),
        ]);
        let sources = classify(dir.path());
        assert_eq!(role_of(&sources, dir.path(), "report.rs"), Role::Production);
        assert_eq!(
            role_of(&sources, dir.path(), "report/tests.rs"),
            Role::TestModule,
            "`mod tests;` under `#[cfg(test)]` in report.rs opens report/tests.rs"
        );
        assert_eq!(
            role_of(&sources, dir.path(), "main_tests.rs"),
            Role::TestModule,
            "a `#[path]` is relative to the declaring file's directory"
        );
    }

    /// The failure this module exists to refuse: a file named like a test
    /// module that the compiler follows unconditionally is production, and
    /// every scan must read it.
    #[test]
    fn a_production_file_cannot_hide_behind_the_test_naming_rule() {
        let dir = tree(&[
            ("main.rs", "mod render_tests;\nmod cmd;\n"),
            ("render_tests.rs", "pub fn shipped() { println!(\"x\"); }\n"),
            ("cmd.rs", "pub mod session;\n"),
            ("cmd/session.rs", "pub fn s() {}\n"),
            ("stray.rs", "pub fn nobody_compiles_this() {}\n"),
        ]);
        let sources = classify(dir.path());
        assert_eq!(
            role_of(&sources, dir.path(), "render_tests.rs"),
            Role::Production,
            "declared without `#[cfg(test)]`, so its name means nothing"
        );
        assert_eq!(
            role_of(&sources, dir.path(), "cmd/session.rs"),
            Role::Production
        );
        assert_eq!(
            role_of(&sources, dir.path(), "stray.rs"),
            Role::Undeclared,
            "a file no `mod` line reaches is not part of the program"
        );
    }

    /// The two cuts this replaces, side by side: the helper must go, and what
    /// follows the helper must stay.
    #[test]
    fn a_test_helper_in_the_middle_of_a_file_hides_nothing_after_it() {
        let body = "\
pub fn shipped() {}

#[cfg(test)]
pub(crate) const FIRST: u32 = 1;

#[cfg(test)]
// A helper the tests share.
pub(crate) fn helper() -> String {
    format!(\"{}\", \"/work\")
}

pub fn also_shipped() {
    println!(\"still production\");
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        eprintln!(\"a fixture\");
    }
}
";
        let prod = production_of(body);
        assert!(prod.contains("pub fn shipped()"));
        assert!(
            prod.contains("pub fn also_shipped()") && prod.contains("still production"),
            "the production after a test-only helper is still production:\n{prod}"
        );
        for gone in [
            "FIRST",
            "fn helper",
            "/work",
            "mod tests",
            "a fixture",
            "#[test]",
        ] {
            assert!(
                !prod.contains(gone),
                "`{gone}` is test text and was kept:\n{prod}"
            );
        }
        assert_eq!(
            prod.lines().count(),
            body.lines().count(),
            "blanked, not removed, so reported line numbers stay true"
        );
    }

    /// A column-zero `}` inside a raw string in a test does not end the cut
    /// early. This is the hazard the `== "}"` check exists for — `render.rs`
    /// holds a generated snippet with `}} catch (e) {{` in a test — and it
    /// backs seven source-scanning guards, so a loosening to
    /// `.starts_with('}')` would silently corrupt all of them.
    ///
    /// The sentinel sits *after* the raw string's `}`-line, so an early stop
    /// keeps it as production and the assertion catches the loosening; a
    /// correct cut blanks the whole module and the sentinel is gone.
    #[test]
    fn a_closing_brace_inside_a_raw_string_does_not_close_the_cut() {
        let body = "\
pub fn shipped() {}

#[cfg(test)]
mod tests {
    #[test]
    fn emits_a_program() {
        let js = r\"
} caught the error
\";
        let sentinel_only_a_test_has = js.len();
        assert!(sentinel_only_a_test_has > 0);
    }
}

pub fn also_shipped() {}
";
        let prod = production_of(body);
        assert!(
            prod.contains("pub fn also_shipped()"),
            "the production after the test module survives:\n{prod}"
        );
        // The sentinel follows the raw string's `}`-prefixed line. A cut that
        // stopped at that brace would keep it; a correct one blanks the module.
        for gone in ["mod tests", "sentinel_only_a_test_has", "caught the error"] {
            assert!(
                !prod.contains(gone),
                "`{gone}` is test text and was kept:\n{prod}"
            );
        }
    }

    /// Only a column-zero attribute gates an item of the file. An indented
    /// one gates a method inside an `impl` the rest of which ships, and a
    /// mention in prose gates nothing.
    #[test]
    fn only_a_column_zero_attribute_opens_a_cut() {
        let body = "\
//! Says `#[cfg(test)]` in a doc comment.
impl Ctx {
    #[cfg(test)]
    pub fn plain() -> Self { todo!() }
    pub fn real() -> Self { todo!() }
}
";
        assert_eq!(production_of(body), body);
    }

    /// The real tree, held to the rule. Two halves: nothing under `src/` is
    /// undeclared, and the test modules are exactly the ones named here — a
    /// list, not a floor, so a file moving either way is an edit made here.
    #[test]
    fn every_rust_file_under_src_is_production_or_a_declared_test_module() {
        const TEST_MODULES: &[&str] = &["testsrc.rs"];
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = classify(&root);
        assert!(sources.len() > 30, "read {} sources", sources.len());
        let rel = |s: &Source| s.path.strip_prefix(&root).unwrap().display().to_string();
        let undeclared: Vec<String> = sources
            .iter()
            .filter(|s| s.role == Role::Undeclared)
            .map(rel)
            .collect();
        assert!(
            undeclared.is_empty(),
            "no `mod` line reaches: {undeclared:?}"
        );
        let tests: Vec<String> = sources
            .iter()
            .filter(|s| s.role == Role::TestModule)
            .map(rel)
            .collect();
        assert_eq!(tests, TEST_MODULES, "the test modules are the ones named");
    }

    /// What the cut leaves behind holds no test text, in the real tree.
    #[test]
    fn production_text_holds_no_test_only_item() {
        let mut offenders = Vec::new();
        for (path, prod) in production() {
            for (i, line) in prod.lines().enumerate() {
                if line == "#[cfg(test)]"
                    || line.starts_with("mod tests {")
                    || line.trim() == "#[test]"
                {
                    offenders.push(format!("{}:{}: {line}", path.display(), i + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }
}
