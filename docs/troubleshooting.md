# Troubleshooting

## `omh doctor`

```console
$ omh doctor
omh doctor: claude (in omh/claude:2133265d, account personal)

  ✓ rules      /work/CLAUDE.md
  ✓ skills     /home/agent/.claude/skills
  ✓ mcp        /home/agent/.mcp.json
  ✓ commands   /home/agent/.claude/commands
  ✓ hooks      /home/agent/.claude/settings.json
  ✓ token      /home/agent/.claude/.credentials.json (atomic write)

  all 6 checks passed — claude's adapter paths are verified
```

Run it after changing an adapter, after upgrading a harness, and any time a
session behaves as though your profile is not there.

## Why it exists

**Factual correctness is not testable in process.**

Adapters assert things about *external software* — that Claude Code reads MCP
config from `$HOME/.mcp.json`, that skills live in `~/.claude/skills`. A green
unit suite proves omh mounts a path faithfully. It cannot prove anything reads
it, and that software ships weekly.

Almost every bug this project has shipped lived at that boundary, and **not one
was catchable by the test suite.** `doctor` is the only cure.

## What it does

It launches the real image with the real mounts and inspects the **guest** paths
the adapter declares. Checking anything host-side would just re-test the staging
directory omh wrote a moment earlier, which is circular.

Capabilities the harness cannot express are **skipped, not failed** — they were
already reported as dropped at launch.

### The credential probe

This is the half no in-process test can reach: whether a token saved at a path
survives depends on how the runtime binds it, not on anything omh wrote. So the
probe attempts the temp-file-plus-rename that every token save performs.

It is **non-destructive by construction** — the file case copies the original
and renames byte-identical content back, so a successful probe changes nothing
and a failed one touches nothing. A health check that costs you your login would
be worse than no check.

Run against a file mount, it reports the real defect:

```
  ✗ token      /home/agent/.claude.json cannot be renamed over —
               a token saved here will not persist
```

### A silent probe is never a pass

If a probe produces no output, that means the sandbox never ran it. Calling that
success would make `doctor` worse than useless, so silence is a failure.

## Current status

Both shipped adapters are verified this way: `claude` passes 6 checks,
`opencode` 6 (hooks correctly skipped). The "unverified claim"
caveat is retired for these two, and any third adapter inherits the same bar.

---

## Common failures

### The harness starts but does not see my rules or skills

Run `omh doctor`. If a path fails there, the adapter is wrong — that is a bug,
and [`docs/design/adapters.md`](design/adapters.md) covers how to fix it.

If doctor passes, check whether the capability was dropped at launch:

```console
$ omh opencode
omh: opencode on omh/s01 — dropped 7 hooks (unsupported)
```

That is the harness genuinely not supporting the feature, not omh losing it.

### I logged in, but the next session is logged out

The token was written somewhere that does not persist. `omh doctor` names it:

```
  ✗ token      /home/agent/.claude.json cannot be renamed over
```

Background in [Accounts](accounts.md#mount-the-directory-never-the-token-file).

### `network omh-<repo> not found`

The plan named a per-project network that was never created. A plan must be
*runnable*, not merely well-formed — this gap made every real launch die while
every unit test passed, and it is the archetypal case for why `doctor` exists.

### `create mountpoint for /work/AGENTS.md mount: ... is outside of rootfs`

Fixed in `0.2.1`. omh mounts its rules onto `/work/CLAUDE.md` and
`/work/AGENTS.md` — paths inside the worktree mount — and left creating those
destinations to the runtime. Docker Desktop will not: `/work` is the host
worktree, so it resolves the destination back to a host path and refuses to
create a mountpoint outside the container's rootfs.

Docker creates the empty file on the host on its way out, which is why this read
as intermittent: the first launch of a session died, the second found the
leftover and worked. omh places those files itself now, before docker sees the
plan. On an older version, launching a second time is the workaround.

### `omh s rm` says the session "is not a working tree"

Worktree registration and the directory on disk disagreed. omh prunes before
adding and falls back to removing the directory outright; if you hit this,
`git worktree prune` in the main checkout clears it.

### `no usable base manifest` or `declares no base-set entries`

omh could not find a readable [base set](design/base-set.md) in `~/.omh/base`,
or the newest one names nothing. `omh init` reinstalls it.

This is deliberately loud. It used to be silent: a stray `.toml` in that
directory became the base set, `init` seeded no MCP servers and reported success
anyway, and every session came up running hooks that pointed at a server which
was not installed.

### `a hook does nothing without `run` or `inject``

A file in a `hooks/` directory is not a usable hook: truncated, half-written, or
written in a harness's words rather than omh's. Related messages from the same
check: ``unknown field `event` `` (that is Claude Code's vocabulary — see
[writing a hook](configuration.md#writing-a-hook)), ``a hook either `run`s
something or `inject`s text, not both``, and ``$` in `inject` must name a
variable`.

Loud on purpose, and checked when the file is read rather than at runtime. An
unparseable hook used to be reported as *"modified by you"* with a blank value —
a false accusation from the one command whose job is telling authorship
straight — while a launch failed hard on the same file.

### `` `graph-refresh` is a name omh ships ``

You have a hook file answering to one of omh's own names. It does not override
omh's and it does not run, so it is refused rather than left inert. Rename it —
or, if what you want is omh's version gone, switch the feature off:

```toml
# <repo>/.omh/settings.toml
[omh]
codegraph = false
```

### `a repo names servers from your catalogue, it cannot declare one`

There is an `mcp.json` in `<repo>/.omh/`. That was where servers lived before
the catalogue, and nothing reads it now. `omh config mcp add` puts one in your
catalogue; a token for this repo alone goes under `[mcp.<name>.env]` in
`.omh/settings.local.toml`.

### `keys.toml is where key templates used to live`

Rename it to `memory.toml`. omh refuses rather than falling back, because the
fallback is the disaster: the shipped defaults would silently re-key every note
written from then on, and every existing key would stop being derivable from
anything.

### `omh why` says something is not installed when it is

Check the message for a path. A layer that cannot be **read** — wrong
permissions, a broken symlink — is now an error naming the file rather than an
empty layer.

If you hit the old behaviour on an older build, the tell is that `omh init`
appears to do nothing: it uses `write_if_absent`, sees the file exists, and
leaves it alone, so the advice and the problem never meet.

### Sessions are piling up

```console
$ omh s ls
$ omh s down s01
```

N sessions is N containers. `idle_timeout` stops unused ones; see
[Sessions](sessions.md#lifecycle).

### The graph shows a project I do not recognise

Graphs are shared per repo across sessions, so `list_projects` shows all of
them. Expected, and [documented](code-graph.md#what-the-agent-can-still-see) —
your session's own project name is in `$OMH_GRAPH_PROJECT`.

### Something else

The things most likely to be wrong are the things omh asserts about other
people's software. If you are debugging one of those, start with `omh doctor`,
then read the relevant page in [design](design/adapters.md) — several of them
record a failure that looked exactly like the one you are probably having.
