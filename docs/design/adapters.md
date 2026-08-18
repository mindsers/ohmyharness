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
path   = "/work/.mcp.json"     # project-scoped; this harness reads no other
render = "mcp-json"
import = "$REPO/.mcp.json"     # host-side, for `omh config mcp import`
verify = "claude mcp list"     # what `omh doctor` asks the harness itself
ready  = "Connected"           # what that answer calls a server it loaded

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

## `token` or `token-probe`, never both

`token` names the file(s) whose contents prove a login. It is the better answer
wherever it applies, because a stat needs no container and cannot be wrong about
what it saw.

Some harnesses keep credentials somewhere omh cannot read — omp keeps them in
SQLite. There the only file that could be named is created by the harness
*starting*, so declaring it would report a login for every session that merely
opened the tool. Such an adapter declares a probe instead:

```toml
[token-probe]
run   = "omp usage --json"   # asked inside the sandbox, where the credentials are
ready = "accountId"          # what that answer says when the login is real
```

The pair is **refused at load**, not resolved by precedence: an adapter
declaring both leaves omh with two answers to one question and no rule for which
wins. It is the same shape as `verify`/`ready` one level up — when the claim is
about software omh did not write, ask that software — and the same rule applies
to the answer: the probe passes only when the command *succeeds* and its output
names `ready`, because a harness that errors out will happily print the marker
inside its error text.

A probe cannot be run by `omh auth`, which tears the container down before it
can ask. So `omh auth` on such a harness records the account and says the login
is **unconfirmed**, and `omh doctor` is what settles it.

## Renderers

| Render | Used by | Effect |
|---|---|---|
| `dir` | skills, commands, subagents | link the catalogue's entries by name, mount read-only |
| `concat` | rules | join the sections into one document, staged and mounted read-only |
| `mcp-json`, `codex-toml`, `opencode-json` | mcp | reshape the canonical server list |
| `claude-settings` | hooks | reshape the canonical hook list |
| `opencode-plugin`, `omp-plugin` | hooks | generate a JavaScript module |

Two of those emit a **program**, and they are separate renders rather than one
parameterised by the harness. They agree on the shell bridge — how a hook body
runs, how a failed predicate is reported, how `$OMH_*` expands — and on nothing
else: opencode registers one object of named handlers and reads a call's
arguments off `output` or `input` depending on the moment, while omp gives each
hook its own `pi.on(...)` registration and hands every handler an `event`. Sharing the
generator would mean a `match` on the harness in every line of it.

`concat` is mounted, never written into the worktree — writing there made omh's
staging indistinguishable from the agent's work and carried omh's rules into
users' pull requests. Any `path` under `/work` is handled the same way,
whichever renderer produced it: omh places the mountpoint and binds over it, so
a project's own file is hidden for the session and returned untouched.

## `verify` and `ready`: asking the harness

A `path` is a claim about software omh did not write, and the suite cannot check
it. `verify` is the harness's own command for listing what it loaded; `ready` is
the word its output uses for a server that is actually running. `omh doctor`
runs the one and greps for the other, on the same line as the server's name.

Both are optional. An adapter that declares neither is simply not asked — the
same degradation as any absent key. What that costs is on record: the `claude`
binding pointed `mcp` at `$HOME/.mcp.json`, which nothing reads, and every check
omh had stayed green while no session ever loaded a server.

Matching the name alone is not enough, and that is the point of `ready`. A
project-scoped document Claude Code has not been told to trust is listed in full
and loaded not at all, so a name-only check passes in exactly the state the
feature is broken in.

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

All three bundled adapters are verified by `doctor` against a real container,
hooks included on every one — opencode's and omp's being generated plugins
rather than config files, so they are checked by `node --check` inside the
sandbox rather than by existing. Any fourth adapter inherits the same bar.

Do not read the check *count* as an adapter fact: it varies with the
capabilities your profile declares and with whether a login has been captured
for that harness.

`opencode` passing `doctor` is **not** the same as `opencode` being proven — it
means the paths are right, not that the harness has been driven for real work.
Only `claude` has. That distinction is deliberate and is repeated wherever the
claim appears, and it applies to `omp` in the same words.

`omp` carries one claim weaker than the other two, and it is written here rather
than left in a comment: its `token-probe` greps `omp usage --json` for
`accountId`, and that field name was read out of oh-my-pi's source rather than
observed in the output of a logged-in run. `doctor` reports the probe green only
against an account that actually logged in, so an adapter shipped without one
has had its *shape* checked and not its *answer*. The other two adapters have no
equivalent gap because a token file either holds bytes or does not.

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

Adapter breadth stays at three harnesses until the base set is earned.

**Breadth before depth is how distributions die.** Ten adapters that each half-work
is a worse product than three that are verified, and it is a much worse product to
maintain, because every one of them is a standing claim about software you do not
control.

The cap moved from two to three for `omp`, and the reasoning is worth keeping
because it is the argument any fourth one has to beat. `omp` was not adopted for
coverage. It was adopted because it is the **second harness to express hooks as a
program rather than as configuration**, and that is a fact about the design omh
could not learn from one example: `opencode-plugin`'s own doc called declarative
hook config the norm and a plugin the exception, and it had exactly one data
point. Two independent harnesses agreeing that hooks are code is what turns
[the hook vocabulary](#hooks-three-maps-and-a-template) from a convenience into
the only place a hook is written once.

A third adapter that had taught omh nothing would not have been worth its
maintenance, whatever it cost to write.

See the [roadmap](roadmap.md) for when that cap lifts.
