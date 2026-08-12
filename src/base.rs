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
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    /// Where this was loaded from. Not part of the file — set by `load_dir`, so
    /// every answer can name the manifest that produced it.
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// What this entry is part of. A server, its hooks and its section of the
    /// rules are one thing, and this is the field that says so — `[omh]` takes
    /// feature names, so an entry belonging to nothing cannot be switched off.
    ///
    /// Required, like `because` and `since`: the grouping spent its life as a
    /// comment header, which is the one claim in the manifest no test could
    /// check.
    pub feature: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Mcp,
    Hook,
    /// A section of the rules the agent is given. Ships as a base-set entry
    /// like everything else omh chooses — the prose an agent is handed costs
    /// context on every turn, and a cost nobody wrote down is one nobody can
    /// argue with.
    Rules,
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

/// `YYYY.MM` or `YYYY-MM-DD` → (year, month). One parser, so a date that the
/// staleness check cannot read is the same date the curation test rejects at
/// load — rather than one silently tolerating what the other would refuse.
pub fn parse_ym(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(['.', '-']);
    let year: u32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    (year >= 2000 && (1..=12).contains(&month)).then_some((year, month))
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
    /// Load the newest manifest in `dir`, newest by **parsed version**.
    ///
    /// Not by filename sort. That was three silent wrong answers at once: any
    /// stray `.toml` sorting after the real one became the base set, `2027.2`
    /// beat `2027.10`, and nothing checked a file's declared `version` at all.
    /// One stray file made `omh init` seed `{}` and report success, and made
    /// `omh why` call omh's own entries the user's.
    ///
    /// Older manifests are kept rather than deleted, so `omh upgrade` can
    /// eventually diff two and say what entered, what left, and why.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut newest: Option<((u32, u32), PathBuf, Self)> = None;

        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if !path.extension().is_some_and(|x| x == "toml") {
                continue;
            }
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            // A manifest an older omh seeded can be missing a field this one
            // requires, and every command loads the manifest — so the failure
            // is the whole tool, not one command. `init` refreshes bundled
            // files and keeps the old one, so saying that here is a way out
            // rather than the advice-that-does-nothing loop `read_layer`
            // documents.
            let manifest: Self = toml::from_str(&raw).with_context(|| {
                format!(
                    "parsing {} — if it was seeded by an older omh, `omh init` refreshes it",
                    path.display()
                )
            })?;

            // A file whose declared version is unreadable is not a candidate.
            // Accepting one on filename alone is what let `zz-notes.toml` win.
            let Some(version) = parse_ym(&manifest.version) else {
                continue;
            };
            if newest.as_ref().is_none_or(|(best, _, _)| version > *best) {
                newest = Some((version, path, manifest));
            }
        }

        let (_, path, mut manifest) = newest.with_context(|| {
            format!(
                "no usable base manifest in {} — run `omh init`",
                dir.display()
            )
        })?;

        // A manifest that parses but names nothing seeds an empty base set and
        // reports success, leaving every session running hooks that point at a
        // server which is not installed. Fail here rather than there.
        if manifest.entries.is_empty() {
            anyhow::bail!("{} declares no base-set entries", path.display());
        }
        manifest.path = Some(path);
        Ok(manifest)
    }

    /// Which manifest answered, and at what version.
    ///
    /// Four separate wrong answers reduced to `omh why` never saying this.
    pub fn source(&self) -> String {
        match &self.path {
            Some(p) => format!("{} · {}", p.display(), self.version),
            None => format!("(unsaved) · {}", self.version),
        }
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
/// Const concatenation of a `&str` const is not available without a macro
/// crate, so this repeats the home rather than deriving it — and
/// `the_graph_cache_lives_under_the_agents_home` fails if the two drift.
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

// The MCP servers and their rationale are not here: they are
// `Manifest::servers()` and `Manifest::rationale()`, read from the base-set
// file. One file that `init` seeds from and `why` explains from cannot
// contradict itself; a hardcoded list beside it can.

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

/// The grep nudge, in the three literal pieces `$p` is spliced between.
///
/// Kept as data rather than one string so its cost can be **computed** instead
/// of typed into the manifest. The manifest declared `~40 B` for this for its
/// whole life; the real figure is over five times that, and nothing could
/// notice, because a hand-written number and the string it describes had no
/// relationship a test could check.
const GREP_NUDGE: [&str; 3] = [
    "This repo has a code graph: project ",
    ". For structural questions — where is X defined, what calls Y, what does \
     this module depend on — search_graph --project ",
    " answers in one call. Grep is right for literal text.",
];

/// What the nudge actually injects, for a given project name. This is the thing
/// the cost in the manifest is a claim about.
/// Test-only: the hook builds its jq expression from `GREP_NUDGE` directly,
/// since `$p` is interpolated by jq at run time rather than by Rust. This is
/// the same string in the form a test can measure.
#[cfg(test)]
pub fn grep_nudge(project: &str) -> String {
    format!(
        "{}{project}{}{project}{}",
        GREP_NUDGE[0], GREP_NUDGE[1], GREP_NUDGE[2]
    )
}

/// Wrap a string for `sh`, so prose with punctuation cannot end the argument
/// it is inside. Single quotes, and the only character that matters inside them
/// is the single quote itself.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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
            name: "git-unavailable",
            event: "PreToolUse",
            matcher: "Bash",
            // Silent unless the command is actually git, for the reason
            // `graph-read` is silent on small files: a nudge on every Bash call
            // is noise the model tunes out, and Bash is most of what an agent
            // runs.
            //
            // git is matched anywhere a command can start, not just at the
            // front. `cd /work && git status` is the same mistake with a prefix,
            // and a **newline** is the separator that matters most — multi-line
            // Bash is one of the most common shapes an agent emits, and an
            // earlier version of this pattern missed every one of them. `[:blank:]`
            // rather than `[:space:]` for the leading-whitespace case, so the
            // newline arm stays the thing doing that work.
            //
            // Built from `GIT_ABSENT` so the sentence the agent meets here
            // and the one the `git-rules` section carries cannot drift.
            command: format!(
                "c=$(jq -r '.tool_input.command // empty'); \
                 case \"$c\" in \
                 git\\ *|git) ;; \
                 *[\\;\\&\\|\\(]*git\\ *|*[[:blank:]]git\\ *) ;; \
                 *) case \"$c\" in *\"\
                 \"git\\ *) ;; *) exit 0 ;; esac ;; esac; \
                 jq -nc --arg m {} '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\
                 \"additionalContext\":$m}}}}'",
                shell_quote(GIT_ABSENT)
            ),
        },
        Hook {
            name: "graph-first",
            event: "PreToolUse",
            matcher: "Grep|Glob",
            // A nudge, not a wall: grep is right for a literal string, and a
            // hook that blocks correct work gets disabled.
            // Built from GREP_NUDGE so the string the agent sees and the cost
            // the manifest claims cannot drift apart.
            command: nudge(&format!(
                r#"("{}" + $p + "{}" + $p + "{}")"#,
                GREP_NUDGE[0], GREP_NUDGE[1], GREP_NUDGE[2]
            )),
        },
        Hook {
            name: "graph-read",
            event: "PreToolUse",
            matcher: "Read",
            // The largest avoidable cost in a session: reading a whole module to
            // see one function, when get_code_snippet answers in ~1,500 bytes.
            // No file size named on purpose — the figure that used to be here
            // was stale on the commit that wrote it.
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

/// A section of the rules omh ships, in the shape a layer would store one.
///
/// The `name` is the manifest entry it answers to, exactly as a hook's is —
/// `the_manifest_and_the_code_describe_the_same_base_set` compares the two
/// name sets in both directions, so a section cannot ship unexplained and an
/// entry cannot explain a section nobody writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub body: String,
}

