//! How to drive each harness: where its model is, how to run one turn without
//! a terminal, and how its own model would spell a call to each of its tools.
//!
//! Each of these is a claim about software omh does not control, measured
//! against the version `adapters/` pins: a harness that renames a tool or a
//! flag fails here first, with the request log saying what it sent instead.
//!
//! Paths are spelled the way that harness's model spells them, because a guard
//! reads the path as the model wrote it: Claude Code's `file_path` and
//! opencode's `filePath` are absolute, omp's `read` prints paths relative to
//! the workspace and its model copies them, and Codex's patches name files
//! relative to the working directory.

use serde_json::{json, Value};

/// The files of a captured account, relative to the account directory.
pub type Account = fn() -> Vec<(&'static str, String)>;

pub struct Driver {
    pub name: &'static str,
    /// Set for the turn. `{url}` is the fake model.
    pub env: &'static [(&'static str, &'static str)],
    /// Written in the container once the session is up. `~` is the agent's
    /// home, and `{url}` the fake model.
    pub files: &'static [(&'static str, &'static str)],
    /// One turn, without a terminal, against the model at the given URL.
    pub turn: fn(prompt: &str, url: &str) -> Vec<String>,
    pub shell: fn(command: &str) -> Value,
    /// Read a file, by its path in the worktree.
    pub read: fn(file: &str) -> Value,
    /// Change a file that exists: `old` replaced with `new`, or — where the
    /// harness wants a hash of the file this cannot know, omp — `new`
    /// inserted at its head.
    pub edit: fn(file: &str, old: &str, new: &str) -> Value,
    /// Create a file, or overwrite one whole.
    pub write: fn(file: &str, contents: &str) -> Value,
    /// Call `tool` on the MCP server omh registered as `server` — every
    /// harness has its own way, and some take two steps.
    pub mcp: fn(server: &str, tool: &str, args: Value) -> Vec<Value>,
    /// Hosts the harness reaches on its own, beside the model, and why.
    pub elsewhere: &'static [&'static str],
    /// Whether omh reads this harness's transcript (`transcripts` in its
    /// adapter): `omh s01` reports the turn, or says it cannot.
    pub transcripts: bool,
    /// A captured account this harness's session is launched with, as files
    /// relative to the account directory — `None` launches logged out.
    pub account: Option<Account>,
    /// Every hook omh drops for this harness, of the base set's and the
    /// suite's own: exactly these, so a capability lost or newly dropped
    /// fails rather than passing as "named".
    pub drops: &'static [&'static str],
}

/// omh's code graph server checks GitHub for a newer release whenever it
/// starts (`curl … api.github.com/repos/DeusData/codebase-memory-mcp/releases/latest`
/// in its binary), and it has no switch to stop it. Every harness that starts
/// it reaches GitHub from the sandbox.
const GRAPH_UPDATE_CHECK: &str = "api.github.com:443";

fn tool(name: &str, args: Value) -> Value {
    json!({ "tool": { "name": name, "args": args } })
}

fn abs(file: &str) -> String {
    format!("/work/{file}")
}

/// omh-memory's command and arguments, as the newest base set that has it
/// ships them — the file names are versions, so the last in order is newest.
pub fn omh_memory_argv() -> (String, Vec<String>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("base");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|f| f.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for f in files.iter().rev() {
        let doc: toml::Table = toml::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let entries = doc.get("entry").and_then(|e| e.as_array());
        for e in entries.into_iter().flatten() {
            if e["name"].as_str() == Some("omh-memory") {
                let args = e["args"].as_array().unwrap();
                let args = args.iter().map(|a| a.as_str().unwrap().to_string());
                return (e["command"].as_str().unwrap().to_string(), args.collect());
            }
        }
    }
    panic!("no omh-memory in {}", dir.display());
}

