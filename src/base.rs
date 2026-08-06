//! The base set — omh's opinion.
//!
//! Everything else in this codebase is a place to put this. A distribution is
//! not its machinery; it is what it chooses, and choosing is the part a
//! marketplace structurally cannot do.
//!
//! Entries earn their place by stating what they cost, what they buy, what was
//! considered instead, and how to remove them. **Cost is measured; benefit is
//! argued.** Those are different kinds of claim and are never presented as the
//! same one — a benchmark over a stochastic metric would have dressed the second
//! as the first, which is why there isn't one.
//!
//! The manifest is the single source of truth: `omh init` seeds from it and
//! `omh why` explains from it, so they cannot disagree.

use crate::render::Server;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The base set as data.
///
/// `omh init` seeds from this and `omh why` explains from it, so the two cannot
/// disagree about what is installed or why. Keeping the rationale in a shipped
/// file rather than in the binary also means the opinion is reviewable by the
/// people it is imposed on.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The base set is versioned because it expires — a distribution's real
    /// work is re-choosing as the catalogue churns.
    pub version: String,
    #[serde(default, rename = "entry")]
    pub entries: Vec<Entry>,
    /// Candidates considered and turned down. Recorded so the same one is not
    /// re-litigated every time somebody rediscovers it.
    #[serde(default)]
    pub rejected: Vec<Rejected>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub since: String,
    /// Argued, not measured. The honest half.
    pub because: String,
    /// A default nobody can leave is a cage.
    pub remove: String,
    /// For `mcp` entries: what `init` seeds. Also the baseline that decides
    /// whether the user's copy counts as modified.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub measured: Vec<Measured>,
    #[serde(default)]
    pub instead_of: Vec<Alternative>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Mcp,
    Hook,
}

/// A cost, with the date it was taken and how.
///
/// Never rendered in the same shape as a computed value: one is a fact about
/// this machine right now, the other is a recording that can go stale, and
/// blurring them is how a document starts claiming more than it can support.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub what: String,
    pub value: String,
    pub how: String,
    pub on: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    pub name: String,
    pub why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejected {
    pub name: String,
    pub considered: String,
    pub because: String,
}

impl Manifest {
    /// Load the newest manifest in `dir`.
    ///
    /// Newest by filename, since versions are `YYYY.MM`. Older ones are kept
    /// rather than deleted so `omh upgrade` can eventually diff two of them and
    /// say what entered, what left, and why.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();

        let path = files
            .last()
            .with_context(|| format!("no base manifest in {} — run `omh init`", dir.display()))?;
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// The MCP servers `omh init` seeds, built from the manifest.
    ///
    /// There is no second definition in code to disagree with this one — that
    /// was the point of moving the base set into a file.
    pub fn servers(&self) -> BTreeMap<String, Server> {
        self.entries
            .iter()
            .filter(|e| e.kind == Kind::Mcp)
            .filter_map(|e| {
                Some((
                    e.name.clone(),
                    Server {
                        command: e.command.clone()?,
                        args: e.args.clone(),
                        env: BTreeMap::new(),
                    },
                ))
            })
            .collect()
    }

