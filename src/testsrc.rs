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
    production_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
}

fn production_in(root: &Path) -> Vec<(PathBuf, String)> {
    classify(root)
        .into_iter()
        .filter(|s| s.role == Role::Production)
        .map(|s| (s.path, production_of(&s.body)))
        .collect()
}

/// Module → module it uses → where the first such use is, as a file and a
/// 1-based line. A module is a top-level one: `src/x.rs` and everything
/// under `src/x/`. Items defined in the crate root are the module `main`.
pub(crate) type Graph = BTreeMap<String, BTreeMap<String, (PathBuf, usize)>>;

/// The `use` graph between the top-level modules of the tree at `root`,
/// read from production code only: test text, comments and literals are
/// blanked first, so a path named in prose is not a dependency.
///
/// An edge is `crate::x`, a member of a grouped `use crate::{x, y::{…}}`,
/// `super::x` where the `super`s climb to the crate root, or — in the crate
/// root itself — a bare `x::` or `self::x`. A `crate::` name that is not a module is an
/// item of the crate root.
pub(crate) fn module_graph(root: &Path) -> Graph {
    let files: Vec<(PathBuf, String)> = production_in(root);
    let place = |path: &Path| -> (String, usize) {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let parts: Vec<String> = rel
            .iter()
            .map(|p| p.to_string_lossy().trim_end_matches(".rs").to_string())
            .collect();
        let depth = parts.len() - usize::from(parts.last().is_some_and(|l| l == "mod"));
        let module = if rel == Path::new("main.rs") {
            "main".to_string()
        } else {
            parts[0].clone()
        };
        let depth = if module == "main" { 0 } else { depth };
        (module, depth)
    };
    let modules: std::collections::BTreeSet<String> =
        files.iter().map(|(p, _)| place(p).0).collect();
    let mut graph = Graph::new();
    for (path, body) in &files {
        let (from, depth) = place(path);
        let code = code_of(body);
        for (n, line) in code.lines().enumerate() {
            for target in uses_on(line, depth, &modules, &code, n) {
                let target = if modules.contains(&target) {
                    target
                } else {
                    "main".to_string()
                };
                if target != from {
                    graph
                        .entry(from.clone())
                        .or_default()
                        .entry(target)
                        .or_insert_with(|| (path.clone(), n + 1));
                }
            }
        }
    }
    graph
}

/// The first segment of every path on line `n` that leaves this module for
/// another. A grouped `use` may open here and close lines later, so the whole
/// text is passed and the group is read from where it opens.
fn uses_on(
    line: &str,
    depth: usize,
    modules: &std::collections::BTreeSet<String>,
    code: &str,
    n: usize,
) -> Vec<String> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !ident(chars[i]) || (i > 0 && ident(chars[i - 1])) {
            i += 1;
            continue;
        }
        // A path: segments joined by `::`, spaces allowed around them.
        let mut segments: Vec<String> = Vec::new();
        let mut j = i;
        let mut group_at: Option<usize> = None;
        loop {
            let start = j;
            while j < chars.len() && ident(chars[j]) {
                j += 1;
            }
            segments.push(chars[start..j].iter().collect());
            let rest: String = chars[j..].iter().collect();
            let trimmed = rest.trim_start();
            let Some(after) = trimmed.strip_prefix("::") else {
                break;
            };
            let after_ws = after.trim_start();
            j = chars.len() - after_ws.chars().count();
            if after_ws.starts_with('{') {
                group_at = Some(j);
                break;
            }
            if !after_ws.starts_with(|c: char| ident(c)) {
                break;
            }
        }
        let next = j.max(i + 1);
        let supers = segments.iter().take_while(|s| *s == "super").count();
        // How many segments lead to the crate root, if this path goes there.
        let to_root = match segments[0].as_str() {
            "crate" => Some(1),
            "super" if depth > 0 && supers == depth => Some(supers),
            "self" if depth == 0 => Some(1),
            // A path, not the bare name: `mod x;` declares, it does not use.
            first
                if depth == 0
                    && modules.contains(first)
                    && first != "main"
                    && (segments.len() > 1 || group_at.is_some()) =>
            {
                Some(0)
            }
            _ => None,
        };
        if let Some(lead) = to_root {
            if let Some(name) = segments.get(lead) {
                out.push(name.clone());
            } else if let Some(at) = group_at {
                out.extend(group_members(code, n, at));
            }
        }
        i = next;
    }
    out
}