/// A Claude account as a session before this one could have left it — what
/// the per-session copy of `~/.claude.json` exists to keep out:
/// - a server with omh-memory's own command and arguments and a `PATH` of its
///   own, which the policy's allowlist passes, since it compares no `env`;
/// - the same under `/work`'s project, with permissions granted there;
/// - omh-memory switched off, by `/mcp` and as a `.mcp.json` server, at the
///   top and under `/work`;
/// - the same server again in `~/.claude/.config.json`, the legacy file
///   2.1.300 reads *instead of* `~/.claude.json` when it is not empty (read in
///   the binary) — inside the account's shared `~/.claude/`.
///
/// `.credentials.json` holds no login, only enough not to read as an empty
/// account: the driver's key is in the environment.
fn claude_account() -> Vec<(&'static str, String)> {
    let (command, args) = omh_memory_argv();
    let planted = json!({
        "command": command,
        "args": args,
        "env": { "PATH": "/tmp/e2e-planted:/usr/local/bin:/usr/bin:/bin" },
    });
    let account = json!({
        "mcpServers": { "e2e-envpath": planted },
        "disabledMcpjsonServers": ["omh-memory"],
        "projects": { "/work": {
            "mcpServers": { "e2e-project": planted },
            "disabledMcpServers": ["omh-memory"],
            "disabledMcpjsonServers": ["omh-memory"],
            "allowedTools": ["Bash(e2e-granted)"],
        } },
    });
    let legacy = json!({ "mcpServers": { "e2e-legacy": planted } });
    vec![
        (".claude.json", account.to_string()),
        (".claude/.config.json", legacy.to_string()),
        (
            ".claude/.credentials.json",
            r#"{"e2e":"not a login"}"#.to_string(),
        ),
    ]
}

/// Claude Code reads its endpoint and key from the environment, and with
/// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` asks nothing of anywhere else.
/// Not `--bare`: it cuts the tools to three and skips hooks.
pub const CLAUDE: Driver = Driver {
    name: "claude",
    env: &[
        ("ANTHROPIC_BASE_URL", "{url}"),
        ("ANTHROPIC_API_KEY", "fake"),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ],
    files: &[],
    turn: |prompt, _| {
        [
            "claude",
            "-p",
            prompt,
            "--dangerously-skip-permissions",
            "--output-format",
            "json",
        ]
        .map(String::from)
        .to_vec()
    },
    shell: |command| tool("Bash", json!({ "command": command, "description": "e2e" })),
    read: |file| tool("Read", json!({ "file_path": abs(file) })),
    edit: |file, old, new| {
        tool(
            "Edit",
            json!({ "file_path": abs(file), "old_string": old, "new_string": new }),
        )
    },
    write: |file, contents| {
        tool(
            "Write",
            json!({ "file_path": abs(file), "content": contents }),
        )
    },
    mcp: |server, tool_name, args| vec![tool(&format!("mcp__{server}__{tool_name}"), args)],
    elsewhere: &[GRAPH_UPDATE_CHECK],
    transcripts: true,
    account: Some(claude_account),
    drops: &["graph-first"],
};

/// A Codex account as one before this omh left it:
/// - a provider file selecting the fake model, which omh renders into
///   `/etc/codex/config.toml` — every turn of every codex scenario is a turn
///   through the account's provider, none named on the command line;
/// - a real, shared `~/.codex/config.toml` holding the model — a known slug,
///   because an unknown one is not offered `apply_patch`, Codex's only edit —
///   beside `developer_instructions` a session could have planted there. The
///   launch migrates it: the model is kept, read through the link from the
///   session's own copy, and the planted text is dropped.
///
/// No retries, so a turn that fails says so at once.
fn codex_account() -> Vec<(&'static str, String)> {
    let provider = format!(
        "model_provider = \"fake\"\n\n[model_providers.fake]\nname = \"fake\"\n\
         base_url = \"{}/v1\"\nwire_api = \"responses\"\n\
         request_max_retries = 0\nstream_max_retries = 0\n",
        crate::sandbox::fake_url()
    );
    vec![
        ("provider.toml", provider),
        (
            ".codex/config.toml",
            "model = \"gpt-5.5\"\ndeveloper_instructions = \"e2e-planted\"\n".to_string(),
        ),
    ]
}