/// What the agent is told about git, in one place.
///
/// Both deliveries read from here — the `git-rules` section and the
/// `git-unavailable` hook — because two copies of a safety notice drift, and
/// the one that drifts is never the one you are reading.
///
/// Written as a claim about *this session*, not about git: an agent told "git
/// is broken" spends its turns trying to fix it, and the repair cannot work.
///
/// It also does no harm, which was checked rather than assumed — `git init`
/// against an unreachable gitdir refuses (git 2.55.0), naming the missing
/// directory, and leaves the pointer file exactly as it was. So this says the
/// attempt is futile, not that it is dangerous. The expensive failure here is
/// the agent promising a commit it cannot make.
pub const GIT_ABSENT: &str = "git does not work in this session, by design and not by fault. \
     The worktree's .git is a pointer at an admin directory on the host, which omh does not \
     mount — so every git command fails with `fatal: not a git repository`. Do not try to \
     repair it: `git init` refuses for the same reason, and re-cloning would only give you a \
     second repository nobody is reviewing. Nothing here is broken and nothing is lost. Your \
     work is already visible outside the sandbox, where the person you are working with \
     reviews it with `omh s diff`, commits it with `omh s commit`, and pushes it with \
     `omh s push`. Say that rather than offering to commit yourself.";

/// The rules omh ships, one section per base-set entry.
///
/// They live here rather than in the manifest for the reason hook commands do:
/// `memory-rules` interpolates `GUEST_LOCAL_NOTES` and `git-rules` reads
/// `GIT_ABSENT`, which the hook reads too. Flattened into TOML both couplings
/// become two strings that can drift, and the drift is silent — a safety notice
/// saying one thing in the rules and another in the hook.
///
/// They were prose `init` appended to `.omh/profile/AGENTS.md`, which meant they
/// reached the repos where somebody remembered and nowhere else, could not be
/// explained by `omh why`, and were invisible to the cost rollup.
pub fn sections() -> Vec<Section> {
    vec![
        Section {
            name: "graph-rules",
            body: "## Code graph\n\n\
                 This repo is indexed as a graph, refreshed after every turn. Prefer it over\n\
                 reading or grepping files when the question is structural:\n\n\
                 - `search_graph` — where is X defined, what is named like Y\n\
                 - `trace_path` — how does A reach B\n\
                 - `get_architecture` — what the modules are and how they depend on each other\n\
                 - `get_code_snippet` — read one symbol instead of a whole file\n\n\
                 Grep is still right for literal text: a string, a config value, a TODO.\n\n\
                 **Use the project named by `$OMH_GRAPH_PROJECT`.** Other sessions of this\n\
                 repo have their own graphs in the same store; querying one of those answers\n\
                 confidently about code that is not in this worktree.\n"
                .into(),
        },
        Section {
            name: "git-rules",
            // Orientation, where the hook is interception: a hook can only fire
            // once the agent has decided to run git, and by then it may already
            // have promised the user a commit. This is what stops the plan being
            // made.
            body: format!("## Git\n\n{GIT_ABSENT}\n"),
        },
        Section {
            name: "memory-rules",
            // "Which graph to ask" ships with memory rather than with the graph
            // because the decision it teaches is *when to reach for `recall`*,
            // and `recall` is what this feature introduces. With memory off the
            // agent should not be told to ask a tool it does not have; with the
            // graph off it loses a comparison, which is the cheaper of the two
            // wrong documents.
            body: format!(
                "## Which graph to ask\n\n\
                 There are two, and they do not overlap:\n\n\
                 - **the code graph** knows **what the code is** — where a symbol lives, how\n  \
                   one module reaches another. Re-derived from the code every turn, so it is\n  \
                   never out of date and never needs to be told anything.\n\
                 - **`recall`** knows **why** it is that way — what was tried and failed, what\n  \
                   turned out not to work, what surprised somebody. None of that is in the\n  \
                   code, so no amount of reading will recover it.\n\n\
                 A *where* or *what* question goes to the code graph. A *why*, *is this safe*,\n\
                 or *has this been tried* question goes to `recall`. When you are about to\n\
                 assume how something here behaves, ask `recall` first — that is exactly the\n\
                 assumption somebody already got wrong once.\n\n\
                 They compose: find the code with the code graph, then ask `recall` what is\n\
                 known about it before changing it.\n\n\
                 {}",
                note_taking()
            ),
        },
    ]
}

