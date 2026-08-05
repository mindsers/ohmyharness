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

/// Installed into the base image rather than a harness layer: a code graph is
/// harness-agnostic, and every session should get the same one.
pub const GRAPH_INSTALL: &str = "npm install -g codebase-memory-mcp";

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
}