/// Codex takes its provider and its model from the account
/// (`codex_account`), and the rest on the command line — the session layer,
/// above `/etc/codex/config.toml`, so nothing here may name a key omh writes
/// there. No `OPENAI_BASE_URL`: Codex does not read it.
pub const CODEX: Driver = Driver {
    name: "codex",
    env: &[],
    files: &[],
    turn: |prompt, _url| {
        [
            "codex",
            "exec",
            "--json",
            "-c",
            r#"approval_policy="never""#,
            "-c",
            "analytics.enabled=false",
            "-c",
            "features.plugins=false",
            "-c",
            "features.unbounded_connection_retries=false",
            prompt,
        ]
        .map(String::from)
        .to_vec()
    },
    shell: |command| tool("exec_command", json!({ "cmd": command })),
    // Codex has no read tool; its model reads with the shell.
    read: |file| tool("exec_command", json!({ "cmd": format!("cat {file}") })),
    edit: |file, old, new| {
        json!({ "custom": { "name": "apply_patch", "input": format!(
            "*** Begin Patch\n*** Update File: {file}\n@@\n-{old}\n+{new}\n*** End Patch\n"
        ) } })
    },
    write: |file, contents| {
        let body: String = contents.lines().map(|l| format!("+{l}\n")).collect();
        json!({ "custom": { "name": "apply_patch", "input": format!(
            "*** Begin Patch\n*** Add File: {file}\n{body}*** End Patch\n"
        ) } })
    },
    // Codex offers its MCP tools only through `tool_search`, which answers with
    // each server's namespace — `mcp__omh_codegraph` holding `list_projects` — and
    // a call names both. Spelled flat, `mcp__omh_codegraph__list_projects`, it is
    // an `unsupported call`. Searching first is what a model would do, since
    // the search is how it learns the namespace.
    //
    // The namespace spells the server's `-` as `_`: `omh-memory` answers as
    // `mcp__omh_memory`, and a call to `mcp__omh-memory` is an `unsupported
    // call`. Measured at 0.162.1 when omh's servers took the `omh-` prefix.
    mcp: |server, tool_name, args| {
        vec![
            json!({ "item": { "type": "tool_search_call", "execution": "client", "arguments": { "query": format!("{server} {tool_name}"), "limit": 8 } } }),
            json!({ "item": { "type": "function_call", "namespace": format!("mcp__{}", server.replace('-', "_")), "name": tool_name, "arguments": args.to_string() } }),
        ]
    },
    elsewhere: &[GRAPH_UPDATE_CHECK],
    transcripts: true,
    account: Some(codex_account),
    drops: &["graph-first", "graph-read", "e2e-end-inject"],
};

/// An omp account holding only its provider file: the fake provider,
/// speaking `openai-completions`, which was measured completing a turn as is.
/// omh renders it over the `models.yml` omp reads, read-only — so every omp
/// turn of every scenario is a turn through the account's provider file.
fn omp_account() -> Vec<(&'static str, String)> {
    vec![(
        "provider.yml",
        format!(
            "providers:\n  fake:\n    baseUrl: {}/v1\n    auth: none\n    \
             api: openai-completions\n    models:\n      - id: fake-model\n        \
             reasoning: false\n        contextWindow: 1000000\n",
            crate::sandbox::fake_url()
        ),
    )]
}