/// What the agent needs to write a note before there is a tool to write one.
///
/// This is the part of the surface that cannot move into a tool description:
/// *record what surprised you* is a trigger, and an agent cannot look up a rule
/// it does not know it needs. The note **shape** is here only because the
/// agent has no *tool* to write through yet — `remember` already enforces the
/// schema at the write, but nothing inside the sandbox can call it, so the
/// agent writes the file by hand and needs to be told the shape. When the MCP
/// surface lands, this shrinks back to the trigger.
///
/// The condition is the MCP surface, not `remember`'s existence: `remember`
/// exists, so a reader checking the wrong one deletes the staged shape while
/// the agent still has no way to reach it.
fn note_taking() -> String {
    format!(
        "## Memory\n\n\
         When something surprises you — you expected one thing and the repo did\n\
         another — record it. Not what you did; what you were wrong about.\n\n\
         Write a Markdown file into `{}/`, named after the\n\
         observation, in this shape:\n\n\
         ```markdown\n\
         ---\n\
         key: <the filename, without .md>\n\
         type: surprise\n\
         source: session $OMH_SESSION, <this harness>\n\
         recorded: <YYYY-MM-DD, the day it happened>\n\
         ---\n\n\
         # One line naming the surprise\n\n\
         ## Expected\n\n\
         ## Observed\n\n\
         ## Evidence\n\n\
         ## Answers\n\n\
         - <the question somebody would later ask to find this>\n\n\
         ## Related\n\n\
         - [[another-notes-key]]\n\
         ```\n\n\
         **Answers** is what makes the note findable later, and only you know it:\n\
         write the question you would have asked five minutes ago, in the words you\n\
         would have used. A note nobody can find is a note nobody wrote.\n\n\
         Store uncertainty rather than false precision, and date by when the thing\n\
         happened rather than when you mentioned it. If you have nothing to put\n\
         under **Expected**, there is nothing here worth recording.\n\n\
         Rename a note by rewriting its `key` and its filename together — never\n\
         one without the other.\n",
        crate::memory::GUEST_LOCAL_NOTES,
    )
}

/// What omh itself contributes to a session, once this repo has had its say.
///
/// Resolved from the manifest by the caller and handed to `container::plan`,
/// the rule `memory_bin` and `base` already follow: `plan` stays pure given a
/// temp filesystem, and a probe inside it is a probe no test can reach.
///
/// Empty is a legitimate value — every feature switched off — and is why this
/// is constructed rather than defaulted: a caller that forgets to resolve it
/// would otherwise silently ship a session none of omh's own material reaches.
#[derive(Debug, Clone, Default)]
pub struct Own {
    pub hooks: Vec<Hook>,
    pub sections: Vec<Section>,
    /// Servers to drop from the rendered document even though `mcp.json` still
    /// lists them. The feature is off *here*; nothing was uninstalled, and the
    /// file is left exactly as the user has it.
    pub disabled_servers: BTreeSet<String>,
    /// Every hook name the manifest owns, whether or not its feature is on.
    ///
    /// A file in a layer answering to one of these is never read. With the
    /// feature on the generated hook wins anyway; with it off, nothing runs —
    /// and it was the second case that shipped broken: the four graph hooks
    /// kept firing from files `init` seeded, against a server that had been
    /// taken out of the document. Disabling that leaves the disabled thing
    /// running is worse than not offering it.
    pub reserved: BTreeSet<String>,
}

