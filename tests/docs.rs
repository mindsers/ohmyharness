//! The documentation is a tree, not a file, and a tree rots differently.
//!
//! One `DESIGN.md` had exactly three inbound links and you could check them by
//! looking. Splitting it across `docs/` turns cross-references into the same
//! kind of unverified claim as an adapter path: plausible, cheap to write, and
//! silently wrong the moment a file is renamed. Nothing else in this repo
//! notices — a dead link does not fail a build, and the reader who finds it is
//! by definition the person who could least afford the detour.
//!
//! So the tree gets the treatment everything else here gets: an invariant with
//! a test that goes red without it.
//!
//! **Two kinds of page, and a link between them is spelled one way.** The
//! pages under `docs/src/content/docs/` are published as a site, and a page of
//! the site is addressed by its route — `/reference/commands/` from another
//! page, the same route after [`SITE`] from anywhere else. Everything else is
//! read where it lies, on GitHub: from a page of the site it is addressed by
//! its URL under [`REPOSITORY`], and from a neighbour by a relative path.
//!
//! The site's own build checks none of this. It publishes a link to a page
//! that does not exist, and the first thing to notice is a reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the site is served. `docs/astro.config.mjs` says so too, and
/// `the_site_is_configured_for_the_address_these_guards_resolve` holds the two
/// together.
const SITE: &str = "https://docs.ohmyharness.sh";

/// Where the rest of the tree is read.
const REPOSITORY: &str = "https://github.com/mindsers/ohmyharness";

/// The pages the site publishes, from the repository root.
const CONTENT: &str = "docs/src/content/docs";

/// Canonical, so that a path reached through `..` and one reached by walking
/// can be compared.
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap()
}

fn shown(file: &Path) -> String {
    file.strip_prefix(repo())
        .unwrap_or(file)
        .display()
        .to_string()
}

/// Every file under the repository whose extension is one of `extensions`,
/// minus build output and anything git ignores by convention. Walked rather
/// than listed: a file that is not walked is a file nothing checks, and the
/// failure mode of a stale list is that it silently stops covering new ones.
fn files(extensions: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            // `.github` is walked despite the dot: CONTRIBUTING, SECURITY and
            // the code of conduct are prose with links, and links rot there
            // exactly as they do under docs/. It only became load-bearing when
            // CONTRIBUTING.md moved out of docs/ — a rename that would
            // otherwise have quietly removed the file from this check.
            // Everything else beginning with a dot is skipped, `.git` above all.
            //
            // `node_modules` is what installing the site leaves beside it:
            // other people's pages, whose links are theirs to keep.
            let hidden = name.starts_with('.') && name != ".github";
            if hidden || name == "target" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(&p, extensions, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| extensions.contains(&x))
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo(), extensions, &mut out);
    out.sort();
    out
}

fn markdown_files() -> Vec<PathBuf> {
    files(&["md"])
}

fn is_page(file: &Path) -> bool {
    file.starts_with(repo().join(CONTENT))
}

/// The pages of the site.
///
/// Named files, because every guard below that says *no page does X* is only
/// as good as this list: a walk that stopped early and a walk that found
/// nothing wrong look the same from an empty failure.
fn pages() -> Vec<PathBuf> {
    let pages: Vec<PathBuf> = markdown_files()
        .into_iter()
        .filter(|f| is_page(f))
        .collect();
    for must in [
        "index.md",
        "reference/commands.md",
        "guides/troubleshooting.md",
    ] {
        assert!(
            pages.iter().any(|p| p == &repo().join(CONTENT).join(must)),
            "the walk never reached {CONTENT}/{must}, so its silence says nothing"
        );
    }
    pages
}