/// The first segment of each member of the `{ … }` group opening at column
/// `col` of line `n`, read to its closing brace however many lines that is.
fn group_members(code: &str, n: usize, col: usize) -> Vec<String> {
    let text: String = code
        .lines()
        .skip(n)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .skip(col + 1)
        .collect();
    let mut out = Vec::new();
    let mut depth = 0;
    let mut expecting = true;
    for (k, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => break,
            '}' => depth -= 1,
            ',' if depth == 0 => expecting = true,
            c if expecting && depth == 0 && (c.is_alphanumeric() || c == '_') => {
                let name: String = text[k..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name != "self" {
                    out.push(name);
                }
                expecting = false;
            }
            _ => {}
        }
    }
    out
}

/// `body` with comments and literals blanked to spaces, newlines kept, so a
/// path that only appears in prose or in a string is not read as code and a
/// line number stays the line in the file.
///
/// Comments are `//` to the end of the line and `/* */`, which nest. Literals
/// are strings with their escapes, raw strings with any number of `#`, byte
/// strings, and char literals — told apart from a lifetime by what follows
/// the quote: an escape, or one character and a closing quote.
pub(crate) fn code_of(body: &str) -> String {
    let c: Vec<char> = body.chars().collect();
    let mut out: Vec<char> = c.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for slot in &mut out[from..to.min(c.len())] {
            if *slot != '\n' {
                *slot = ' ';
            }
        }
    };
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut i = 0;
    while i < c.len() {
        let at = |k: usize| c.get(k).copied();
        if c[i] == '/' && at(i + 1) == Some('/') {
            let end = (i..c.len()).find(|&k| c[k] == '\n').unwrap_or(c.len());
            blank(&mut out, i, end);
            i = end;
        } else if c[i] == '/' && at(i + 1) == Some('*') {
            let (mut k, mut depth) = (i + 2, 1);
            while k < c.len() && depth > 0 {
                if c[k] == '/' && at(k + 1) == Some('*') {
                    depth += 1;
                    k += 2;
                } else if c[k] == '*' && at(k + 1) == Some('/') {
                    depth -= 1;
                    k += 2;
                } else {
                    k += 1;
                }
            }
            blank(&mut out, i, k);
            i = k;
        } else if (c[i] == 'r' || (c[i] == 'b' && at(i + 1) == Some('r')))
            && (i == 0 || !ident(c[i - 1]))
            && {
                let k = i + if c[i] == 'b' { 2 } else { 1 };
                let hashes = (k..c.len()).take_while(|&h| c[h] == '#').count();
                at(k + hashes) == Some('"')
            }
        {
            let k = i + if c[i] == 'b' { 2 } else { 1 };
            let hashes = (k..c.len()).take_while(|&h| c[h] == '#').count();
            let open = k + hashes + 1;
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let end = (open..c.len())
                .find(|&e| c[e..].starts_with(&close))
                .map_or(c.len(), |e| e + close.len());
            blank(&mut out, i, end);
            i = end;
        } else if c[i] == '"' {
            let mut k = i + 1;
            while k < c.len() && c[k] != '"' {
                k += if c[k] == '\\' { 2 } else { 1 };
            }
            blank(&mut out, i, k + 1);
            i = k + 1;
        } else if c[i] == '\'' && at(i + 1) == Some('\\') {
            let end = (i + 3..c.len()).find(|&e| c[e] == '\'').unwrap_or(c.len());
            blank(&mut out, i, end + 1);
            i = end + 1;
        } else if c[i] == '\'' && at(i + 2) == Some('\'') {
            blank(&mut out, i, i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// The strongly connected components of more than one module, each sorted,
/// in sorted order. Tarjan's algorithm.
pub(crate) fn cycles(graph: &Graph) -> Vec<Vec<String>> {
    struct Walk<'g> {
        graph: &'g Graph,
        index: BTreeMap<&'g str, usize>,
        low: BTreeMap<&'g str, usize>,
        stack: Vec<&'g str>,
        found: Vec<Vec<String>>,
    }
    impl<'g> Walk<'g> {
        fn visit(&mut self, v: &'g str) {
            let n = self.index.len();
            self.index.insert(v, n);
            self.low.insert(v, n);
            self.stack.push(v);
            for w in self.graph.get(v).into_iter().flat_map(|t| t.keys()) {
                let w = w.as_str();
                if !self.index.contains_key(w) {
                    self.visit(w);
                    let lw = self.low[w];
                    let lv = self.low.get_mut(v).unwrap();
                    *lv = (*lv).min(lw);
                } else if self.stack.contains(&w) {
                    let iw = self.index[w];
                    let lv = self.low.get_mut(v).unwrap();
                    *lv = (*lv).min(iw);
                }
            }
            if self.low[v] == self.index[v] {
                let mut component = Vec::new();
                while let Some(w) = self.stack.pop() {
                    component.push(w.to_string());
                    if w == v {
                        break;
                    }
                }
                if component.len() > 1 {
                    component.sort();
                    self.found.push(component);
                }
            }
        }
    }
    let mut walk = Walk {
        graph,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: Vec::new(),
        found: Vec::new(),
    };
    for v in graph.keys() {
        if !walk.index.contains_key(v.as_str()) {
            walk.visit(v);
        }
    }
    walk.found.sort();
    walk.found
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

    fn edge_set(graph: &Graph) -> Vec<(String, String)> {
        graph
            .iter()
            .flat_map(|(from, to)| to.keys().map(move |t| (from.clone(), t.clone())))
            .collect()
    }

    /// Every shape a module edge hides in, and every shape that only looks
    /// like one: a path in a doc comment, a nested block comment, a string, a
    /// raw string holding a quote, a byte string, a test-only item. A char
    /// literal holding `"` must not open a string, a lifetime must not open a
    /// char literal, and a `//` inside a string must not open a comment —
    /// each of those, mishandled, swallows the edge on the line after it.
    #[test]
    fn the_module_graph_reads_edges_from_code_and_nothing_else() {
        let dir = tree(&[
            (
                "main.rs",
                "mod a;\nmod b;\nmod c;\nmod d;\nmod e;\nmod f;\nmod g;\nmod h;\n\
                 use a::Thing;\n\
                 pub(crate) fn helper() {}\n\
                 fn main() { b::go(); std::process::exit(0); }\n",
            ),
            (
                "a.rs",
                "use crate::{\n    b::{One, Two},\n    c,\n};\n\
                 pub struct Thing;\n\
                 pub fn quote() -> char { '\"' }\n\
                 pub fn id<'x>(s: &'x str) -> &'x str { crate::d::id(s) }\n",
            ),
            (
                "b.rs",
                "//! Names `crate::e` in a doc comment.\n\
                 /* a block /* nested, naming crate::e */ still crate::e */\n\
                 pub fn go() {\n\
                 \x20   let s = \"crate::e \\\" crate::e\";\n\
                 \x20   let r = r#\"one \" crate::e\"#;\n\
                 \x20   let y = b\"crate::e\";\n\
                 \x20   let _ = (s, r, y);\n\
                 }\n\
                 pub use crate::f::Exported;\n",
            ),
            (
                "c.rs",
                "pub fn x() { crate::helper() }\n\
                 #[cfg(test)]\nmod tests {\n    use crate::g;\n}\n",
            ),
            ("d.rs", "pub fn id(s: &str) -> &str { super::e::y(s) }\n"),
            (
                "e.rs",
                "mod inner;\npub struct Own;\npub fn y(s: &str) -> &str { s }\n",
            ),
            ("e/inner.rs", "use super::Own;\nuse super::super::g;\n"),
            (
                "f.rs",
                "pub struct Exported;\n\
                 pub fn line() -> String { format!(\"  // {}\\n\", crate::h::NAME) }\n",
            ),
            ("g.rs", "pub fn g() {}\n"),
            ("h.rs", "pub const NAME: &str = \"h\";\n"),
        ]);
        let graph = module_graph(dir.path());
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            edge_set(&graph),
            vec![
                pair("a", "b"),
                pair("a", "c"),
                pair("a", "d"),
                pair("b", "f"),
                pair("c", "main"),
                pair("d", "e"),
                pair("e", "g"),
                pair("f", "h"),
                pair("main", "a"),
                pair("main", "b"),
            ],
            "{graph:#?}"
        );
        let (path, line) = &graph["a"]["d"];
        assert_eq!(
            (path.strip_prefix(dir.path()).unwrap(), *line),
            (Path::new("a.rs"), 7),
            "an edge names where it was found, by the line in the file"
        );
    }

    /// A cycle is a strongly connected component of more than one module, and
    /// each is reported once, sorted, whatever order the edges arrive in.
    #[test]
    fn cycles_are_the_components_of_more_than_one_module() {
        let mut graph = Graph::new();
        for (from, to) in [
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("c", "d"),
            ("d", "e"),
            ("e", "d"),
            ("f", "a"),
        ] {
            graph
                .entry(from.to_string())
                .or_default()
                .insert(to.to_string(), (PathBuf::new(), 0));
        }
        assert_eq!(
            cycles(&graph),
            vec![vec!["a", "b", "c"], vec!["d", "e"]],
            "f reaches the cycle but is not in it"
        );
    }

    /// The modules that use each other, as they stand. A ratchet: a new cycle
    /// fails, and so does breaking one without striking it here — the list is
    /// exact, so it only ever shrinks on purpose. When it is empty, the graph
    /// is a hierarchy and this asserts it stays one.
    #[test]
    fn the_modules_form_no_cycle_beyond_the_known_ones() {
        const KNOWN_CYCLES: &[&[&str]] = &[
            &["adapter", "hook"],
            &[
                "ask",
                "auth",
                "base",
                "catalogue",
                "cli",
                "cmd",
                "config",
                "container",
                "derive",
                "detect",
                "doctor",
                "facts",
                "idle",
                "image",
                "key",
                "main",
                "memory",
                "notice",
                "render",
                "report",
                "rules",
                "runtime",
                "session",
                "settings",
                "shadow",
                "ssh",
                "stack",
                "why",
            ],
        ];
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let graph = module_graph(&root);

        // Liveness: a blanker that ate code would find no cycles and pass.
        let edges = graph.values().map(BTreeMap::len).sum::<usize>();
        assert!(edges > 150, "only {edges} edges read: {graph:#?}");
        for (from, to) in [("main", "cli"), ("cmd", "session"), ("doctor", "image")] {
            assert!(
                graph.get(from).is_some_and(|t| t.contains_key(to)),
                "{from} → {to} is in the code and was not read"
            );
        }

        let found = cycles(&graph);
        let known: Vec<Vec<String>> = KNOWN_CYCLES
            .iter()
            .map(|c| c.iter().map(|m| m.to_string()).collect())
            .collect();
        let rel = |p: &Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
        let explain = |cycle: &[String]| -> String {
            let mut out = String::new();
            for from in cycle {
                for (to, (path, line)) in &graph[from] {
                    if cycle.contains(to) {
                        out.push_str(&format!("\n    {from} → {to}  ({}:{line})", rel(path)));
                    }
                }
            }
            out
        };
        let report: String = found
            .iter()
            .map(|c| format!("\n  {c:?}{}", explain(c)))
            .collect();
        assert_eq!(
            found, known,
            "the cycles between modules changed. A new one is a dependency to \
             invert; a broken one is progress — strike it from KNOWN_CYCLES.\
             \nNow:{report}"
        );
    }

    /// The real tree, held to the rule. Two halves: nothing under `src/` is
    /// undeclared, and the test modules are exactly the ones named here — a
    /// list, not a floor, so a file moving either way is an edit made here.
    #[test]
    fn every_rust_file_under_src_is_production_or_a_declared_test_module() {
        const TEST_MODULES: &[&str] = &[
            "main_tests.rs",
            "report/tests.rs",
            "shadow/tests.rs",
            "testsrc.rs",
            "tui/tests.rs",
        ];
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