/// Everything the manifest generates, minus the features this repo turned off.
///
/// A feature is all-or-nothing on purpose. `codegraph` on with `graph-refresh`
/// off is a graph that quietly stops tracking the code, which is the one
/// combination that manufactures confident wrong answers — so it is
/// unrepresentable rather than warned about.
///
/// Two ways a feature is off, and they are different acts:
///
/// - **switched off here**, by `[omh]`. Nothing is uninstalled.
/// - **removed**, by taking its server out of your profile. `remove` promises
///   that `omh config mcp rm codegraph` takes the hooks and the rules section
///   with it, and that command only edits `mcp.json` — so the promise is kept
///   here or nowhere. Before generation the hooks were files and removing the
///   server left four of them behind; generating them unconditionally would
///   have rebuilt that defect with no file left to delete.
///
/// `installed` is the servers the resolved profile declares. A feature with no
/// server of its own — `git-notice` — is unaffected by it.
///
/// Fails rather than filters when the binary ships a hook or a section this
/// manifest does not describe. That is omh disagreeing with itself, not a
/// preference somebody expressed, and the two were the same silent `false`:
/// the entry was not generated *and* `reserved` blocked any layer file from
/// standing in, so it existed nowhere and nothing said so.
pub fn own(
    manifest: &Manifest,
    off: &BTreeSet<String>,
    installed: &BTreeSet<String>,
) -> Result<Own> {
    // A feature keeps its non-server parts only while a server it owns is
    // still there. `any` rather than `all`: a feature with two servers and one
    // removed is a judgement nothing here can make, and keeping it is the
    // conservative half.
    let gone: BTreeSet<&str> = manifest
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Mcp)
        .fold(BTreeMap::<&str, bool>::new(), |mut acc, e| {
            let present = installed.contains(&e.name);
            *acc.entry(e.feature.as_str()).or_insert(false) |= present;
            acc
        })
        .into_iter()
        .filter(|(_, present)| !present)
        .map(|(feature, _)| feature)
        .collect();

    let on = |name: &str| -> Result<bool> {
        let entry = manifest.entry(name).with_context(|| {
            format!(
                "this omh ships `{name}` and {} describes no entry for it — the                  binary and the manifest disagree about the base set.                  `omh init` refreshes the bundled manifest.",
                manifest.source()
            )
        })?;
        Ok(!off.contains(&entry.feature) && !gone.contains(entry.feature.as_str()))
    };

    let mut own = Own {
        hooks: Vec::new(),
        sections: Vec::new(),
        disabled_servers: manifest
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Mcp && off.contains(&e.feature))
            .map(|e| e.name.clone())
            .collect(),
        // Every hook the manifest owns, on or off — which is why this is built
        // from the manifest rather than from `hooks()`. A file answering to one
        // of these is never read, and with the feature off there would be
        // nothing to override it with.
        reserved: manifest
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Hook)
            .map(|e| e.name.clone())
            .collect(),
    };
    for hook in hooks() {
        if on(hook.name)? {
            own.hooks.push(hook);
        }
    }
    for section in sections() {
        if on(section.name)? {
            own.sections.push(section);
        }
    }
    Ok(own)
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

    /// `git log --reverse --date=short | head -1`. Nothing in this repo could
    /// have been measured before it existed.
    const FIRST_COMMIT: (u32, u32, u32) = (2026, 8, 5);

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
            assert!(!e.since.trim().is_empty(), "{}: no `since`", e.name);

            for m in &e.measured {
                for (field, value) in [
                    ("what", &m.what),
                    ("value", &m.value),
                    ("how", &m.how),
                    ("on", &m.on),
                ] {
                    assert!(
                        !value.trim().is_empty(),
                        "{}: measured `{field}` is blank",
                        e.name
                    );
                }
                // A date the tool cannot read is a manifest defect, not a
                // measurement. Left unchecked it silently disables staleness
                // for that cost and prints itself to the user verbatim.
                parse_ym(&m.on).unwrap_or_else(|| panic!("{}: `{}` is not a date", e.name, m.on));

                // Day precision, not month. Every `on` in this manifest once
                // read 2026-08-04 — one day before this repository's first
                // commit, so no measurement of this repo could have been taken
                // then. A month-granular check passes that date happily, which
                // is how the first version of this very assertion failed to
                // catch the thing it was written for.
                let day: Vec<u32> = m.on.split('-').filter_map(|p| p.parse().ok()).collect();
                assert_eq!(day.len(), 3, "{}: `{}` needs YYYY-MM-DD", e.name, m.on);
                assert!(
                    (day[0], day[1], day[2]) >= FIRST_COMMIT,
                    "{}: measured {} predates this repository ({}-{:02}-{:02})",
                    e.name,
                    m.on,
                    FIRST_COMMIT.0,
                    FIRST_COMMIT.1,
                    FIRST_COMMIT.2
                );
            }
        }
    }

    /// An entry that names no feature is an entry nobody can switch off.
    ///
    /// `[omh]` is keyed on features, so this is load-bearing rather than
    /// documentary: the field is the only thing standing between a new entry
    /// and a default with no way out — which is the one thing the base set's
    /// own rule forbids.
    ///
    /// The grouping it records existed as a comment header in the manifest,
    /// the single claim in that file no test could check, while every other
    /// claim an entry makes is a field with a guard demanding it be filled.
    #[test]
    fn every_base_set_entry_names_its_feature() {
        for e in &shipped().entries {
            assert!(
                !e.feature.trim().is_empty(),
                "{}: names no feature. An entry belonging to nothing cannot be \
                 disabled, because `[omh]` takes feature names.",
                e.name
            );
        }
    }

    /// `remove` is printed by `omh why` as the way out, so an instruction that
    /// silently does nothing is worse than none at all.
    ///
    /// The five hooks each said `rm .omh/profile/hooks/<name>.json`, naming a
    /// file omh no longer writes. Removal is feature-level now: the graph hooks
    /// go with the server, and the git notice has nothing to uninstall.
    #[test]
    fn no_remove_instruction_names_a_path_omh_no_longer_writes() {
        for e in &shipped().entries {
            assert!(
                !e.remove.contains(".omh/profile/"),
                "{}: `remove` says `{}`, and that path is not written any more — \
                 the hooks are generated from this manifest",
                e.name,
                e.remove
            );
        }
    }

    /// A tool the agent does not know about is a tool it will not use — half
    /// of what makes the graph more than an installed package.
    ///
    /// Named tools, when *not* to use them, and which project is its own: the
    /// store holds every session's graph, and querying another session's
    /// answers confidently about code that is not in this worktree.
    #[test]
    fn the_graph_section_explains_the_tools_and_which_project_to_ask() {
        let body = section_body("graph-rules");
        assert!(body.contains("search_graph"), "must name the tools: {body}");
        assert!(
            body.to_lowercase().contains("grep"),
            "and when not to use them: {body}"
        );
        assert!(
            body.contains("OMH_GRAPH_PROJECT"),
            "and which project is its own: {body}"
        );
    }

    /// The agent meets `fatal: not a git repository` and has to explain it to
    /// itself. Left to guess it reaches for `git init`, which refuses for the
    /// same reason and changes nothing — so the notice says the repair is
    /// futile rather than leaving that to be discovered a turn later.
    ///
    /// Naming what to run instead is the load-bearing half: an agent that
    /// knows only that git is missing still promises a commit it cannot make.
    #[test]
    fn the_git_section_says_the_repair_is_futile_and_what_to_do_instead() {
        let body = section_body("git-rules");
        assert!(
            body.contains("git init"),
            "the move it would otherwise make has to be named: {body}"
        );
        assert!(
            body.contains("omh s commit") && body.contains("omh s push"),
            "and what the human runs instead: {body}"
        );
    }

    fn section_body(name: &str) -> String {
        sections()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is a section omh ships"))
            .body
    }

    /// A feature is not a group of hooks. It is a group of entries **across
    /// kinds** — a server, the hooks that make it used, the section telling the
    /// agent it is there — and that is why it is the unit removal and disabling
    /// work on. Half of `codegraph` is not a smaller version of it.
    ///
    /// Asserted on the one feature that has all three, and asserted because the
    /// grouping used to be a comment header: removing the server left four
    /// hooks nudging the agent toward something that was gone.
    #[test]
    fn a_feature_gathers_entries_across_kinds() {
        let manifest = shipped();
        let kinds: BTreeSet<Kind> = manifest
            .entries
            .iter()
            .filter(|e| e.feature == "codegraph")
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            BTreeSet::from([Kind::Mcp, Kind::Hook, Kind::Rules]),
            "codegraph is a server, the hooks that make it used, and the section \
             that tells the agent it exists"
        );
    }

    /// The rules are the one cost paid on every single turn, so the number in
    /// the manifest has to be the number the agent is actually handed.
    ///
    /// The same guard as `the_grep_nudges_declared_cost_matches_the_string_it_ships`,
    /// and for the same reason: `~40 B` sat in this file describing a 243-byte
    /// string, through a review that read it twice, because a hand-written cost
    /// and the string it describes have no relationship a test can check.
    #[test]
    fn every_rules_section_costs_what_it_says() {
        let manifest = shipped();
        for section in sections() {
            let entry = manifest
                .entry(section.name)
                .unwrap_or_else(|| panic!("{} has no manifest entry", section.name));
            let claim = &entry.measured[0].value;
            let declared: usize = claim
                .trim_end_matches(" B")
                .replace(',', "")
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{}: `{claim}` is not a byte count", section.name));
            assert_eq!(
                declared,
                section.body.len(),
                "{}: the manifest claims {declared} B and the section ships {} B. \
                 Re-measure rather than trimming the prose to fit.",
                section.name,
                section.body.len()
            );
        }
    }

    /// `omh config mcp rm codegraph` has to take the hooks and the rules
    /// section with it. Before generation the four hooks were files and
    /// removing the server left them behind, nudging the agent toward
    /// something that was gone; generation would have reintroduced exactly
    /// that, because what omh generates was decided by the manifest alone.
    ///
    /// The `remove` field promises this. A guard on the *string* — which is
    /// what shipped first — passes just as happily when the instruction does
    /// nothing, so this asserts the behaviour instead.
    #[test]
    fn removing_a_feature_server_stops_generating_the_rest_of_it() {
        let manifest = shipped();
        let installed = BTreeSet::from(["memory".to_string()]);
        let own = own(&manifest, &BTreeSet::new(), &installed).unwrap();

        assert!(
            !own.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "no graph hook may outlive its server: {:?}",
            own.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );
        assert!(
            !own.sections.iter().any(|s| s.name == "graph-rules"),
            "and neither may the section telling the agent to query it"
        );
        assert!(
            own.sections.iter().any(|s| s.name == "memory-rules"),
            "memory is still installed, so its section stays"
        );
        assert!(
            own.hooks.iter().any(|h| h.name == "git-unavailable"),
            "git-notice has no server to remove, so nothing about it changes"
        );
    }

    /// A name the code ships and the manifest does not describe is not a
    /// feature somebody switched off — it is omh disagreeing with itself, and
    /// the two states were the same `false`.
    ///
    /// What made it destructive rather than merely lossy: the hook was not
    /// generated *and* `reserved` blocked any layer file of that name from
    /// substituting, so it existed nowhere and nothing said so. Reachable by
    /// hand-editing `~/.omh/base`, which `omh why`'s own comment calls a
    /// directory anyone can drop a file into.
    #[test]
    fn a_shipped_hook_the_manifest_does_not_describe_is_an_error() {
        let dir = manifest_dir(&[("2026.08.toml", &format!("version = \"2026.08\"{ONE_ENTRY}"))]);
        let manifest = Manifest::load_dir(dir.path()).unwrap();

        let err = own(&manifest, &BTreeSet::new(), &BTreeSet::new())
            .expect_err("the binary ships hooks this manifest never mentions");
        let err = format!("{err:#}");
        assert!(err.contains("graph-refresh"), "must name it: {err}");
        assert!(err.contains("omh init"), "and the way out: {err}");
    }

    /// A hand-written cost and the thing it measures have no relationship a
    /// test can check — which is how `~40 B` sat in the manifest describing a
    /// 243-byte string, through a review that read it twice.
    ///
    /// Where a cost is computable it gets computed, and the manifest has to
    /// agree. This is the only measurement in the base set that can be checked
    /// in-process; the rest need a container, and are the reason `omh doctor`
    /// exists for adapter claims.
    #[test]
    fn the_grep_nudges_declared_cost_matches_the_string_it_ships() {
        // A representative session project name — `repo-sNN`, and it appears
        // twice in the nudge, so the length is not incidental.
        let project = project_name("ohmyharness", "s01");
        let actual = grep_nudge(&project).len();

        let entry = shipped()
            .entry("graph-first")
            .expect("graph-first in the manifest")
            .measured[0]
            .value
            .clone();
        let declared: usize = entry
            .trim_end_matches(" B")
            .replace(',', "")
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("graph-first cost `{entry}` is not a byte count"));

        assert_eq!(
            declared, actual,
            "the manifest claims {declared} B; the nudge it ships is {actual} B for project \
             `{project}`. Re-measure rather than adjusting the string to fit."
        );
    }

    // ── load_dir ────────────────────────────────────────────────────────────
    //
    // This had no tests, which is how it shipped three ways to silently choose
    // the wrong base set. All of them were found by running the binary in a
    // scratch HOME, none by reading it.

    fn manifest_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        dir
    }

    const ONE_ENTRY: &str = r#"