/// The address a page is served at: `/reference/commands/` for
/// `reference/commands.md`, and a directory's own address for its `index.md`.
///
/// With the trailing slash: the site is built as a directory for each page,
/// and this is the one spelling these guards accept for it.
fn route(page: &Path) -> String {
    let relative = page
        .strip_prefix(repo().join(CONTENT))
        .unwrap()
        .with_extension("");
    let mut parts: Vec<String> = relative
        .iter()
        .map(|part| part.to_string_lossy().to_string())
        .collect();
    if parts.last().is_some_and(|last| last == "index") {
        parts.pop();
    }
    match parts.is_empty() {
        true => "/".to_string(),
        false => format!("/{}/", parts.join("/")),
    }
}

fn routes() -> BTreeMap<String, PathBuf> {
    pages().into_iter().map(|p| (route(&p), p)).collect()
}

/// Inline markdown links: `[text](target)`. Deliberately not a full parser —
/// it needs to find link targets, and anything it cannot parse it must skip
/// rather than guess at, since a false failure trains people to ignore it.
fn links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ']' && i + 1 < bytes.len() && bytes[i + 1] == '(' {
            let mut depth = 1;
            let mut j = i + 2;
            let mut target = String::new();
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                target.push(bytes[j]);
                j += 1;
            }
            if depth == 0 {
                // A link may carry a title: [t](path "Title"). Keep the path.
                let t = target.split_whitespace().next().unwrap_or("").to_string();
                out.push(t);
            }
            i = j;
        }
        i += 1;
    }
    out
}

/// What a link target addresses.
enum Target<'a> {
    /// A spot on the page the link is written in.
    Here(&'a str),
    /// A page of the site, by route, and maybe a spot on it.
    Site(&'a str),
    /// A file in this repository as GitHub shows it on `main`.
    Repository(&'a str),
    /// Somewhere this repository does not control.
    Elsewhere,
    /// A path from the file the link is written in.
    Relative(&'a str),
}

fn target(link: &str) -> Target<'_> {
    if let Some(spot) = link.strip_prefix('#') {
        return Target::Here(spot);
    }
    if let Some(rest) = link.strip_prefix(SITE) {
        // The bare address is the front page.
        return Target::Site(if rest.is_empty() { "/" } else { rest });
    }
    for view in ["/blob/main/", "/tree/main/"] {
        if let Some(path) = link
            .strip_prefix(REPOSITORY)
            .and_then(|rest| rest.strip_prefix(view))
        {
            return Target::Repository(path);
        }
    }
    if link.starts_with("http://") || link.starts_with("https://") || link.starts_with("mailto:") {
        return Target::Elsewhere;
    }
    if link.starts_with('/') {
        return Target::Site(link);
    }
    Target::Relative(link)
}

fn split(addressed: &str) -> (&str, Option<&str>) {
    match addressed.split_once('#') {
        Some((path, spot)) => (path, Some(spot).filter(|s| !s.is_empty())),
        None => (addressed, None),
    }
}

/// The page with its front matter off: what the reader is shown.
fn prose(body: &str) -> &str {
    let Some(rest) = body.strip_prefix("---\n") else {
        return body;
    };
    match rest.find("\n---\n") {
        Some(end) => &rest[end + "\n---\n".len()..],
        None => body,
    }
}

/// The lines of a page that are not inside a code fence.
///
/// A `# comment` in a shell block is not a heading. Read as one it is an
/// anchor no reader can reach, and a link to it resolves here and nowhere
/// else.
fn outside_fences(body: &str) -> Vec<&str> {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in prose(body).lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push(line);
        }
    }
    out
}