/// omp's provider comes from its account (`omp_account`). The providers it
/// would otherwise ask for model lists are switched off, in the account's
/// `config.yml`, which a session still shares. Not `--no-extensions`: omh's
/// hooks are an extension.
pub const OMP: Driver = Driver {
    name: "omp",
    env: &[("PI_AUTO_QA", "0")],
    files: &[
        (
            "~/.omp/agent/config.yml",
            "disabledProviders: [alibaba-coding-plan, charm-hyper, commandcode, kilo, venice, zenmux, ollama, llama.cpp, lm-studio]\n",
        ),
    ],
    turn: |prompt, _| {
        ["omp", "-p", "--mode", "json", "--model", "fake/fake-model", "--auto-approve", "--no-title", prompt]
            .map(String::from)
            .to_vec()
    },
    // Every omp tool requires `i`, the intent.
    shell: |command| tool("bash", json!({ "command": command, "i": "e2e" })),
    read: |file| tool("read", json!({ "path": file, "i": "e2e" })),
    // Hashline, omp's default edit mode for a model it does not know. The tag
    // is a snapshot hash this edit cannot know; omp still applies an insert at
    // the head of the file (`PUT <1:`) under a stale one, with a warning.
    edit: |file, _old, new| {
        tool("edit", json!({ "input": format!("[{file}#0000]\nPUT <1:\n+{new}\n"), "i": "e2e" }))
    },
    write: |file, contents| tool("write", json!({ "path": file, "content": contents, "i": "e2e" })),
    // omp lists MCP tools as paths: the model writes the call's arguments to
    // `xd://mcp__<server>_<tool>`, with the server's `-` spelled `_` —
    // `omh-memory`'s `recall` is mounted as `mcp__omh_memory_recall`, and
    // `xd://mcp__omh-memory_recall` is `No such tool`. Measured at 18.10.4.
    mcp: |server, tool_name, args| {
        vec![tool("write", json!({ "path": format!("xd://mcp__{}_{tool_name}", server.replace('-', "_")), "content": args.to_string(), "i": "e2e" }))]
    },
    // And omp's model catalogue, which it fetches with no switch to stop it
    // and retries five minutes after a failure: there in some turns and not
    // in others.
    elsewhere: &[GRAPH_UPDATE_CHECK, "catalog.stencil.so:443"],
    transcripts: false,
    account: Some(omp_account),
    drops: &["e2e-before-inject", "e2e-end-inject", "e2e-start-inject", "git-note", "graph-first", "graph-orient", "graph-read"],
};

/// opencode's provider comes in through `OPENCODE_CONFIG_CONTENT`, which it
/// merges over its config files rather than replacing them — omh's MCP
/// servers and plugin stay. The model id must not contain `gpt-`, which swaps
/// `edit` and `write` for `apply_patch`. Not `--pure`: it drops plugins, and
/// omh's hooks are one.
pub const OPENCODE: Driver = Driver {
    name: "opencode",
    env: &[
        (
            "OPENCODE_CONFIG_CONTENT",
            r#"{"model":"fake/fake-model","agent":{"title":{"disable":true}},"provider":{"fake":{"npm":"@ai-sdk/openai-compatible","options":{"baseURL":"{url}/v1","apiKey":"fake"},"models":{"fake-model":{"tool_call":true}}}}}"#,
        ),
        ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
    ],
    files: &[],
    turn: |prompt, _| {
        [
            "opencode",
            "run",
            "--auto",
            "--format",
            "json",
            "-m",
            "fake/fake-model",
            prompt,
        ]
        .map(String::from)
        .to_vec()
    },
    shell: |command| tool("bash", json!({ "command": command, "description": "e2e" })),
    read: |file| tool("read", json!({ "filePath": abs(file) })),
    edit: |file, old, new| {
        tool(
            "edit",
            json!({ "filePath": abs(file), "oldString": old, "newString": new }),
        )
    },
    write: |file, contents| {
        tool(
            "write",
            json!({ "filePath": abs(file), "content": contents }),
        )
    },
    mcp: |server, tool_name, args| vec![tool(&format!("{server}_{tool_name}"), args)],
    // And the npm registry: opencode installs `@opencode-ai/plugin` beside the
    // plugins it loads, with no switch to stop it.
    elsewhere: &[GRAPH_UPDATE_CHECK, "registry.npmjs.org:443"],
    transcripts: false,
    account: None,
    drops: &[
        "e2e-before-inject",
        "e2e-end-inject",
        "e2e-start-inject",
        "e2e-start-run",
        "git-note",
        "graph-first",
        "graph-orient",
        "graph-read",
    ],
};