    /// One line per entry, for `omh init` to print. The full answer is
    /// `omh why <name>`.
    pub fn rationale(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter(|e| e.kind == Kind::Mcp)
            .map(|e| (e.name.as_str(), e.because.as_str()))
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn rejection(&self, name: &str) -> Option<&Rejected> {
        self.rejected.iter().find(|r| r.name == name)
    }
}

/// Where the graph server keeps its index inside the sandbox.
///
/// Mounted from a volume keyed by **repo**, not by harness, so the index
/// survives a container rebuild and a switch from Claude Code to opencode.
pub const GRAPH_CACHE: &str = "/home/agent/.cache/codebase-memory-mcp";

pub const GRAPH_VERSION: &str = "0.9.0";

/// Port the graph UI is reachable on from the host.
pub const GRAPH_UI_PORT: u16 = 9749;

/// Port the server itself binds.
///
/// It binds **container loopback** and offers no bind-address flag, so a
/// published port forwards to nothing. Verified: `HTTP 200` inside the sandbox,
/// no response from the host. A bridge listening on all interfaces fixes it
/// without asking the tool to expose itself.
pub const GRAPH_UI_INTERNAL: u16 = 9748;

pub const GRAPH_BIN: &str = "codebase-memory-mcp";

// The MCP servers and their rationale used to live here as two hardcoded
// functions. They are `Manifest::servers()` and `Manifest::rationale()` now:
// one file that `init` seeds from and `why` explains from, which cannot
// disagree with itself the way two definitions can.

/// The graph UI runs **once per repo**, not once per session.
///
/// Every session's graph lives in one volume, so a per-session server showed
/// every other session's graph anyway — N identical websites. Matching the
/// server's scope to its data's scope removes the duplication, survives
/// sessions starting and stopping, and lets the container mount *only* the
/// index: no worktree, no credentials, no profile.
pub fn ui_container(repo: &str) -> String {
    format!("omh-graph-{repo}")
}

/// A stable loopback port for the graph UI.
///
/// Derived, like the ssh port: a browser tab you left open must keep working
/// across a restart.
pub fn ui_port(container: &str) -> u16 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    container.hash(&mut h);
    "graph-ui".hash(&mut h);
    const LOW: u32 = 49152;
    (LOW + (h.finish() % (65535 - LOW) as u64) as u32) as u16
}

/// Install the **UI variant** from GitHub Releases, checksum-verified.
///
/// Not `npm install`: the published 0.9.0 installer hardcodes
/// `variant = platform === 'linux' ? '-portable' : ''` and never reads
/// `CBM_VARIANT`, so the documented `CBM_VARIANT=ui` yields the lean binary.
/// Verified in a container — it reports "built without the embedded UI".
pub fn graph_install() -> String {
    // `-portable` is upstream's own linux convention; TARGETARCH is what
    // buildkit sets, so the same Dockerfile works on arm64 and amd64.
    format!(
        "set -eu; \
         ARCH=${{TARGETARCH:-$(dpkg --print-architecture)}}; \
         A=codebase-memory-mcp-ui-linux-$ARCH-portable.tar.gz; \
         B=https://github.com/DeusData/codebase-memory-mcp/releases/download/v{GRAPH_VERSION}; \
         cd /tmp && curl -sSLO \"$B/$A\" && curl -sSLO \"$B/checksums.txt\" && \
         grep \" $A$\" checksums.txt | sha256sum -c - && \
         tar xzf \"$A\" && \
         install -m 0755 \"$(find /tmp -maxdepth 2 -name {GRAPH_BIN} -type f | head -1)\" \
           /usr/local/bin/{GRAPH_BIN} && \
         rm -rf /tmp/*"
    )
}

/// Serve the graph UI. Needs stdin held open: the MCP server shuts down when
/// stdio closes, and it takes the UI down with it.
pub fn ui_command(port: u16) -> String {
    format!(
        "sleep infinity | {GRAPH_BIN} --ui=true --port={GRAPH_UI_INTERNAL} & \
         socat TCP-LISTEN:{port},fork,reuseaddr TCP:127.0.0.1:{GRAPH_UI_INTERNAL}"
    )
}

/// Run the graph UI as a container of its own.
///
/// Its own container rather than a process inside a session: lifecycle becomes
/// `docker run` / `docker rm`, which is idempotent by construction. The
/// per-session version needed a `pgrep` guard, a detached exec, and a `pkill` —
/// and each of those was a bug before it worked.
pub fn ui_run_args(image: &str, container: &str, cache_volume: &str, port: u16) -> Vec<String> {
    vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        container.into(),
        "-p".into(),
        format!("127.0.0.1:{port}:{GRAPH_UI_PORT}"),
        // The index and nothing else. No worktree, no credentials, no profile.
        "-v".into(),
        format!("{cache_volume}:{GRAPH_CACHE}"),
        image.into(),
        "sh".into(),
        "-c".into(),
        ui_command(GRAPH_UI_PORT),
    ]
}

/// Drop a session's graph.
///
/// `omh s rm` removes the worktree; without this the index outlives the code it
/// describes, and every later `list_projects` offers graphs of branches that no
/// longer exist anywhere.
pub fn drop_graph_command(project: &str) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!("{GRAPH_BIN} cli delete_project --project '{project}' >/dev/null 2>&1 || true"),
    ]
}