/// GitHub's heading slug: lowercase, drop punctuation, spaces become hyphens.
/// Taken from what the reader is shown rather than from what was typed —
/// `` `omh bench` `` renders as a heading whose anchor contains no backticks,
/// and half the headings here are code.
///
/// The site slugs a heading the same way, and that is measured rather than
/// assumed: every heading id in a build of the site was compared with what
/// this returns for the page it came from, and
/// `a_heading_is_slugged_the_way_the_site_slugs_it` keeps the pairs that
/// disagreed until code spans were read as spans.
fn slug(heading: &str) -> String {
    let written: Vec<char> = heading.trim_start_matches('#').trim().chars().collect();
    let run = |from: usize| written[from..].iter().take_while(|c| **c == '`').count();
    let mut shown = String::new();
    let mut i = 0;
    while i < written.len() {
        if written[i] != '`' {
            shown.push(written[i]);
            i += 1;
            continue;
        }
        // A code span runs to the next fence of the same length, so a longer
        // one can hold a backtick. Its text is shown as written, less one
        // space of padding on each side when both sides have one.
        let fence = run(i);
        let mut j = i + fence;
        let mut close = None;
        while j < written.len() && close.is_none() {
            match written[j] {
                '`' if run(j) == fence => close = Some(j),
                '`' => j += run(j),
                _ => j += 1,
            }
        }
        let Some(close) = close else {
            // A fence nothing closes is punctuation, and goes with the rest.
            i += fence;
            continue;
        };
        let mut span = &written[i + fence..close];
        let padded = span.len() > 2 && span[0] == ' ' && span[span.len() - 1] == ' ';
        if padded && span.iter().any(|c| *c != ' ') {
            span = &span[1..span.len() - 1];
        }
        shown.extend(span);
        i = close + fence;
    }
    // Underscores survive: GitHub strips punctuation but keeps `-` and `_`, so
    // a `carry_in` heading anchors at #carry_in, not #carryin. `_` is emphasis
    // syntax too, but in these headings it is almost always an identifier.
    shown
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// Every anchor a page offers. A heading that repeats one above it gets a
/// number after its slug — `-1`, then `-2` — which is what a build of the site
/// gave three headings of the same name.
fn anchors(body: &str) -> BTreeSet<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeSet::new();
    for line in outside_fences(body) {
        // One to six `#` and then a space. `#53` opening a wrapped sentence
        // is prose, and so is a seventh `#`.
        let level = line.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) || !line[level..].starts_with(' ') {
            continue;
        }
        let slug = slug(line);
        let count = seen.entry(slug.clone()).or_insert(0);
        out.insert(match *count {
            0 => slug,
            n => format!("{slug}-{n}"),
        });
        *count += 1;
    }
    out
}

/// Why a link is dead, or `None` when it leads somewhere.
///
/// One function, so that a link is judged the same way whichever guard is
/// asking and no spelling is checked for its file by one and for its anchor by
/// nobody.
fn dead(file: &Path, body: &str, link: &str, routes: &BTreeMap<String, PathBuf>) -> Option<String> {
    let missing = |spot: &str, at: &Path| {
        let read = fs::read_to_string(at).ok()?;
        (!anchors(&read).contains(spot)).then(|| format!("no heading `#{spot}` in {}", shown(at)))
    };
    match target(link) {
        Target::Elsewhere => None,
        Target::Here(spot) => match spot.is_empty() || anchors(body).contains(spot) {
            true => None,
            false => Some(format!("no heading `#{spot}` on this page")),
        },
        Target::Site(addressed) => {
            let (path, spot) = split(addressed);
            let Some(page) = routes.get(path) else {
                return Some(match routes.contains_key(&format!("{path}/")) {
                    true => format!("the site serves `{path}/`, with the slash"),
                    false => format!("the site has no page at `{path}`"),
                });
            };
            spot.and_then(|spot| missing(spot, page))
        }
        Target::Repository(addressed) => {
            let (path, spot) = split(addressed);
            let at = repo().join(path);
            if !at.exists() {
                return Some(format!("the repository has no `{path}`"));
            }
            // A line number is a spot too, and one nothing here can check.
            let heading = spot.filter(|_| path.ends_with(".md"));
            heading.and_then(|spot| missing(spot, &at))
        }
        Target::Relative(addressed) => {
            if is_page(file) {
                return Some(
                    "a page of the site links by route (`/reference/commands/`) or by \
                     URL — a relative path is published as written and leads nowhere"
                        .to_string(),
                );
            }
            let (path, spot) = split(addressed);
            let at = file.parent().unwrap().join(path);
            if !at.exists() {
                return Some("no such file".to_string());
            }
            if let Some(page) = at.canonicalize().ok().filter(|at| is_page(at)) {
                return Some(format!(
                    "a page of the site is linked by its address, {SITE}{}",
                    route(&page)
                ));
            }
            spot.and_then(|spot| missing(spot, &at))
        }
    }
}

