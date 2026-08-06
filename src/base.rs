//! The base set — omh's opinion.
//!
//! Everything else in this codebase is a place to put this. A distribution is
//! not its machinery; it is what it chooses, and choosing is the part a
//! marketplace structurally cannot do.
//!
//! Entries earn their place by measurement, not taste. Until `omh bench`
//! exists, each one carries the argument that put it here and should be read as
//! provisional.

use crate::render::Server;
use std::collections::BTreeMap;

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

/// MCP servers every project gets.
pub fn servers() -> BTreeMap<String, Server> {
    BTreeMap::from([(
        "codegraph".to_string(),
        Server {
            command: GRAPH_BIN.to_string(),
            // No arguments: with no args the binary is an MCP server on stdio,
            // and it finds its index in the cache omh mounts.
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    )])
}

/// One line per entry, explaining why it is here. `omh init` prints these, and
/// `omh why` will read them — a default nobody can interrogate is
/// indistinguishable from an arbitrary one.
pub fn rationale() -> Vec<(&'static str, &'static str)> {
    vec![(
        "codegraph",
        "structural queries instead of re-grepping the repo every task; \
         MIT, a static binary with no runtime or database. Provisional until \
         `omh bench` measures it.",
    )]
}

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
            name: "graph-first",
            event: "PreToolUse",
            matcher: "Grep|Glob",
            // A nudge, not a wall. Grep is the right tool for a literal string,
            // and a hook that blocks correct work gets disabled.
            // Names the project, because the store holds every session's graph
            // for this repo and querying the wrong one answers confidently
            // about code that is not in this worktree. The hook fires when the
            // agent is deciding, which is where that lands.
            command: format!(
                "echo \"This repo has a code graph: project ${PROJECT_ENV}. \
                 For 'where is X defined', 'what calls Y', or 'what does this \
                 module depend on', search_graph --project ${PROJECT_ENV} answers \
                 in one call. Grep is right for literal text.\""
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
    use std::path::Path;

    #[test]
    fn the_base_set_ships_a_code_graph() {
        let s = servers();
        assert!(s.contains_key("codegraph"), "got: {:?}", s.keys().collect::<Vec<_>>());
        assert_eq!(s["codegraph"].command, GRAPH_BIN);
    }

    /// Base servers run in the sandbox. A command carrying a host path would
    /// work on the machine that wrote it and nowhere else.
    #[test]
    fn base_servers_reference_nothing_on_the_host() {
        for (name, server) in servers() {
            assert!(!server.command.contains('/'), "{name}: {} is a host path", server.command);
            for arg in &server.args {
                assert!(
                    !arg.starts_with("/Users") && !arg.starts_with("/home/") || arg.starts_with("/home/agent"),
                    "{name}: {arg} is not a sandbox path"
                );
            }
        }
    }

    /// Every entry has to be able to answer "why is this here".
    #[test]
    fn every_entry_carries_its_argument() {
        let reasons: BTreeMap<_, _> = rationale().into_iter().collect();
        for name in servers().keys() {
            let why = reasons.get(name.as_str()).unwrap_or_else(|| panic!("{name} has no rationale"));
            assert!(why.len() > 20, "{name}: `{why}` explains nothing");
        }
    }

    // ── indexing ────────────────────────────────────────────────────────────

    #[test]
    fn indexing_runs_inside_the_sandbox_with_the_cache_mounted() {
        let args = index_args("omh/base:x", "omh-cache-repo", Path::new("/host/repo"), "repo");
        let joined = args.join(" ");
        assert!(joined.contains("omh-cache-repo:"), "the cache volume must be mounted: {joined}");
        assert!(joined.contains(GRAPH_CACHE), "at the path the server uses: {joined}");
        assert!(joined.contains("/host/repo:"), "the code must be readable: {joined}");
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
        hooks().into_iter().find(|h| h.name == name).unwrap_or_else(|| panic!("no {name} hook"))
    }

    /// A graph that describes the code as it was when the session started is
    /// worse than none: it answers confidently about code the agent has since
    /// rewritten. Re-indexing costs 0.14s.
    #[test]
    fn the_graph_refreshes_when_a_turn_ends() {
        let h = hook("graph-refresh");
        assert_eq!(h.event, "Stop");
        assert!(h.command.contains("index_repository"), "got: {}", h.command);
        assert!(h.command.contains("/work"), "it indexes the session, not the checkout");
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
            assert!(!h.command.contains(forbidden), "must not block: {}", h.command);
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
        assert!(cmd.contains("TARGETARCH") || cmd.contains("dpkg --print-architecture"),
            "arch must be derived: {cmd}");
    }

    /// Verified in a container: backgrounded with stdin closed, the server logs
    /// `ui.serving` and then `server.shutdown` immediately — the UI dies with
    /// the stdio session.
    #[test]
    fn serving_the_ui_holds_stdin_open() {
        let cmd = ui_command(GRAPH_UI_PORT);
        assert!(cmd.contains("sleep infinity |"), "stdin must stay open: {cmd}");
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
        let mounts: Vec<&String> =
            args.iter().skip_while(|a| *a != "-v").step_by(2).skip(1).take(1).collect();
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
        assert!(args.windows(2).any(|w| w[0] == "--name" && w[1] == "omh-graph-r"));
    }
}
