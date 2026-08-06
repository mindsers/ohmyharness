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
| `dir` | skills, commands, subagents | union layers by entry name, mount read-only |
| `concat` | rules | join layers, write into the worktree |
| `mcp-json`, `codex-toml`, `opencode-json` | mcp | reshape the canonical server list |
| `claude-settings` | hooks | reshape the canonical hook list |

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
`claude` passes 6 checks, `opencode` 4 (subagents and hooks correctly skipped).
Any third adapter inherits the same bar.

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