/// A link that points at nothing is a dead end for the one reader who followed
/// it, and nothing in the build notices. Its anchor is the half that breaks
/// during exactly the kind of edit that renames a section — the link still
/// resolves, so nothing complains, and the reader lands at the top of a long
/// page with no idea what they were meant to be looking at.
#[test]
fn every_link_resolves() {
    let routes = routes();
    let mut broken: Vec<String> = Vec::new();
    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        for link in links(&body) {
            if let Some(why) = dead(&file, &body, &link, &routes) {
                broken.push(format!("{} → {link}\n      {why}", shown(&file)));
            }
        }
    }
    assert!(broken.is_empty(), "dead links:\n  {}", broken.join("\n  "));
}

/// The site's address outside a markdown link: in a message omh prints, in a
/// manifest `omh why` reads out, in a workflow, in a comment.
///
/// These are the ones a reader cannot repair. A dead link on a page is one
/// click from the page that replaced it; a dead address in a refusal is all
/// the binary had to say, and it stays in every copy installed until the next
/// release.
#[test]
fn every_address_on_the_site_is_a_page() {
    let routes = routes();
    let mut broken: Vec<String> = Vec::new();
    let mut read: BTreeSet<String> = BTreeSet::new();
    for file in files(&[
        "rs", "md", "toml", "sh", "yml", "yaml", "tmpl", "mjs", "json",
    ]) {
        let Ok(body) = fs::read_to_string(&file) else {
            continue;
        };
        read.insert(shown(&file));
        for (n, line) in body.lines().enumerate() {
            for (at, _) in line.match_indices(SITE) {
                let written: String = line[at..]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !"`\"'()<>[]{},\\".contains(*c))
                    .collect();
                // A sentence ends where its address does.
                let address = written.trim_end_matches(['.', ':', ';', '!', '?']);
                if let Some(why) = dead(&file, &body, address, &routes) {
                    broken.push(format!(
                        "{}:{} → {address}\n      {why}",
                        shown(&file),
                        n + 1
                    ));
                }
            }
        }
    }
    for must in ["README.md", "src/image.rs", "docs/astro.config.mjs"] {
        assert!(
            read.contains(must),
            "the scan never read {must}, so its silence says nothing"
        );
    }
    assert!(
        broken.is_empty(),
        "addresses the site does not serve:\n  {}",
        broken.join("\n  ")
    );
}

