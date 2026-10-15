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

pub struct Driver {
    pub name: &'static str,
    /// Set for the turn. `{url}` is the fake model.
    pub env: &'static [(&'static str, &'static str)],
    /// Written in the container once the session is up. `~` is the agent's
    /// home.
    pub files: &'static [(&'static str, &'static str)],
    /// One turn, without a terminal, against the model at the given URL.
    pub turn: fn(prompt: &str, url: &str) -> Vec<String>,
    pub shell: fn(command: &str) -> Value,
    /// Read a file, by its path in the worktree.
    pub read: fn(file: &str) -> Value,
    /// Replace `old` with `new` in a file that exists.
    pub edit: fn(file: &str, old: &str, new: &str) -> Value,
    /// Create a file, or overwrite one whole.
    pub write: fn(file: &str, contents: &str) -> Value,
}

pub const ALL: [&Driver; 4] = [&CLAUDE, &CODEX, &OMP, &OPENCODE];

fn tool(name: &str, args: Value) -> Value {
    json!({ "tool": { "name": name, "args": args } })
}

fn abs(file: &str) -> String {
    format!("/work/{file}")
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
};

/// Codex takes a provider on the command line — the session layer, above
/// `/etc/codex/config.toml`, so nothing here may name a key omh writes there.
/// A known model slug, because an unknown one is not offered `apply_patch`,
/// which is Codex's only edit. No `OPENAI_BASE_URL`: Codex does not read it.
pub const CODEX: Driver = Driver {
    name: "codex",
    env: &[],
    files: &[],
    turn: |prompt, url| {
        let provider = format!(
            r#"model_providers.fake={{name="fake",base_url="{url}/v1",wire_api="responses",request_max_retries=0,stream_max_retries=0}}"#
        );
        [
            "codex",
            "exec",
            "--json",
            "-m",
            "gpt-5.5",
            "-c",
            "model_provider=fake",
            "-c",
            &provider,
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
};

/// omp's provider is a file. `openai-completions`, not `anthropic-messages`,
/// which renames every tool with a leading `_`. The providers it would
/// otherwise ask for model lists are switched off; catalog.stencil.so has no
/// switch, and its failure is swallowed. Not `--no-extensions`: omh's hooks
/// are an extension.
pub const OMP: Driver = Driver {
    name: "omp",
    env: &[("PI_AUTO_QA", "0")],
    files: &[
        (
            "~/.omp/agent/models.yml",
            "providers:\n  fake:\n    baseUrl: http://127.0.0.1:8787/v1\n    auth: none\n    api: openai-completions\n    models:\n      - id: fake-model\n        reasoning: false\n        contextWindow: 1000000\n",
        ),
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
    // is the file's snapshot hash, which only a `read` hands out; a guard
    // decides before omp checks it, and anything past the guard is refused
    // by omp for the tag rather than applied.
    edit: |file, _old, new| {
        tool("edit", json!({ "input": format!("[{file}#0000]\nPUT <1:\n+{new}\n"), "i": "e2e" }))
    },
    write: |file, contents| tool("write", json!({ "path": file, "content": contents, "i": "e2e" })),
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
};