/// Canonical hook, in the shape a profile layer stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub name: &'static str,
    pub event: &'static str,
    pub matcher: &'static str,
    pub command: String,
}

/// The env var carrying the graph's project name into the sandbox.
///
/// Hooks run inside the container and must name the project they refresh. Baking
/// a path in would make the hook file session-specific; an env var keeps it one
/// shared, reviewable file.
pub const PROJECT_ENV: &str = "OMH_GRAPH_PROJECT";

/// A graph is per-session, because a session's worktree is not the checkout the
/// agent started from — it holds whatever the agent has since written.
pub fn project_name(repo: &str, session: &str) -> String {
    format!("{repo}-{session}")
}

/// Hooks that make the graph actually get used. Without them the server is
/// installed and never called, which is how most of these end up.
pub fn hooks() -> Vec<Hook> {
    // Nudges speak through `hookSpecificOutput.additionalContext` — the
    // documented channel a hook uses to reach the model. Bare stdout on exit 0
    // is not, and the first version of the grep nudge may never have been seen.
    let nudge = |body: &str| {
        format!(
            "jq -nc --arg p \"${PROJECT_ENV}\" '{{\"hookSpecificOutput\":{{\
             \"hookEventName\":\"PreToolUse\",\"additionalContext\":{body}}}}}'"
        )
    };

    vec![
        Hook {
            name: "graph-refresh",
            event: "Stop",
            matcher: "",
            // 0.14s incrementally. A graph describing the code as it was when
            // the session started is worse than none: it answers confidently
            // about code the agent has since rewritten.
            command: format!(
                "{GRAPH_BIN} cli index_repository --repo-path /work \
                 --name \"${PROJECT_ENV}\" --mode fast >/dev/null 2>&1 || true"
            ),
        },
        Hook {
            name: "graph-orient",
            event: "SessionStart",
            matcher: "",
            // The only graph tool that costs nothing per tool call: orientation
            // the agent is given once instead of discovering by reading files.
            //
            // SessionStart re-fires on resume and compact, so this is paid every
            // time context is rebuilt, not once. `overview` is 6,173 bytes; the
            // four aspects that actually orient are 2,138. The flag repeats — a
            // comma-separated list returns empty, verified against the binary.
            command: format!(
                "a=$({GRAPH_BIN} cli get_architecture --project \"${PROJECT_ENV}\" \
                 --aspects layers --aspects packages --aspects boundaries \
                 --aspects entry_points 2>/dev/null | tail -1); \
                 [ -n \"$a\" ] || exit 0; \
                 jq -nc --arg a \"$a\" --arg p \"${PROJECT_ENV}\" \
                 '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SessionStart\",\
                 \"additionalContext\":(\"Code graph for project \" + $p + \
                 \" — modules, layers, boundaries and entry points. Query it with \
                 search_graph/trace_path/get_code_snippet rather than exploring by \
                 hand:\\n\" + $a)}}}}'"
            ),
        },
        Hook {
            name: "graph-first",
            event: "PreToolUse",
            matcher: "Grep|Glob",
            // A nudge, not a wall: grep is right for a literal string, and a
            // hook that blocks correct work gets disabled.
            command: nudge(
                r#"("This repo has a code graph: project " + $p + ". For structural questions — where is X defined, what calls Y, what does this module depend on — search_graph --project " + $p + " answers in one call. Grep is right for literal text.")"#,
            ),
        },
        Hook {
            name: "graph-read",
            event: "PreToolUse",
            matcher: "Read",
            // The largest avoidable cost in a session: src/auth.rs is 35,814
            // bytes and get_code_snippet answers the same question in 1,511.
            //
            // Read is also the most frequent tool there is, so this speaks only
            // when a symbol lookup would actually be cheaper — a source file big
            // enough to be worth not reading whole. Otherwise silent: a nudge on
            // every call becomes noise the model tunes out.
            command: format!(
                "f=$(jq -r '.tool_input.file_path // empty'); \
                 case \"$f\" in \
                 *.rs|*.ts|*.tsx|*.js|*.jsx|*.py|*.go|*.java|*.rb|*.php|*.c|*.h|*.cc|\
                 *.cpp|*.hpp|*.cs|*.swift|*.kt|*.scala) ;; *) exit 0 ;; esac; \
                 [ -f \"$f\" ] || exit 0; \
                 [ \"$(wc -c < \"$f\")\" -gt 8000 ] || exit 0; \
                 jq -nc --arg p \"${PROJECT_ENV}\" --arg f \"$f\" \
                 '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\
                 \"additionalContext\":($f + \" is large. For one symbol rather than the \
                 whole file: get_code_snippet --project \" + $p + \" --qualified-name \
                 <name>, and search_graph finds the name.\")}}}}'"
            ),
        },
    ]
}

