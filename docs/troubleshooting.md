# Troubleshooting

## `omh doctor`

```console
$ omh doctor
omh doctor: claude (in omh/claude:2133265d, account personal)

  ✓ AGENTS     /work/CLAUDE.md
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
`opencode` 4 (subagents and hooks correctly skipped). The "unverified claim"
caveat is retired for these two, and any third adapter inherits the same bar.

---

## Common failures

### The harness starts but does not see my rules or skills

Run `omh doctor`. If a path fails there, the adapter is wrong — that is a bug,
and [`docs/design/adapters.md`](design/adapters.md) covers how to fix it.

If doctor passes, check whether the capability was dropped at launch:

```console
$ omh opencode
omh: opencode on omh/s01 — dropped 1 subagents, 2 hooks (unsupported)
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

### `has no command string — it is not a usable hook`

A file in a `hooks/` directory is not valid: truncated, half-written, or edited
with the wrong key. The message names it.

Also loud on purpose. An unreadable hook used to be reported as *"modified by
you"* with a blank value — a false accusation from the one command whose job is
telling authorship straight.

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

N sessions is N containers. `policy.idle_timeout` stops unused ones; see
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