[[entry]]
name = "codegraph"
kind = "mcp"
feature = "codegraph"
since = "2026.06"
because = "b"
remove = "r"
command = "c"
"#;

    /// The manifest in `~/.omh/base` is whatever the last `init` seeded, and a
    /// newer omh can require a field it does not have — `feature` did exactly
    /// that. Every command loads the manifest, so the upgrade turns the whole
    /// tool off until it is refreshed, and the way to refresh it is the one
    /// thing the error has to say.
    ///
    /// The closed loop this repo already paid for once: `chmod 000` on
    /// `mcp.json` made `omh why` advise `omh init`, which did nothing. Here
    /// `init` genuinely fixes it — bundled files are refreshed, with the old
    /// one kept — so the advice is worth giving and worth pinning.
    #[test]
    fn a_manifest_an_older_omh_wrote_says_how_to_refresh_it() {
        let dir = manifest_dir(&[(
            "2026.08.toml",
            "version = \"2026.08\"\n[[entry]]\nname = \"codegraph\"\nkind = \"mcp\"\n\
             since = \"2026.06\"\nbecause = \"b\"\nremove = \"r\"\ncommand = \"c\"\n",
        )]);
        let err = format!("{:#}", Manifest::load_dir(dir.path()).unwrap_err());
        assert!(err.contains("2026.08.toml"), "must name the file: {err}");
        assert!(err.contains("omh init"), "must say the way out: {err}");
    }

    /// A stray `.toml` sorting after the real manifest used to *become* the
    /// base set: `init` seeded `{}` and reported success, and `omh why` called
    /// omh's own entries the user's.
    #[test]
    fn a_stray_toml_cannot_become_the_base_set() {
        let dir = manifest_dir(&[
            ("2026.08.toml", &format!("version = \"2026.08\"{ONE_ENTRY}")),
            ("zz-notes.toml", "version = \"notes\"\n"),
        ]);
        let m = Manifest::load_dir(dir.path()).unwrap();
        assert_eq!(m.version, "2026.08");
        assert_eq!(m.servers().len(), 1, "the real manifest must win");
    }

    /// Filename sort made `2027.2` beat `2027.10`, silently serving an older
    /// base set. Zero-padding was load-bearing and unenforced.
    #[test]
    fn versions_are_compared_numerically_not_lexicographically() {
        let dir = manifest_dir(&[
            ("z.toml", &format!("version = \"2027.2\"{ONE_ENTRY}")),
            ("a.toml", &format!("version = \"2027.10\"{ONE_ENTRY}")),
        ]);
        assert_eq!(Manifest::load_dir(dir.path()).unwrap().version, "2027.10");
    }

    /// The failure `the_document_init_seeds_actually_contains_the_base_set`
    /// describes, arriving through the runtime path that test cannot see: a
    /// manifest that parses but names nothing seeds an empty base set while
    /// hooks still point at a server that is not installed.
    #[test]
    fn a_manifest_naming_nothing_is_an_error_not_an_empty_base_set() {
        let dir = manifest_dir(&[("2026.08.toml", "version = \"2026.08\"\n")]);
        let err = Manifest::load_dir(dir.path()).unwrap_err().to_string();
        assert!(err.contains("no base-set entries"), "got: {err}");
    }

    #[test]
    fn an_empty_directory_says_what_to_do() {
        let dir = manifest_dir(&[]);
        let err = Manifest::load_dir(dir.path()).unwrap_err().to_string();
        assert!(err.contains("omh init"), "got: {err}");
    }

    /// Every answer has to be able to name the manifest that produced it.
    #[test]
    fn a_loaded_manifest_knows_where_it_came_from() {
        let dir = manifest_dir(&[("2026.08.toml", &format!("version = \"2026.08\"{ONE_ENTRY}"))]);
        let source = Manifest::load_dir(dir.path()).unwrap().source();
        assert!(source.contains("2026.08.toml"), "got: {source}");
        assert!(source.contains("2026.08"), "got: {source}");
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

        // The rules sections have the same split for the same reason, and so
        // the same failure available: a section shipped with no entry reaches
        // every session unexplained and uncosted, and an entry with no section
        // is `omh why` describing prose nobody receives.
        let declared: BTreeSet<&str> = manifest
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Rules)
            .map(|e| e.name.as_str())
            .collect();
        let shipped_sections: BTreeSet<&str> = sections().iter().map(|s| s.name).collect();
        assert_eq!(
            declared, shipped_sections,
            "rules sections in the manifest vs sections in the code"
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
        let body =
            serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": manifest.servers() }))
                .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let servers = parsed["mcpServers"]
            .as_object()
            .expect("an mcpServers object");
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

    /// The manifest's arguments and the launcher's mounts have to name the
    /// same directories. A server that starts, finds nothing, and reports "0
    /// notes" is the failure this prevents — it looks exactly like an empty
    /// store, so nobody investigates.
    ///
    /// Asserted against the constants rather than against literals, so moving
    /// a mount without updating the manifest cannot stay green.
    #[test]
    fn the_memory_server_is_pointed_at_the_directories_omh_mounts() {
        let servers = shipped().servers();
        let memory = servers
            .get(crate::memory::tools::SERVER_KEY)
            .expect("the base set must declare the memory server");

        assert!(
            memory
                .args
                .iter()
                .any(|a| a == crate::memory::GUEST_LOCAL_NOTES),
            "the local store is mounted at {}, args say {:?}",
            crate::memory::GUEST_LOCAL_NOTES,
            memory.args
        );
        // The committed layer is tracked, so it arrives inside the worktree —
        // there is no mount for it, and its path is /work-relative.
        assert!(
            memory.args.iter().any(|a| a == "/work/.omh/notes"),
            "the team store lives in the checkout: {:?}",
            memory.args
        );
        // Nothing that pins a session: one manifest serves every session, and
        // the server reads $OMH_SESSION for provenance.
        assert!(
            !memory.args.iter().any(|a| a.contains("--session")),
            "a session baked into the base set would be wrong for every other one"
        );
    }

    /// A hand-typed byte count in this file has already been wrong by 5x — the
    /// grep nudge declared ~40 B and shipped 243. Anything computable in
    /// process is computed, and this is.
    #[test]
    fn the_memory_surfaces_declared_cost_matches_what_it_ships() {
        let mut server = crate::memory::tools::Server {
            team: std::path::PathBuf::from("/nonexistent-team"),
            local: std::path::PathBuf::from("/nonexistent-local"),
            templates: crate::memory::shipped_templates(),
            session: "s01".into(),
            client: None,
            today: || "2026-08-08".to_string(),
        };
        let listed = crate::mcp::Tools::list(&mut server);
        let actual: usize = listed
            .iter()
            .map(|t| {
                t.name.len()
                    + t.description.len()
                    + serde_json::to_string(&t.input_schema).unwrap().len()
            })
            .sum();

        let declared = shipped()
            .entries
            .iter()
            .find(|e| e.name == "memory")
            .expect("the memory entry")
            .measured
            .iter()
            .find(|m| m.what.contains("injected"))
            .expect("an injected-cost measurement")
            .value
            .trim_end_matches(" B")
            .parse::<usize>()
            .expect("a byte count");

        assert_eq!(
            actual, declared,
            "re-measure rather than adjusting the surface to fit"
        );
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
    ///
    /// Scoped to the hooks that reach the graph, which is where the guarantee
    /// comes from: the store holds every session's graph for this repo, so a
    /// query that does not name one answers about the wrong worktree. A hook
    /// that touches no store has no project to name, and asserting otherwise
    /// would only force a variable into text that does not use it.
    #[test]
    fn hooks_that_query_the_graph_name_their_project_through_the_environment() {
        let querying: Vec<_> = hooks()
            .into_iter()
            .filter(|h| h.command.contains(GRAPH_BIN))
            .collect();
        assert!(
            !querying.is_empty(),
            "the filter must still match something"
        );
        for h in querying {
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

    /// The rules file says this too, but a rules file decays as context grows —
    /// which is the reason this repo already gives for preferring delivery
    /// attached to the call. The hook fires at the moment the agent reaches for
    /// git, which is where the sentence actually lands.
    #[test]
    fn the_git_notice_fires_on_the_call_that_would_fail() {
        let h = hook("git-unavailable");
        assert_eq!(h.event, "PreToolUse");
        assert_eq!(h.matcher, "Bash", "git arrives as a shell command");
        assert!(
            h.command.contains("git init"),
            "the repair it would otherwise reach for has to be named: {}",
            h.command
        );
    }

    /// Every hook is a shell one-liner, and nothing else here would notice one
    /// that cannot parse.
    ///
    /// The `git-unavailable` hook embeds prose, and prose contains apostrophes:
    /// a `shell_quote` that lets one through produces `unexpected EOF while
    /// looking for matching '`, which is a hook that silently never runs. Every
    /// assertion over a hook's *command string* is satisfied by that hook —
    /// `contains("git init")` passes on a script `sh` refuses to parse. This is
    /// the cheapest guard that covers all of them, including the ones whose
    /// binaries are not installed here.
    #[test]
    fn every_hook_command_is_valid_shell() {
        for h in hooks() {
            let out = std::process::Command::new("sh")
                .args(["-n", "-c", &h.command])
                .output()
                .expect("sh must run");
            assert!(
                out.status.success(),
                "{} is not parseable by sh: {}\n{}",
                h.name,
                String::from_utf8_lossy(&out.stderr),
                h.command
            );
        }
    }

    /// And that every one of them survives being *run*, which parsing does not
    /// prove: an unbound variable, a `case` that falls through to an error, or a
    /// missing binary all parse fine.
    ///
    /// The graph binary is stubbed rather than required — what is under test is
    /// omh's script, not the server. A hook whose tool is absent must still exit
    /// 0 and stay quiet, because a session where the graph is not installed is
    /// a session, not a failure.
    #[test]
    fn every_hook_runs_quietly_when_its_tool_says_nothing() {
        let stub = tempfile::tempdir().unwrap();
        for name in [GRAPH_BIN, "codebase-memory-mcp"] {
            let at = stub.path().join(name);
            std::fs::write(&at, "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let path = format!(
            "{}:{}",
            stub.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );

        for h in hooks() {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(&h.command)
                .env("PATH", &path)
                .env(PROJECT_ENV, "repo-s01")
                .stdin(std::process::Stdio::null())
                .output()
                .expect("sh must run");
            assert!(
                out.status.success(),
                "{} exited {:?}: {}",
                h.name,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                out.stderr.is_empty(),
                "{} wrote to stderr, which the harness shows the user: {}",
                h.name,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// Run the hook the way the harness does.
    ///
    /// Asserting on the command *string* proves the sentence is embedded, never
    /// that a shell will emit it — and this one is a `case` over prose that has
    /// to survive `sh` quoting. Two separate defects lived through the string
    /// assertion above: the pattern matching nothing, and `shell_quote` letting
    /// the apostrophe in "worktree's" end the argument, which is a syntax error
    /// rather than a wrong answer.
    fn fire_hook(command: &str) -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&hook("git-unavailable").command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("sh must run");
        let payload = serde_json::json!({ "tool_input": { "command": command } });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.stderr.is_empty(),
            "the hook must not write to stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn the_git_notice_reaches_the_agent_verbatim() {
        let fired = fire_hook("git status");
        let parsed: serde_json::Value =
            serde_json::from_str(&fired).unwrap_or_else(|e| panic!("not JSON: {fired} ({e})"));
        assert_eq!(
            parsed["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap(),
            GIT_ABSENT,
            "the prose has to survive shell quoting intact"
        );
    }

    /// The shapes an agent actually emits. A newline-separated script is the
    /// most common of them and the easiest to miss, because a `case` separator
    /// class written by hand does not include one.
    #[test]
    fn the_git_notice_matches_git_wherever_a_command_can_start() {
        for command in [
            "git status",
            "cd /work && git status",
            "cd /work; git init",
            "cd /work\ngit status",
            "  git status",
            "echo hi | git apply",
        ] {
            assert!(
                !fire_hook(command).trim().is_empty(),
                "silent on {command:?}, which is a git call"
            );
        }
    }

    /// Bash is most of what an agent runs, so a nudge on every call is the
    /// noise `graph-read` exists to avoid. This is the `0 B` the manifest claims.
    #[test]
    fn the_git_notice_is_silent_on_everything_else() {
        for command in ["cargo test", "ls -la", "echo git", "rg digital"] {
            assert!(
                fire_hook(command).trim().is_empty(),
                "fired on {command:?}, which is not a git call"
            );
        }
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

    /// Reading a whole module to see one function is the largest avoidable cost
    /// in a session: `get_code_snippet` answers the same question in ~1,500
    /// bytes. No file size named — the figure that used to be here was stale on
    /// the commit that wrote it, and it appeared in four places.
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
