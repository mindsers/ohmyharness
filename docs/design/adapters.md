# Adapters

A harness is a TOML file, not a code change. So is an editor.

## Capabilities are optional keys

The profile carries the **superset** of what any harness might express. An
adapter declares which parts its harness can actually express, and where.

```toml
name    = "claude"
bin     = "claude"
install = "npm install -g @anthropic-ai/claude-code"

[capabilities.rules]
path   = "/work/CLAUDE.md"
also   = ["/work/AGENTS.md"]
render = "concat"

[capabilities.mcp]
path   = "$HOME/.mcp.json"
render = "mcp-json"
import = "$REPO/.mcp.json"     # host-side, for `omh config mcp import`

creds = ["$HOME/.claude/", "$HOME/.claude.json"]
token = ["$HOME/.claude/.credentials.json"]
```

**An absent key means the harness cannot do that thing.** Degradation is a
missing map entry, not special-case logic — which is why adding a harness needs
no branch anywhere in the codebase.

It is announced once, at launch, rather than silently swallowed:

```console
$ omh codex
omh: codex on omh/s01 — dropped 2 hooks, 3 subagents (unsupported)
```

The capability classes are: `rules`, `skills`, `mcp`, `commands`, `subagents`,
`hooks`.

### Unknown fields are rejected

Adapters parse with `deny_unknown_fields`. Without it, a stale adapter parses
cleanly with **zero** capabilities and every harness silently degrades to
nothing — the worst possible failure for a tool whose entire promise is that
your setup is already there.

## Renderers

| Render | Used by | Effect |
|---|---|---|
| `dir` | skills, commands, subagents | link the catalogue's entries by name, mount read-only |
| `concat` | rules | join the sections into one document, staged and mounted read-only |
| `mcp-json`, `codex-toml`, `opencode-json` | mcp | reshape the canonical server list |
| `claude-settings` | hooks | reshape the canonical hook list |

`concat` is mounted, never written into the worktree — writing there made omh's
staging indistinguishable from the agent's work and carried omh's rules into
users' pull requests.

## Hooks: three maps and a template

A hook is authored in omh's vocabulary and translated when it is staged, so the
adapter is where a harness's spelling lives — the same rule as every `path`.

```toml
# Adapter-level: hooks match on these, and the Agent Skills standard and
# subagent frontmatter both carry harness tool names too. One vocabulary.
[tools]
edit   = "Edit|Write|MultiEdit"
read   = "Read"
shell  = "Bash"
search = "Grep|Glob"

[capabilities.hooks]
path   = "$HOME/.claude/settings.json"
render = "claude-settings"

[capabilities.hooks.events]        # omh's moments → this harness's names
session-start = "SessionStart"
turn-end      = "Stop"
before-tool   = "PreToolUse"
after-tool    = "PostToolUse"

[capabilities.hooks.fields]        # where this harness keeps each field
tool-file    = ".tool_input.file_path"
tool-command = ".tool_input.command"

[capabilities.hooks.inject]        # how it accepts advisory text for the agent
template = """jq -nc --arg m {{text}} '{"hookSpecificOutput":{"hookEventName":"{{event}}","additionalContext":$m}}'"""

[capabilities.hooks.refuse]        # and how it blocks a call, with a reason
template = """jq -nc --arg m {{text}} '{"hookSpecificOutput":{"hookEventName":"{{event}}","permissionDecision":"deny","permissionDecisionReason":$m}}'"""
```

**`fields` is read in the renderer's own language.** Those values are jq paths
because Claude Code hands a hook its payload on stdin; opencode's are property
names, because a plugin receives the tool's arguments as an object. The map
answers "where does this harness keep the file path", and there is no single
syntax for which that is true — so each render reads it in the language it
emits, and an adapter says which by its `render`.

**Advising and blocking are two templates, and either may be absent.** They are
one field apart on Claude Code and genuinely different mechanisms elsewhere: on
opencode advisory text has no channel before a tool runs, and the only way to
speak there is to throw, which blocks. A harness that can do one and not the
other drops the hooks wanting the other **by name** — never substituting, in
either direction.

**An absent entry means this harness has no such thing** — the capability map's
rule, one level down. The hooks wanting it are dropped **by name**, saying what
they asked for, and the rest still ship: a harness with `turn-end` and no
`before-tool` keeps most of them, and a count per capability would report that
as "hooks: 0".

`events` is required on a `hooks` binding. Without it nothing can be expressed,
every hook is dropped, and the harness receives an empty settings document —
indistinguishable from a harness that declares no hooks, except that this one
claimed to have them. These maps are refused on any other capability, where
nothing would read them.

**Every renderer must round-trip through its parser.** `omh config mcp import`
is the exact inverse of rendering, so a format that renders but parses lossily
means import silently drops fields. That is a test, not a hope.

## Adding a harness

1. Write `~/.omh/adapters/<name>.toml`. Declare only what the harness genuinely
   supports.
2. `omh doctor <name>`.
3. Fix whatever it says. Repeat.

Step 2 is not optional, and it is not a formality. **Adapter facts are
unverified claims about external software that ships weekly**, and they break
*silently* — a wrong path means the harness starts fine and simply never sees
your profile.

A green unit suite proves omh mounted a path faithfully. It cannot prove
anything read it. See [Troubleshooting](../troubleshooting.md#why-it-exists).

### The bar for shipping one

Both bundled adapters are verified by `doctor` against a real container:
`claude` passes 8 checks and `opencode` 7, hooks included on both — opencode's
being a generated plugin rather than a config file. Any third adapter inherits
the same bar.

`opencode` passing `doctor` is **not** the same as `opencode` being proven — it
means the paths are right, not that the harness has been driven for real work.
Only `claude` has. That distinction is deliberate and is repeated wherever the
claim appears.

## Adding an editor

Same principle, smaller file:

```toml
# ~/.omh/editors/zed.toml
name = "zed"
bin  = "zed"
args = ["$URL"]
```

Editors attach from outside over SSH rather than running inside the sandbox, so
they have no capabilities to declare. See [Editors](../editors.md).

## Breadth is capped on purpose

Adapter breadth stays at two harnesses until the base set is earned.

**Breadth before depth is how distributions die.** Ten adapters that each half-work
is a worse product than two that are verified, and it is a much worse product to
maintain, because every one of them is a standing claim about software you do not
control.

See the [roadmap](roadmap.md) for when that cap lifts.