/// A line with every link pinned to a commit taken out of it.
///
/// `…/blob/<forty hex digits>/docs/design/gone.md` names the path a page had
/// *at that commit*, which is the one way left to cite a page that has gone,
/// and it goes on resolving whatever happens to the tree afterwards. Forty
/// digits and nothing shorter: an abbreviation is unique until the repository
/// grows another object that starts the same way.
fn without_pinned_links(line: &str) -> String {
    let pinned = format!("{REPOSITORY}/blob/");
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find(&pinned) {
        let after = &rest[at + pinned.len()..];
        let commit = after.split('/').next().unwrap_or("");
        let is_commit = commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit());
        let end = match is_commit {
            // To where the address ends, as the scan for the site's own does.
            true => after
                .find(|c: char| c.is_whitespace() || "`\"'()<>[]{},\\".contains(c))
                .unwrap_or(after.len()),
            false => 0,
        };
        out.push_str(&rest[..at]);
        if !is_commit {
            out.push_str(&pinned);
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Every page that has left its path: the eight that were files directly under
/// `docs/` until the site was built, the eight design pages that followed them
/// into it, and the design pages that have been dissolved since.
///
/// Keyed on what left, like the scan for retired verbs and for its reason: a
/// guard that reads the links which exist cannot see a path that is no longer
/// one. And a path in a comment or a printed message was never a link — when
/// these pages moved, two messages omh prints named a file under `docs/` that
/// the move took away, and nothing that checks links could have said so.
///
/// Assembled rather than written whole: the sweep that moves a page is a
/// search and replace over the tree, and it must not be able to fix the guard.
#[test]
fn nothing_names_a_page_by_the_path_it_left() {
    let left: Vec<String> = [
        "accounts",
        "code-graph",
        "commands",
        "configuration",
        "editors",
        "getting-started",
        "sessions",
        "troubleshooting",
        "design/adapters",
        "design/architecture",
        "design/base-set",
        "design/decisions",
        "design/distribution",
        "design/risks",
        "design/roadmap",
        "design/trust",
        "design/git",
        "design/adoption",
        "design/memory",
        "design/memory-benchmark",
        "design/memory-m0",
        "design/memory-rationale",
    ]
    .iter()
    .map(|page| format!("docs/{page}.md"))
    .collect();

    let mut found: Vec<String> = Vec::new();
    let mut read: BTreeSet<String> = BTreeSet::new();
    for file in files(&[
        "rs", "md", "toml", "sh", "yml", "yaml", "tmpl", "mjs", "json",
    ]) {
        let Ok(body) = fs::read_to_string(&file) else {
            continue;
        };
        read.insert(shown(&file));
        for (n, line) in body.lines().enumerate() {
            let line = without_pinned_links(line);
            if left.iter().any(|path| line.contains(path.as_str())) {
                found.push(format!("{}:{}: {}", shown(&file), n + 1, line.trim()));
            }
        }
    }
    for must in [
        "README.md",
        "src/image.rs",
        "tests/cli.rs",
        ".github/CONTRIBUTING.md",
    ] {
        assert!(
            read.contains(must),
            "the scan never read {must}, so its silence says nothing"
        );
    }
    assert!(
        found.is_empty(),
        "these name a page where it used to be. It is under {CONTENT}/ now, and \
         served from {SITE}:\n  {}",
        found.join("\n  ")
    );
}

/// The directories the site's sidebar lists, read from its configuration.
fn sidebar() -> BTreeSet<String> {
    let config = fs::read_to_string(repo().join("docs/astro.config.mjs")).unwrap();
    let listed: BTreeSet<String> = config
        .split("directory:")
        .skip(1)
        .filter_map(|after| {
            let after = after.trim_start();
            let quote = after.chars().next().filter(|c| "'\"".contains(*c))?;
            after[1..].split(quote).next().map(str::to_string)
        })
        .collect();
    assert!(
        !listed.is_empty(),
        "docs/astro.config.mjs lists no directory in its sidebar, or lists them \
         in a shape this no longer reads"
    );
    listed
}

/// A page nothing links to is a page nobody reads. When docs were one file that
/// was impossible by construction; in a tree it is the default outcome of
/// adding a file and forgetting the index, which is how documentation quietly
/// becomes a place things go to be lost.
///
/// On the site the index is the sidebar, and it is built from directories
/// rather than from links: a page is listed because of where it is. So a page
/// of the site is reachable when the sidebar lists its directory, and any
/// other page under `docs/` when something links to it.
#[test]
fn every_docs_page_is_reachable() {
    let sidebar = sidebar();
    let content = repo().join(CONTENT);
    let pages = pages();
    let mut lost: Vec<String> = Vec::new();

    for page in &pages {
        let relative = page.strip_prefix(&content).unwrap();
        let mut parts = relative.iter();
        let first = parts.next().unwrap().to_string_lossy().to_string();
        // The front page is where the address itself leads.
        let front = first == "index.md";
        if !front && !sidebar.contains(&first) {
            lost.push(format!("{} — the sidebar lists no `{first}`", shown(page)));
        }
    }
    for directory in &sidebar {
        let holds_a_page = pages.iter().any(|p| p.starts_with(content.join(directory)));
        if !holds_a_page {
            lost.push(format!(
                "the sidebar lists `{directory}`, and {CONTENT}/{directory}/ holds no page"
            ));
        }
    }

    let mut linked: BTreeSet<PathBuf> = BTreeSet::new();
    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        for link in links(&body) {
            let at = match target(&link) {
                Target::Relative(addressed) => file.parent().unwrap().join(split(addressed).0),
                Target::Repository(addressed) => repo().join(split(addressed).0),
                _ => continue,
            };
            if let Ok(at) = at.canonicalize() {
                linked.insert(at);
            }
        }
    }
    let docs = repo().join("docs");
    lost.extend(
        markdown_files()
            .into_iter()
            .filter(|f| f.starts_with(&docs) && !is_page(f))
            .filter(|f| f.file_name().is_some_and(|n| n != "README.md"))
            .filter(|f| !f.canonicalize().is_ok_and(|c| linked.contains(&c)))
            .map(|f| format!("{} — nothing links to it", shown(&f))),
    );

    assert!(
        lost.is_empty(),
        "unreachable pages:\n  {}",
        lost.join("\n  ")
    );
}

/// Everything the site publishes is a page these guards read.
///
/// The site takes a page's address from its path, and takes it through a
/// slugger: `Code Graph.md` is served at `/code-graph/`. Every guard here
/// reads the address off the path instead, so a name the slugger would change
/// is a page whose links are checked against an address it is not served at.
/// And a page written in anything but markdown is one no guard opens at all.
#[test]
fn every_page_is_served_at_the_path_it_is_filed_under() {
    let content = repo().join(CONTENT);
    let mut odd: Vec<String> = Vec::new();
    let mut stack = vec![content.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            // What a desktop leaves in a directory is not a page.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let relative = path.strip_prefix(&content).unwrap();
            let plain = relative.with_extension("").iter().all(|part| {
                let part = part.to_string_lossy();
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
            if path.extension().is_none_or(|e| e != "md") || !plain {
                odd.push(shown(&path));
            }
        }
    }
    odd.sort();
    assert!(
        odd.is_empty(),
        "a page of the site is a `.md` file whose path is lowercase letters, \
         digits and hyphens:\n  {}",
        odd.join("\n  ")
    );
}

/// A page says what it is called once, in its front matter.
///
/// The site prints `title` as the page's heading, lists the page under it in
/// the sidebar, and refuses to build a page without one. `description` is what
/// a search result and a link preview show, and the site builds without it —
/// so it is the half that goes missing. A `# Heading` in the body is the
/// opposite mistake, and the natural one when a page is moved in from a file
/// that needed it: the reader gets two titles, one above the other.
#[test]
fn every_page_introduces_itself_once() {
    let mut wrong: Vec<String> = Vec::new();
    for page in pages() {
        let body = fs::read_to_string(&page).unwrap();
        let front = body
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front);
        let Some(front) = front else {
            wrong.push(format!("{} — no front matter", shown(&page)));
            continue;
        };
        for key in ["title", "description"] {
            let stated = front.lines().any(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .is_some_and(|value| !value.trim().is_empty())
            });
            if !stated {
                wrong.push(format!("{} — no `{key}`", shown(&page)));
            }
        }
        if let Some(heading) = outside_fences(&body).iter().find(|l| l.starts_with("# ")) {
            wrong.push(format!(
                "{} — `{heading}` is a second title; the first is in the front matter",
                shown(&page)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "pages that do not introduce themselves once:\n  {}",
        wrong.join("\n  ")
    );
}

/// The guards above resolve addresses under [`SITE`] and send readers to
/// [`REPOSITORY`]. Both are also written in the site's own configuration, and a
/// constant that disagrees with the thing it describes checks every link
/// against a site that is not the one being published.
#[test]
fn the_site_is_configured_for_the_address_these_guards_resolve() {
    let config = fs::read_to_string(repo().join("docs/astro.config.mjs")).unwrap();
    assert!(
        config.contains(&format!("site: '{SITE}'")),
        "docs/astro.config.mjs does not publish to {SITE}"
    );
    // The link under every page that offers to edit it. It is assembled from
    // this prefix and the page's path, so it is right for every page or none.
    assert!(
        config.contains(&format!("baseUrl: '{REPOSITORY}/edit/main/docs/'")),
        "docs/astro.config.mjs does not send an edit to {REPOSITORY}, on main, under docs/"
    );
}

/// Backticked bare semver: `` `0.1.0` ``. Three parts, no `v`, which is how
/// this repo writes *its own* version — another project's goes with its tag
/// prefix (`` `v0.19.0` `` for iwe) or with fewer parts (`` `2026.08` `` for a
/// base manifest), and both stay out of the way here.
///
/// omh's *past* releases take the tag prefix for the same reason. "Fixed in
/// `` `v0.2.1` ``" in troubleshooting is a fact about which release carried a
/// fix, not a claim about what the crate is now: bumping it with the version
/// would make it false, and leaving it bare would fail the guard below at every
/// release until someone did. The prefix says "a release, not this one".
fn stated_versions(body: &str) -> Vec<(usize, String)> {
    body.lines()
        .enumerate()
        .flat_map(|(i, line)| {
            line.split('`')
                .skip(1)
                .step_by(2)
                .filter(|span| {
                    let parts: Vec<&str> = span.split('.').collect();
                    parts.len() == 3
                        && parts
                            .iter()
                            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                })
                .map(move |span| (i + 1, span.to_string()))
        })
        .collect()
}

/// A release bumps `Cargo.toml` and the tag, and both are checked against each
/// other by the release workflow. The prose is the third copy, nothing checks
/// it, and it is the one a reader meets first — so a page introducing itself
/// with **Status: early.** `0.1.0` outlives the release it describes and
/// undersells or oversells the thing depending on which way the drift went.
#[test]
fn no_page_states_a_version_the_crate_does_not() {
    let crate_version = env!("CARGO_PKG_VERSION");
    let mut stale: Vec<String> = Vec::new();

    for file in markdown_files() {
        let body = fs::read_to_string(&file).unwrap();
        for (line, stated) in stated_versions(&body) {
            if stated != crate_version {
                stale.push(format!("{}:{line} says `{stated}`", shown(&file)));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "the crate is at {crate_version}:\n  {}",
        stale.join("\n  ")
    );
}

/// The README is the entry point, so a link out of it that dies is the most
/// expensive one in the repo. `every_link_resolves` holds each one; this holds
/// that there is one to hold.
#[test]
fn the_readme_points_at_the_site() {
    let body = fs::read_to_string(repo().join("README.md")).unwrap();
    assert!(
        links(&body)
            .iter()
            .any(|l| matches!(target(l), Target::Site(_))),
        "README should route readers to {SITE}"
    );
}

/// Every design page opens by saying whether it describes something built.
///
/// These pages are the project's memory: most record something that was
/// tried, what it cost, and what was decided. That makes them worth keeping
/// and also makes them dangerous, because a page describing a design reads
/// exactly like a page describing a system — and the difference is the whole
/// value of reading one.
///
/// `adoption.md` had opened `**Status: built.**` for a while and nothing else
/// did, in three different spellings between them. A convention the build does
/// not check is one that decays: the pages that drift are the ones nobody is
/// editing, which are also the ones most likely to be describing something
/// that changed underneath them.
///
/// The rule is deliberately weak about *content* and strict about
/// *presence*. It cannot tell whether "built" is true — nothing can, from
/// here — only that somebody was made to answer the question when they added
/// the page.
#[test]
fn every_design_page_states_its_status() {
    let dir = repo().join("docs/design");
    let mut silent = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        // Near the top, or it is not a status — a sentence about state
        // buried on line 200 is prose, and the reader who needed it has
        // already believed the page.
        let stated = body.lines().take(10).any(|l| {
            l.trim_start()
                .trim_start_matches("> ")
                .starts_with("**Status:")
        });
        if !stated {
            silent.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    silent.sort();
    assert!(
        silent.is_empty(),
        "these design pages do not say whether they describe something built, \
         so a reader cannot tell a design from a system:\n  {}\n\nOpen with a \
         line like `**Status: built.**` or `**Status: designed, not wired.**` \
         in the first ten lines.",
        silent.join("\n  ")
    );
}

/// The anchors these guards compute are the ones the site serves.
///
/// Each pair below is a heading as a page wrote it and the id a build of the
/// site gave it. The last two are why this is a test: a heading that holds a
/// code span inside a code span, or opens with a padded one, was slugged here
/// by deleting its backticks, which leaves the padding behind as hyphens and
/// keeps a space the renderer had already taken away. The very last was
/// malformed, and has been rewritten on its page since; it stays here as the
/// shape that was measured.
#[test]
fn a_heading_is_slugged_the_way_the_site_slugs_it() {
    for (heading, served) in [
        ("## Requirements", "requirements"),
        ("## `omh doctor`", "omh-doctor"),
        ("## `carry_in`", "carry_in"),
        ("## `omh memory …`", "omh-memory-"),
        (
            "### Codex says \"Login using `codex login` and then run this command again\"",
            "codex-says-login-using-codex-login-and-then-run-this-command-again",
        ),
        (
            "### `` `graph-refresh` is a name omh ships ``",
            "graph-refresh-is-a-name-omh-ships",
        ),
        (
            "### `a hook does nothing without `run` or `inject``",
            "a-hook-does-nothing-without-runorinject",
        ),
    ] {
        assert_eq!(slug(heading), served, "for the heading {heading}");
    }
}

/// A line of prose can open with a `#` — a wrapped sentence that reaches a
/// pull request's number at the line break does. Markdown makes a heading of
/// one to six of them and then a space, and of nothing else: the roadmap had
/// such a line, the site gave it no id, and this file offered one.
#[test]
fn a_line_that_opens_with_a_hash_is_not_always_a_heading() {
    let page =
        "## A heading\n\nclosed in\n#53–67 and the rest of the sentence\n\n####### seven\n#tag\n";
    assert_eq!(
        anchors(page),
        BTreeSet::from(["a-heading".to_string()]),
        "only the first of these lines is a heading"
    );
}

/// What the path guard lets through is a link pinned to a whole commit id,
/// and only that.
#[test]
fn only_a_link_pinned_to_a_commit_may_name_a_path_that_is_gone() {
    let pinned = format!(
        "{REPOSITORY}/blob/{}/docs/design/gone.md#then",
        "a".repeat(40)
    );
    for (line, kept) in [
        (
            format!("see [the design]({pinned}) for it"),
            "see [the design]() for it".to_string(),
        ),
        (
            format!("{REPOSITORY}/blob/main/docs/design/gone.md"),
            format!("{REPOSITORY}/blob/main/docs/design/gone.md"),
        ),
        (
            format!("{REPOSITORY}/blob/abc1234/docs/design/gone.md"),
            format!("{REPOSITORY}/blob/abc1234/docs/design/gone.md"),
        ),
        (
            "`docs/design/gone.md` said so".to_string(),
            "`docs/design/gone.md` said so".to_string(),
        ),
    ] {
        assert_eq!(without_pinned_links(&line), kept, "for the line {line}");
    }
}
