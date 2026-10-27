---
title: Adapters
description: Add a harness or an editor to omh with one TOML file, and every field an adapter can declare.
---

Add a harness or an editor to omh with one TOML file. A harness is data, not a
code change.

**Status: built.** Four adapters ship — `claude`, `codex`, `opencode` and `omp`.
Only `claude` has been driven for real work; the other three pass `omh doctor`,
which proves their paths and nothing about their behaviour.

## Adding a harness

<div class="steps">

1. Write `~/.omh/adapters/<name>.toml`.

   Declare only what the harness genuinely supports: an
   [absent key](#capabilities-are-optional-keys) means it cannot do that thing.
   Every field is in [Adapter fields](#adapter-fields) below, and the four
   [bundled adapters](https://github.com/mindsers/ohmyharness/tree/main/adapters)
   are worked examples.

2. Check it inside a real sandbox.

   ```console
   $ omh doctor --harness <name>
   ```

3. Fix whatever it says, and run it again.

</div>

**Step 2 is not optional, and it is not a formality.** Adapter facts are
unverified claims about external software that ships weekly, and they break
*silently* — a wrong path means the harness starts fine and simply never sees
your profile. A green unit suite proves omh mounted a path faithfully. It cannot
prove anything read it. See [Troubleshooting](/guides/troubleshooting/#why-it-exists).

### The bar for shipping one

All four bundled adapters are verified by `doctor` against a real container,
hooks included on every one. Any fifth adapter inherits the same bar.

- **Passing `doctor` is not being proven.** It means the paths are right, not
  that the harness has been driven for real work — only `claude` has. That
  distinction is deliberate, is repeated wherever the claim appears, and applies
  to `opencode`, `omp` and `codex` in the same words.
- **The check count is not an adapter fact.** It varies with the capabilities
  your profile declares and with whether a login has been captured for that
  harness.

<details>
<summary>How each adapter's hooks are checked, and omp's one weaker claim</summary>

`claude`'s and `codex`'s hooks are configuration files, which `doctor` expects
to find and to find non-empty. opencode's and omp's are generated plugins rather
than config files, so they are checked by `node --check` inside the sandbox
rather than by existing.

`omp` carries one claim weaker than the others, and it is written here rather
than left in a comment: its `token-probe` greps `omp usage --json` for
`accountId`, and that field name was read out of oh-my-pi's source rather than
observed in the output of a logged-in run. `doctor` reports the probe green only
against an account that actually logged in, so an adapter shipped without one
has had its *shape* checked and not its *answer*. The other three adapters have
no equivalent gap, because a token file either holds bytes or does not.

</details>

## Adding an editor

Same principle, smaller file. Write `~/.omh/editors/<name>.toml`:

```toml
# ~/.omh/editors/zed.toml
name = "zed"
bin  = "zed"
args = ["$URL"]
```

| Field | What it says | Default |
|---|---|---|
| `name` | the word `omh sNN attach <editor>` takes: letters, digits, `-` and `_` | required |
| `bin` | the executable on your **host** — an editor is not installed in the sandbox | required |
| `args` | its arguments; `$URL` becomes `ssh://<alias>/work` and `$ALIAS` the session's SSH alias | required |

Editors attach from outside over SSH rather than running inside the sandbox, so
they have no capabilities to declare. See [Editors](/guides/editors/).

## Adapter fields

The `claude` adapter, shortened:

```toml
name    = "claude"
bin     = "claude"
version = "2.1.300"
install = "npm install -g @anthropic-ai/claude-code@{{version}}"
creds   = ["$HOME/.claude/", "$HOME/.claude.json"]
token   = ["$HOME/.claude/.credentials.json"]

[capabilities.rules]
path   = "/work/CLAUDE.md"
also   = ["/work/AGENTS.md"]
render = "concat"

[capabilities.mcp]
path   = "/work/.mcp.json"     # project-scoped; this harness reads no other
render = "mcp-json"
import = "$REPO/.mcp.json"     # host-side, for `omh settings mcp import`
verify = "claude mcp list"     # what `omh doctor` asks the harness itself
ready  = "Connected"           # what that answer calls a server it loaded
```

### The harness

| Field | What it says | Default |
|---|---|---|
| `name` | the harness's name, as `omh new`, `omh auth` and `omh doctor --harness` take it. Letters, digits, `-` and `_` only — it is joined into paths | required |
| `bin` | the executable to run inside the sandbox | required |
| `install` | the command that installs `bin` into the image; it may interpolate the pin as `{{version}}` | required |
| `version` | the harness version this adapter pins. `install` must interpolate it | no pin |
| `checksums` | SHA-256 sums keyed by `uname -m` (`x86_64`, `aarch64`), for a harness delivered as a release binary rather than an npm package | none |
| `creds` | the paths holding credentials, captured by `omh auth` and remounted on every launch. **A trailing `/` declares a directory** | none |
| `token` | the file(s) whose contents prove a login — see [below](#token-or-token-probe-never-both) | none |
| `[token-probe]` | `run` and `ready`: ask the harness whether it is logged in, where no file can say | none |
| `login` | what you have to do once the harness opens, to log in | none |
| `transcripts` | where the harness writes its transcripts. omh mounts a per-session directory over it, so `omh sNN` can read what the agent did and what it cost | none, and omh says *not recorded* rather than showing an empty session |
| `[tools]` | how the harness names `edit`, `read`, `shell` and `search` — see [Hooks](#hooks-three-maps-and-a-template) | none |
| `[capabilities.<class>]` | one binding per capability the harness can express | none |
| `[policy]` | the harness's own policy file, which omh fills on every launch — see [below](#policy-what-a-session-may-load) | none |

`creds` names a directory rather than the token file because a bind-mounted
*file* cannot be replaced by rename, and every tool saves its token by writing a
temporary file and renaming it over.

### A capability binding

| Field | What it says | Default |
|---|---|---|
| `path` | the guest path the harness reads; `$HOME` expands per image | required |
| `also` | more guest paths pointed at the same content | none |
| `render` | how omh produces what the harness reads — see [Renderers](#renderers) | required |
| `import` | the host-side path `omh import` and `omh settings mcp import` read from: where *you* keep it, not where the container reads | none — `omh import` then needs `--from` |
| `verify` | the harness's own command for listing what it loaded, run by `omh doctor` inside the sandbox | not asked |
| `ready` | the word `verify`'s output uses for a server the harness took up | not asked |
| `events`, `fields`, `fields-by-tool`, `inject`, `refuse` | `hooks` only — see [Hooks](#hooks-three-maps-and-a-template) | none |

### `[policy]`: what a session may load

```toml
# adapters/codex.toml
[policy]
path   = "/etc/codex/requirements.toml"
format = "codex-requirements"
mask   = ["$HOME/.codex/cloud-config-bundle-cache.json"]
```

The account directory is writable and shared by every session on it, so
anything a session adds there — an MCP server through `codex mcp add` or
written into `~/.claude.json`, a hook — would load in every later session. A
harness's policy layer is read from `/etc`, which omh fills from the launch
with a read-only file, and nothing a session writes there outlives its
container:

| Field | What it says | Default |
|---|---|---|
| `path` | the guest path the harness reads its policy from: a file under `/etc/`, absolute, with no `..`, not under a `creds` or `transcripts` path and not a capability's own path — a literal, never expanded | required |
| `format` | `codex-requirements` or `claude-managed` | required |
| `mask` | guest paths strictly inside one of `creds`' directories, with no `..`, that a session could write and the harness would trust on its next start; each is hidden behind an empty read-only file | none |
| `copy` | `creds` file entries each session gets its own copy of, seeded from the account's without what names, switches or grants a server, and never written back — see [`~/.claude.json` is each session's own](/guides/accounts/#claudejson-is-each-sessions-own). `claude-managed` only | none |

The policy admits exactly the MCP servers the launch staged, by command and
arguments compared exactly — and by name too, on Codex. Neither harness
compares `env`, which is still open ([risk 1b](/concepts/risks/#1b-the-account-directory-is-writable-and-every-repo-shares-it)). It is written even when
nothing is staged, because a harness with no allowlist admits anything. On
Codex, what the policy stops in your checkout or account — a repo's own
`.codex/config.toml` servers or hooks, an enterprise bundle cache — is named at
launch and by `omh doctor`. `omh eject` writes the policy itself under
`system/`.

- **codex** — `requirements.toml`: the `[mcp_servers]` allowlist, an entry
  that holds every plugin's servers to a list of their own — empty for any
  plugin not named — `allow_managed_hooks_only` (omh's hooks are in the system
  layer and still run; a repo's `.codex/config.toml` hooks do not, by Codex's
  source), and a pinned `chatgpt_base_url`. An enterprise account's cloud
  bundle cache is masked, so Codex fetches the bundle at every start — and, by
  Codex's source, offline it does not start; no enterprise account has been
  run.
- **claude** — a `managed-settings.d` drop-in: `allowManagedMcpServersOnly`,
  `allowedMcpServers`, and `disableClaudeAiConnectors`. claude.ai connectors
  are switched off in a session (read in the 2.1.300 binary; not measured):
  one is matched by its URL, and allowlisting that URL would let a session put
  its own server there. An organisation's own managed list replaces omh's
  (Claude Code's docs; not measured).

### What omh refuses when it loads an adapter

omh enforces each of these, with an error naming the adapter:

- a key it does not know — see [Unknown fields are rejected](#unknown-fields-are-rejected)
- a `name` that is anything but letters, digits, `-` and `_`
- a `version` that `install` never interpolates, or a `{{version}}` with no `version` to fill it
- an `install` that runs `sha256sum -c` while declaring no `checksums`
- `token` and `token-probe` together
- no capabilities at all — it would launch a harness that can see none of your profile
- `hooks` with no `[tools]`, `[tools]` with no `hooks`, `hooks` with no `events`, and a hook map on any capability but `hooks`

One rule is yours to keep: **declare `verify` and `ready` together.** `doctor`
runs the check only when both are set, and skips half a pair without a word.

<details>
<summary>Why a pin and a checksum are checked as the adapter loads</summary>

A pin can be inert two ways, and both compile and both ship an image nobody
meant: a `version` the command has no `{{version}}` to receive installs the
latest release while claiming a pin, and a `{{version}}` with nothing to fill it
installs a package literally called `{{version}}`.

A checksum verified but never declared is a check against nothing: the install
would `sha256sum -c` a file it also wrote, or fail on a sum that is not there.
If the command checks one, the field carries it — which is where `omh doctor`
reads it too.

The name is checked because it is a path: omh resolves a harness by joining the
word into `<dir>/<word>.toml`, and keys the credential directory on it. A name
of `..` would name a file outside the catalogue, whose `install` becomes a line
of a host `docker build`.

</details>

## Capabilities are optional keys

The profile carries the **superset** of what any harness might express. An
adapter declares which parts its harness can actually express, and where. The
capability classes are `rules`, `skills`, `mcp`, `commands`, `subagents` and
`hooks`.

**An absent key means the harness cannot do that thing.** Degradation is a
missing map entry, not special-case logic — which is why adding a harness needs
no branch anywhere in the codebase.

It is announced once, at launch, rather than silently swallowed:

```console
$ omh new codex
codex on omh/s02 — dropped 0 commands, 0 subagents (unsupported); dropped hooks: graph-first (no `search` tool), graph-read (no `read` tool)
```

### Unknown fields are rejected

Adapters parse with `deny_unknown_fields`. Without it, a stale adapter parses
cleanly with **zero** capabilities and every harness silently degrades to
nothing — the worst possible failure for a tool whose entire promise is that
your setup is already there.

## `token` or `token-probe`, never both

Declare one way to prove a login. **omh refuses an adapter that declares both**,
at load.

| Declare | When | It passes when |
|---|---|---|
| `token` | the harness writes its login to a file omh can stat | the file holds bytes |
| `[token-probe]` | the credentials are somewhere omh cannot read — omp keeps them in SQLite | `run` *succeeds* inside the sandbox and its output names `ready` |

```toml
[token-probe]
run   = "omp usage --json"   # asked inside the sandbox, where the credentials are
ready = "accountId"          # what that answer says when the login is real
```

**`omh doctor` settles a probe, not `omh auth`.** `omh auth` tears the container
down before it can ask, so on such a harness it records the account and says the
login is **unconfirmed**.

<details>
<summary>Why a file beats a probe, and why declaring both is refused</summary>

`token` is the better answer wherever it applies, because a stat needs no
container and cannot be wrong about what it saw.

Some harnesses keep credentials somewhere omh cannot read. There the only file
that could be named is created by the harness *starting*, so declaring it would
report a login for every session that merely opened the tool. Such an adapter
declares a probe instead.

The pair is refused at load, not resolved by precedence: an adapter declaring
both leaves omh with two answers to one question and no rule for which wins. It
is the same shape as `verify`/`ready` one level up — when the claim is about
software omh did not write, ask that software — and the same rule applies to the
answer: the probe passes only when the command succeeds *and* its output names
`ready`, because a harness that errors out will happily print the marker inside
its error text.

</details>

## Renderers

| Render | Used by | Effect |
|---|---|---|
| `dir` | skills, commands, subagents | link the catalogue's entries by name, mount read-only |
| `concat` | rules | join the sections into one document, staged and mounted read-only |
| `mcp-json`, `codex-toml`, `opencode-json` | mcp | reshape the canonical server list |
| `claude-settings` | hooks | reshape the canonical hook list |
| `codex-hooks` | hooks | write Codex's `[[hooks.<Event>]]` tables into its system layer, `/etc/codex/config.toml`, which Codex runs as managed |
| `opencode-plugin`, `omp-plugin` | hooks | generate a JavaScript module |

**Nothing is written into the worktree.** `concat`, and any `path` under `/work`
whichever renderer produced it, is mounted: omh places the mountpoint and binds
over it, so a project's own file is hidden for the session and returned
untouched.

**Every renderer must round-trip through its parser.** `omh settings mcp import`
is the exact inverse of rendering, so a format that renders but parses lossily
means import silently drops fields. That is a test, not a hope.

<details>
<summary>Why the two plugin renders are separate, and why concat stopped writing to the worktree</summary>

Two of the renders emit a **program**, and they are separate renders rather than
one parameterised by the harness. They agree on the shell bridge — how a hook
body runs, how a failed predicate is reported, how `$OMH_*` expands — and on
nothing else: opencode registers one object of named handlers and reads a call's
arguments off `output` or `input` depending on the moment, while omp gives each
hook its own `pi.on(...)` registration and hands every handler an `event`.
Sharing the generator would mean a `match` on the harness in every line of it.

`concat` once wrote into the worktree. That made omh's staging
indistinguishable from the agent's work, and carried omh's rules into users'
pull requests.

</details>

## `verify` and `ready`: asking the harness

`verify` is the harness's own command for listing what it loaded; `ready` is the
word its output uses for a server the harness has taken up. `omh doctor` runs
the one and greps for the other, on the same line as the server's name.

| Harness | `verify` | `ready` | What a pass proves |
|---|---|---|---|
| `claude` | `claude mcp list` | `Connected` | the server is running |
| `codex` | `codex mcp list` | `enabled` | Codex's parsed config holds the server — the file was read, not that the server starts |
| `opencode`, `omp` | — | — | not asked |

That is as much as each harness can say. The pass line names the word, so the
two do not read as the same fact.

**Both are optional.** An adapter that declares neither is simply not asked —
the same degradation as any absent key.

<details>
<summary>Why a name in the list is not enough, and what an unasked path cost</summary>

A `path` is a claim about software omh did not write, and the suite cannot check
it. What leaving it unasked costs is on record: the `claude` binding pointed
`mcp` at `$HOME/.mcp.json`, which nothing reads, and every check omh had stayed
green while no session ever loaded a server.

Matching the name alone is not enough, and that is the point of `ready`. A
project-scoped document Claude Code has not been told to trust is listed in full
and loaded not at all, so a name-only check passes in exactly the state the
feature is broken in.

</details>

## Hooks: three maps and a template

A hook is authored in omh's vocabulary and translated when it is staged, so the
adapter is where a harness's spelling lives — the same rule as every `path`.

```toml
# Adapter-level: hooks match on these, and the Agent Skills standard and
# subagent frontmatter both carry harness tool names too. One vocabulary.
# No `search`: Claude Code's native builds reach grep and glob through `Bash`.
[tools]
edit   = "Edit|Write|MultiEdit"
read   = "Read"
shell  = "Bash"

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

| Key | Maps |
|---|---|
| `[tools]` | omh's tool words — `edit`, `read`, `shell`, `search` — to the harness's tool names. Adapter-level |
| `events` | omh's moments — `session-start`, `turn-end`, `before-tool`, `after-tool` — to the harness's names. **Required** on a `hooks` binding |
| `fields` | where the harness keeps `tool-file` and `tool-command`, in the renderer's own language |
| `fields-by-tool.<tool>` | a field that lives somewhere else on one tool: a complete expression, used verbatim. A tool with no entry falls back to `fields` |
| `inject` | the template for advisory text the agent reads; the call goes ahead |
| `refuse` | the template that blocks a call, with a reason |

In a template, `{{text}}` receives a shell word and `{{event}}` the harness's own
word for the moment — some protocols name the event back in the payload.

**An absent entry means this harness has no such thing** — the capability map's
rule, one level down. The hooks wanting it are dropped **by name**, saying what
they asked for, and the rest still ship: a harness with `turn-end` and no
`before-tool` keeps most of them, and a count per capability would report that
as "hooks: 0".

**`fields` is read in the renderer's own language.** Claude Code's and Codex's
are jq paths, because they hand a hook its payload on stdin; opencode's and
omp's are property names, because a plugin receives the tool's arguments as an
object. An adapter says which by its `render`.

**Advising and blocking are two templates, and either may be absent.** A harness
that can do one and not the other drops the hooks wanting the other **by name** —
never substituting, in either direction.

**`events` is required on a `hooks` binding, and these maps are refused on any
other capability**, where nothing would read them. omh enforces both.

<details>
<summary>Why there is no single syntax for fields, why advise and block stay apart, and what fields-by-tool is for</summary>

The `fields` map answers "where does this harness keep the file path", and
there is no single syntax for which that is true — so each render reads it in
the language it emits.

Advising and blocking are one field apart on Claude Code and genuinely
different mechanisms elsewhere: on opencode advisory text has no channel before
a tool runs, and the only way to speak there is to throw, which blocks.

Without `events` nothing can be expressed: every hook is dropped, and the
harness receives an empty settings document — indistinguishable from a harness
that declares no hooks, except that this one claimed to have them.

`fields-by-tool` is the escape hatch `fields` cannot express. omp's `edit` is
the reason it exists: its path is not a property at all but a substring of one
`input` string, one or more `[PATH#TAG]` sections. Codex's `edit` is the same
problem in another shape — its file is read out of the patch's first
`*** Update File:`, `*** Add File:` or `*** Delete File:` header. On both, a call
touching several files shows a guard only the first.

</details>

## Breadth is capped on purpose

Adapter breadth is capped until the base set is earned. Four harnesses ship: the
cap moved from two to three for `omp` (#37, in 0.6.0), and `codex` came in as the
fourth in 0.10.0 (#107), adopted so that "the second-most-used harness is in".

**Breadth before depth is how distributions die.** Ten adapters that each
half-work is a worse product than three that are verified, and it is a much
worse product to maintain, because every one of them is a standing claim about
software you do not control.

See the [roadmap](/project/roadmap/) for when that cap lifts.

<details>
<summary>Why omp moved the cap, and the argument a further one has to beat</summary>

`omp` was not adopted for coverage. It was adopted because it is the **second
harness to express hooks as a program rather than as configuration**, and that
is a fact about the design omh could not learn from one example:
`opencode-plugin`'s own doc called declarative hook config the norm and a plugin
the exception, and it had exactly one data point. Two independent harnesses
agreeing that hooks are code is what turns
[the hook vocabulary](#hooks-three-maps-and-a-template) from a convenience into
the only place a hook is written once.

A third adapter that had taught omh nothing would not have been worth its
maintenance, whatever it cost to write. `codex`, the fourth, came in for
coverage, and #107 says so.

</details>

## Next

<div class="cards">

- [Troubleshooting](/guides/troubleshooting/#omh-doctor)\
  What each row of `omh doctor` means, and what to do when one fails.
- [Configuration](/reference/configuration/)\
  The catalogue an adapter renders from, and what a repository takes from it.
- [Editors](/guides/editors/)\
  Open a session in VS Code, Zed, Cursor or Neovim.
- [Roadmap](/project/roadmap/)\
  When the cap on harnesses lifts.

</div>