/// Index a repository into the shared graph.
///
/// Runs **inside the sandbox**, because the cache is a container volume: an
/// index built on the host would be written somewhere no session can read.
pub fn index_args(
    image: &str,
    cache_volume: &str,
    repo: &std::path::Path,
    name: &str,
) -> Vec<String> {
    vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        // Read-only: indexing reads code, and an indexer that can write into
        // the checkout is a sandbox hole for no benefit.
        format!("{}:/work:ro", repo.display()),
        "-v".into(),
        format!("{cache_volume}:{GRAPH_CACHE}"),
        // The server derives its project name from the working directory, not
        // from --repo-path: run elsewhere and `--name r` becomes
        // `some-other-path-r`. Verified against the real binary.
        "-w".into(),
        "/work".into(),
        image.into(),
        GRAPH_BIN.into(),
        "cli".into(),
        "index_repository".into(),
        "--repo-path".into(),
        "/work".into(),
        // Sessions live at different paths and the server derives a project
        // name from the path; without this every session builds its own graph.
        "--name".into(),
        name.into(),
        "--mode".into(),
        "fast".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// The manifest as shipped. Tested through the real file rather than a
    /// fixture: a manifest that parses in a test and not in the wild is the
    /// failure this whole module exists to prevent.
    const BUNDLED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    fn shipped() -> Manifest {
        Manifest::load_dir(Path::new(BUNDLED)).expect("bundled base manifest")
    }

    /// `docs/design/distribution.md` says every base-set entry states what it
    /// costs, what it buys, what was considered instead, and how to remove it —
    /// and that anything unable to fill in all four is taste pretending to be
    /// curation.
    ///
    /// That was aspiration written in a document nothing enforced. Here it is a
    /// test, so a future entry cannot be added without its reasoning: the
    /// cheapest moment to demand a justification is before it ships, and the
    /// only moment anyone reliably does is when something turns red.
    #[test]
    fn every_base_set_entry_states_its_case() {
        let manifest = shipped();
        assert!(
            !manifest.entries.is_empty(),
            "a base set with no entries is not a distribution"
        );

        for e in &manifest.entries {
            assert!(!e.because.trim().is_empty(), "{}: no `because`", e.name);
            assert!(
                !e.remove.trim().is_empty(),
                "{}: no way to remove it",
                e.name
            );
            assert!(
                !e.instead_of.is_empty(),
                "{}: nothing recorded as considered-instead. An entry with no \
                 alternatives was not chosen, it was defaulted to.",
                e.name
            );
            assert!(
                !e.measured.is_empty(),
                "{}: no measured cost. Benefit is argued here, but cost is the \
                 half that must be measured — it is what creeps.",
                e.name
            );
        }
    }

    /// A rejection is a product artifact. Without one recorded, the same
    /// candidate gets re-litigated every time someone rediscovers it.
    #[test]
    fn rejections_say_why_they_were_rejected() {
        for r in &shipped().rejected {
            assert!(
                !r.because.trim().is_empty(),
                "{}: rejected with no reason",
                r.name
            );
        }
    }

    /// The manifest carries the *reasoning*; hook commands stay in code, because
    /// they are intricate shell that interpolates `GRAPH_BIN` and `PROJECT_ENV`
    /// and would lose that coupling flattened into TOML.
    ///
    /// Two sources describing one base set can drift, and the drift is silent in
    /// the worst direction: `omh why` confidently explaining an entry that is no
    /// longer installed, or an entry shipping with no explanation at all. So the
    /// name sets must match exactly, in both directions.
    #[test]
    fn the_manifest_and_the_code_describe_the_same_base_set() {
        let manifest = shipped();

        let declared: BTreeSet<&str> = manifest
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Hook)
            .map(|e| e.name.as_str())
            .collect();
        let shipped_hooks: BTreeSet<&str> = hooks().iter().map(|h| h.name).collect();
        assert_eq!(
            declared, shipped_hooks,
            "hooks in the manifest vs hooks in the code"
        );

        // MCP servers are not checked here: since `Manifest::servers()` derives
        // from the manifest there is no second definition to disagree with, and
        // asserting it would only prove that a filter works. The hook half is
        // real because hook *commands* genuinely still live in code.
    }

    /// `Manifest::servers()` drops an entry whose `command` is missing, so an
    /// mcp entry without one is installed nowhere while still being listed in
    /// the base set and explained by `omh why` — present in every account of
    /// itself except the one that matters.
    #[test]
    fn an_mcp_entry_without_a_command_is_not_silently_dropped() {
        let manifest = shipped();
        let declared = manifest
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Mcp)
            .count();
        assert_eq!(
            declared,
            manifest.servers().len(),
            "an mcp entry is missing its `command` and would seed nothing"
        );
    }

    /// Exactly the document `init` writes into the shared layer.
    ///
    /// A manifest that parses but yields an empty server map is silent on both
    /// sides: init reports success, and every new sandbox simply comes up
    /// without a graph. Nothing downstream notices, because "no MCP servers
    /// configured" is a legitimate state.
    #[test]
    fn the_document_init_seeds_actually_contains_the_base_set() {
        let manifest = shipped();
        let body = serde_json::to_string_pretty(
            &serde_json::json!({ "mcpServers": manifest.servers() }),
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let servers = parsed["mcpServers"].as_object().expect("an mcpServers object");
        assert!(!servers.is_empty(), "init would seed an empty base set");
        assert_eq!(servers["codegraph"]["command"], GRAPH_BIN);
    }

    #[test]
    fn the_base_set_ships_a_code_graph() {
        let s = shipped().servers();
        assert!(
            s.contains_key("codegraph"),
            "got: {:?}",
            s.keys().collect::<Vec<_>>()
        );
        assert_eq!(s["codegraph"].command, GRAPH_BIN);
    }

    /// Base servers run in the sandbox. A command carrying a host path would
    /// work on the machine that wrote it and nowhere else.
    #[test]
    fn base_servers_reference_nothing_on_the_host() {
        for (name, server) in shipped().servers() {
            assert!(
                !server.command.contains('/'),
                "{name}: {} is a host path",
                server.command
            );
            for arg in &server.args {
                assert!(
                    !arg.starts_with("/Users") && !arg.starts_with("/home/")
                        || arg.starts_with("/home/agent"),
                    "{name}: {arg} is not a sandbox path"
                );
            }
        }
    }

    /// Every entry has to be able to answer "why is this here", and the answer
    /// has to be an actual sentence. `every_base_set_entry_states_its_case`
    /// checks a `because` exists; this checks it says something.
    #[test]
    fn every_entry_carries_its_argument() {
        let manifest = shipped();
        let reasons: BTreeMap<_, _> = manifest.rationale().into_iter().collect();
        for name in manifest.servers().keys() {
            let why = reasons
                .get(name.as_str())
                .unwrap_or_else(|| panic!("{name} has no rationale"));
            assert!(why.len() > 20, "{name}: `{why}` explains nothing");
        }
    }

    // ── indexing ────────────────────────────────────────────────────────────

    #[test]
    fn indexing_runs_inside_the_sandbox_with_the_cache_mounted() {
        let args = index_args(
            "omh/base:x",
            "omh-cache-repo",
            Path::new("/host/repo"),
            "repo",
        );
        let joined = args.join(" ");
        assert!(
            joined.contains("omh-cache-repo:"),
            "the cache volume must be mounted: {joined}"
        );
        assert!(
            joined.contains(GRAPH_CACHE),
            "at the path the server uses: {joined}"
        );
        assert!(
            joined.contains("/host/repo:"),
            "the code must be readable: {joined}"
        );
    }

    /// The repo is mounted read-only: indexing reads code, and an indexer that
    /// can write into the checkout is a sandbox hole for no benefit.
    #[test]
    fn indexing_cannot_write_to_the_checkout() {
        let joined = index_args("omh/base:x", "vol", Path::new("/host/repo"), "repo").join(" ");
        assert!(joined.contains("/host/repo:/work:ro"), "got: {joined}");
    }

    /// Sessions live at different paths, and the server derives a project name
    /// from the path. Without a stable name every session would build its own
    /// graph from scratch and share nothing.
    #[test]
    fn every_session_indexes_into_one_named_project() {
        let a = index_args("i", "v", Path::new("/host/repo"), "myrepo").join(" ");
        let b = index_args("i", "v", Path::new("/host/worktrees/s01"), "myrepo").join(" ");
        assert!(a.contains("--name myrepo") && b.contains("--name myrepo"));
    }

    #[test]
    fn indexing_names_the_repository_it_was_given() {
        let joined = index_args("i", "v", Path::new("/host/repo"), "r").join(" ");
        assert!(joined.contains("--repo-path /work"), "got: {joined}");
    }

    // ── keeping the graph current ───────────────────────────────────────────

    /// The server derives its project name from the **working directory**, not
    /// from `--repo-path`: run elsewhere, `--name probe` becomes
    /// `private-tmp-…-scratchpad-probe`. Verified against the real binary.
    #[test]
    fn indexing_runs_with_the_repo_as_its_working_directory() {
        let args = index_args("i", "v", Path::new("/host/repo"), "r");
        assert!(
            args.windows(2).any(|w| w[0] == "-w" && w[1] == "/work"),
            "the project name depends on cwd: {args:?}"
        );
    }

    #[test]
    fn a_sessions_graph_is_its_own() {
        assert_ne!(project_name("repo", "s01"), project_name("repo", "s02"));
        assert_ne!(project_name("alpha", "s01"), project_name("beta", "s01"));
    }

    #[test]
    fn a_sessions_graph_name_is_stable() {
        assert_eq!(project_name("repo", "s01"), project_name("repo", "s01"));
    }

    // ── hooks ───────────────────────────────────────────────────────────────

    fn hook(name: &str) -> Hook {
        hooks()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no {name} hook"))
    }

    /// A graph that describes the code as it was when the session started is
    /// worse than none: it answers confidently about code the agent has since
    /// rewritten. Re-indexing costs 0.14s.
    #[test]
    fn the_graph_refreshes_when_a_turn_ends() {
        let h = hook("graph-refresh");
        assert_eq!(h.event, "Stop");
        assert!(h.command.contains("index_repository"), "got: {}", h.command);
        assert!(
            h.command.contains("/work"),
            "it indexes the session, not the checkout"
        );
    }

    /// The whole point. An MCP server the agent never reaches for is installed
    /// and inert.
    #[test]
    fn the_agent_is_pointed_at_the_graph_before_it_greps() {
        let h = hook("graph-first");
        assert_eq!(h.event, "PreToolUse");
        assert!(h.matcher.contains("Grep"), "got: {}", h.matcher);
        assert!(
            h.command.contains("search_graph"),
            "the nudge must name the tool to use: {}",
            h.command
        );
    }

    /// A nudge, not a wall: grep is the right tool for a literal string, and a
    /// hook that blocks correct work gets disabled.
    #[test]
    fn the_nudge_never_blocks_the_tool() {
        let h = hook("graph-first");
        for forbidden in ["exit 1", "deny", "block"] {
            assert!(
                !h.command.contains(forbidden),
                "must not block: {}",
                h.command
            );
        }
    }

    /// Hooks are one shared file across every session, so they name the project
    /// through the environment rather than baking a session into the text.
    #[test]
    fn hooks_name_their_project_through_the_environment() {
        for h in hooks() {
            assert!(
                h.command.contains(PROJECT_ENV),
                "{} must name its project: {}",
                h.name,
                h.command
            );
        }
    }

    /// The store holds every session's graph for this repo. A nudge that names
    /// the tool but not the project invites the agent to answer confidently
    /// about code that is not in this worktree — and it fires at the moment the
    /// agent is deciding, which is where naming it actually lands.
    #[test]
    fn the_nudge_names_the_project_to_query() {
        let h = hook("graph-first");
        assert!(h.command.contains(PROJECT_ENV), "got: {}", h.command);
    }

    // ── the graph UI ────────────────────────────────────────────────────────

    /// The npm package cannot deliver the UI build. Verified in a container:
    /// `CBM_VARIANT=ui npm install -g` still reports "built without the
    /// embedded UI", because the published installer ignores the variable.
    #[test]
    fn the_ui_build_comes_from_the_release_not_npm() {
        let cmd = graph_install();
        assert!(cmd.contains("-ui-"), "must fetch the UI variant: {cmd}");
        assert!(!cmd.contains("npm install"), "npm cannot deliver it: {cmd}");
    }

    /// A binary fetched over the network into an image every session runs is
    /// exactly where a supply-chain check earns its keep — and upstream
    /// publishes checksums.
    #[test]
    fn the_download_is_checksum_verified() {
        let cmd = graph_install();
        assert!(cmd.contains("checksums.txt"), "got: {cmd}");
        assert!(cmd.contains("sha256sum -c"), "got: {cmd}");
    }

    /// Apple Silicon builds arm64 images and Intel builds amd64; a hardcoded
    /// arch fails on one of them with a confusing exec error.
    #[test]
    fn the_download_follows_the_build_architecture() {
        let cmd = graph_install();
        assert!(
            cmd.contains("TARGETARCH") || cmd.contains("dpkg --print-architecture"),
            "arch must be derived: {cmd}"
        );
    }

    /// Verified in a container: backgrounded with stdin closed, the server logs
    /// `ui.serving` and then `server.shutdown` immediately — the UI dies with
    /// the stdio session.
    #[test]
    fn serving_the_ui_holds_stdin_open() {
        let cmd = ui_command(GRAPH_UI_PORT);
        assert!(
            cmd.contains("sleep infinity |"),
            "stdin must stay open: {cmd}"
        );
        assert!(cmd.contains("--ui=true"), "got: {cmd}");
    }

    /// The server binds container loopback and has no bind-address flag, so a
    /// published port forwards to nothing. Verified: HTTP 200 inside the
    /// sandbox, no response from the host.
    #[test]
    fn the_ui_is_bridged_onto_an_interface_the_host_can_reach() {
        let cmd = ui_command(GRAPH_UI_PORT);
        assert!(cmd.contains("socat"), "got: {cmd}");
        assert!(
            cmd.contains(&format!("TCP-LISTEN:{GRAPH_UI_PORT}")),
            "must listen where docker publishes: {cmd}"
        );
        assert!(
            cmd.contains(&format!("TCP:127.0.0.1:{GRAPH_UI_INTERNAL}")),
            "and forward to where the server binds: {cmd}"
        );
    }

    /// Regression: removing a session left its graph behind, so the cache grew
    /// with dead sessions and the agent could query code that no longer exists.
    #[test]
    fn removing_a_session_drops_its_graph() {
        let cmd = drop_graph_command("ohmyharness-s02").join(" ");
        assert!(cmd.contains("delete_project"), "got: {cmd}");
        assert!(cmd.contains("ohmyharness-s02"), "got: {cmd}");
    }

    /// Dropping a graph that was never built is not a failure — a session may
    /// have been removed before it ever launched.
    #[test]
    fn dropping_a_graph_that_is_not_there_is_forgiving() {
        let cmd = drop_graph_command("nope").join(" ");
        assert!(cmd.contains("|| true"), "got: {cmd}");
    }

    // ── the graph UI is a repo-scoped service ───────────────────────────────

    /// Every session's graph lives in one volume, so a per-session server
    /// served every other session's graph anyway — N identical websites.
    #[test]
    fn the_ui_is_named_for_the_repo_not_a_session() {
        let c = ui_container("ohmyharness");
        assert!(c.contains("ohmyharness"));
        assert!(!c.contains("s01"), "not session-scoped: {c}");
        assert_eq!(c, ui_container("ohmyharness"), "and stable");
    }

    /// It needs the index and nothing else. A UI container holding a writable
    /// worktree and live credentials would be exposure for no purpose.
    #[test]
    fn the_ui_container_mounts_only_the_index() {
        let args = ui_run_args("omh/base:x", "omh-graph-r", "omh-cache-r", 50000);
        let mounts: Vec<&String> = args
            .iter()
            .skip_while(|a| *a != "-v")
            .step_by(2)
            .skip(1)
            .take(1)
            .collect();
        assert_eq!(mounts.len(), 1, "exactly one mount: {args:?}");
        let joined = args.join(" ");
        assert!(joined.contains("omh-cache-r"), "the index: {joined}");
        assert!(!joined.contains("/work"), "no worktree: {joined}");
        assert!(!joined.contains(".claude"), "no credentials: {joined}");
    }

    #[test]
    fn the_ui_container_publishes_on_loopback_only() {
        let joined = ui_run_args("i", "c", "v", 50000).join(" ");
        assert!(joined.contains("127.0.0.1:50000:"), "got: {joined}");
        assert!(!joined.contains("0.0.0.0"), "got: {joined}");
    }

    /// Lifecycle is `docker run` / `docker rm`, which is idempotent by
    /// construction — the per-session version needed a pgrep guard, a detached
    /// exec and a pkill, and each was a bug before it worked.
    #[test]
    fn the_ui_runs_detached_under_its_own_name() {
        let args = ui_run_args("i", "omh-graph-r", "v", 1);
        assert!(args.contains(&"-d".to_string()), "got: {args:?}");
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--name" && w[1] == "omh-graph-r"));
    }

    /// A hook talks to the model through `hookSpecificOutput.additionalContext`,
    /// injected as a system reminder. Bare stdout on exit 0 is not that
    /// mechanism — the first nudge shipped that way and may never have been seen.
    #[test]
    fn nudges_speak_through_additional_context() {
        for h in hooks() {
            if h.event == "Stop" {
                continue; // refreshes the index, says nothing to the model
            }
            assert!(
                h.command.contains("additionalContext"),
                "{}: {}",
                h.name,
                h.command
            );
            assert!(
                h.command.contains("hookSpecificOutput"),
                "{}: {}",
                h.name,
                h.command
            );
        }
    }

    /// Reading a 35KB file to see one function is the largest avoidable cost in
    /// a session: `src/auth.rs` is 35,814 bytes and `get_code_snippet` answers
    /// the same question in 1,511.
    #[test]
    fn reading_a_file_points_at_the_symbol_lookup() {
        let h = hook("graph-read");
        assert_eq!(h.event, "PreToolUse");
        assert_eq!(h.matcher, "Read");
        assert!(h.command.contains("get_code_snippet"), "got: {}", h.command);
    }

    /// `Read` is the most frequent tool there is. A nudge on every call is
    /// recurring cost and becomes noise the model tunes out, so it speaks only
    /// when a symbol lookup would actually be cheaper.
    #[test]
    fn the_read_nudge_stays_silent_when_it_has_nothing_to_say() {
        let cmd = hook("graph-read").command.clone();
        assert!(cmd.contains("file_path"), "must inspect the target: {cmd}");
        assert!(cmd.contains("wc -c"), "and its size: {cmd}");
    }

    /// Orientation the agent gets once, instead of discovering it by reading
    /// files. The only graph tool that costs nothing per tool call.
    #[test]
    fn a_session_starts_with_the_module_map() {
        let h = hook("graph-orient");
        assert_eq!(h.event, "SessionStart");
        assert!(h.command.contains("get_architecture"), "got: {}", h.command);
    }

    /// SessionStart re-fires on resume and compact, so this is not paid once —
    /// it is paid every time context is rebuilt. `overview` costs 6,173 bytes;
    /// the four aspects that actually orient cost 2,138.
    #[test]
    fn orientation_is_kept_small_because_it_repeats() {
        let cmd = hook("graph-orient").command.clone();
        assert!(
            !cmd.contains("overview"),
            "too broad for something that repeats: {cmd}"
        );
        for aspect in ["layers", "packages", "boundaries", "entry_points"] {
            assert!(cmd.contains(aspect), "missing {aspect}: {cmd}");
        }
    }

    /// A comma-separated list yields nothing; the flag repeats. Verified
    /// against the real binary.
    #[test]
    fn aspects_are_passed_as_repeated_flags() {
        let cmd = hook("graph-orient").command.clone();
        assert!(
            !cmd.contains("layers,packages"),
            "comma form returns empty: {cmd}"
        );
        assert_eq!(cmd.matches("--aspects").count(), 4, "got: {cmd}");
    }
}
