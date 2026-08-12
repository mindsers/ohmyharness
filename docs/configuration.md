# Configuration

Your setup is declared once and rendered into whatever shape each harness reads.
This page covers where it lives, how the layers merge, and how to change things.

> This page describes what ships today. The layer model is **being replaced** —
> one personal catalogue, per-project selection instead of per-project content,
> and a rules file composed with the project's own rather than mounted over it.
> See [The profile](design/profile.md) for the model and the reasoning.

## The three layers

```
~/.omh/profile/          layer 1 — personal, every project
<repo>/.omh/profile/     layer 2 — project, COMMITTED, shared with your team
<repo>/.omh/local/       layer 3 — project, GITIGNORED, yours alone
```

Every layer has the same shape:

```
AGENTS.md   skills/   mcp.json   commands/   hooks/   subagents/   policy.toml
```

Merge order is 1 → 2 → 3, later winning:

| File | How it merges |
|---|---|
| `AGENTS.md` | concatenated, in layer order |
| `skills/`, `commands/`, `subagents/`, `hooks/` | union by entry name |
| `mcp.json` | merged by server name |
| `policy.toml` | overridden key by key |

**Layer 2 is committed, so it must never contain a secret.** That is what layer
3 and [`carry_in`](#carry_in) are for.

### Why three

Two would force a choice between "shared with my team" and "mine alone" for
things that are genuinely both. The rules for a project belong in the repo; the
API key that makes one of them work does not.

## Provenance

Three layers are undebuggable without it, which is the standard complaint about
oh-my-zsh and the thing [trust](design/trust.md) exists to prevent. So every
effective value reports where it came from and what it beat:

```console
$ omh config
policy:
  carry_in         [".env.local"]     ← local (overrides shared)
  idle_timeout     30m                ← personal

mcp:
  codegraph        codebase-memory-mcp  ← shared
```

## Changing settings

```console
$ omh config set idle_timeout 45m
$ omh config unset idle_timeout            # let the layer beneath resurface
$ omh config edit --layer shared           # $EDITOR escape hatch
```

**Writes default to the gitignored layer**, so a mistyped API key cannot be
committed by accident. Writing to the committed layer works, and says so:

```console
$ omh config set carry_in '[".env"]' --layer shared
warning: the shared layer is COMMITTED — never put a secret here
```

`unset` removes the value from one layer rather than forcing a value, which is
what lets the layer beneath take over again — the difference matters when you
are overriding a team default temporarily.

## `settings.toml`

`<repo>/.omh/settings.toml` says what this repo does with **omh's own
features**:

```toml
[omh]
codegraph = false     # the server, its four hooks and its section of the rules
```

Feature names only. `graph-first = false` is refused, naming the feature it
belongs to — keeping the graph while dropping one of the things that make it
used is taking a bundle apart, not changing a setting, and "graph on, refresher
off" is a graph that quietly stops tracking the code.

Disabling is not removal: your `mcp.json` is untouched, the server is left out
of the document *this* session is given, and the next repo gets it back.
**Removing the server is the other door** — `omh config mcp rm codegraph` takes
the feature with it, hooks and rules section included, because a hook nudging
the agent toward a server that is gone is worse than no hook.

It layers like everything else — `~/.omh/settings.toml`, then this file, then
`<repo>/.omh/settings.local.toml`, which `omh init` adds to `.omh/.gitignore`.

Nothing else lives here yet. A `carry_in` written into it is refused by name
rather than read and ignored; it belongs in `policy.toml` until the layer model
below is replaced.

## `policy.toml`

| Key | Values | Meaning |
|---|---|---|
| `carry_in` | list of paths | untracked files copied into the worktree |
| `idle_timeout` | duration (`30m`, `2h`, `90s`) | stop a session nobody has used for this long. Unset means never |
| `runtime` | `auto` \| `docker` \| `sbx` | which backend to use; `auto` prefers `sbx` when present |
| `persistence` | `dtach` \| `none` | whether harnesses survive the terminal closing |
| `account` | account name | which captured login this project uses |

## MCP servers

```console
$ omh config mcp ls
$ omh config mcp add linear npx -- -y mcp-remote https://mcp.linear.app/sse
$ omh config mcp rm linear
```

MCP lives under `config` because MCP servers **are** configuration — same three
layers, same provenance, same gitignored-by-default write target.

### Importing what you already have

```console
$ omh config mcp import claude
```

Nobody retypes MCP servers they have already configured, so `import` is the
on-ramp. It is the exact inverse of the renderers, which forces a real
constraint: **every format that renders must also parse, and the pair must
round-trip.** Otherwise import silently drops fields. That is a test, not a hope
— see [Adapters](design/adapters.md#renderers).

Import never clobbers. Each server is added, recognised as already identical, or
reported as a conflict and left alone; re-running is a no-op.

Import paths expand against the **host**, deliberately using a different
expansion than everything else — the guest home would send import looking into a
filesystem that does not exist yet.

**Planned:** extending import beyond MCP to rules, skills, hooks and commands,
plus a `plugin` capability that reads Claude marketplace plugins and re-renders
them for other harnesses. See [roadmap](design/roadmap.md).

## `carry_in`

A git worktree contains only **tracked** files. No `.env`, no certs — so without
help both the agent and your IDE land somewhere that cannot run your app.

```toml
carry_in = [".env.local", "certs/"]
```

```console
$ omh claude
omh: carried .env.local
omh: carried certs/
omh: warning: carry_in lists .env.missing — not in this checkout
```

**This is the only path by which a secret reaches the agent.** That is why it is
an explicit allowlist, why omh prints what it carried, and why patterns are
validated: `carry_in` is read from a *committed* layer, so an entry like
`../../.ssh` would otherwise copy host secrets into a sandbox the agent controls.

A **missing** path is reported, never skipped — a `.env` you believe you are
carrying and are not is exactly what wastes an hour inside the sandbox.
Re-running copies only what changed; the checkout stays the source of truth.

**Copy, not symlink.** A symlink's target would have to resolve inside the
sandbox, which would mean mounting your main checkout — exposing the uncommitted
work the worktree model exists to protect.

`node_modules` is deliberately not carried. It is built in the sandbox, for the
sandbox's platform.

### Keeping the agent's `git status` clean

Carried files must not show up as untracked, or the agent is invited to commit
your `.env` onto the session branch.

omh's own `CLAUDE.md` / `AGENTS.md` are not written into the worktree at all —
they are **mounted read-only** over their declared filenames. Writing them there
made omh's staging indistinguishable from the agent's work, and `info/exclude`
could not help: gitignore semantics say nothing about a file git already tracks,
so a repo that commits its own `CLAUDE.md` saw a permanent modification nobody
made, and `omh s commit` published omh's rules over the project's conventions.

**The mount composes your project's rules rather than replacing them.** The
repo's own `AGENTS.md` is read before anything is mounted over it and joined
into the document the agent gets, after your personal layer and before the
project ones:

```
<!-- omh: personal -->           ~/.omh/profile/AGENTS.md
<!-- omh: <repo>/AGENTS.md -->   the project's own, tracked
<!-- omh: shared -->             <repo>/.omh/profile/AGENTS.md
<!-- omh: local -->              <repo>/.omh/local/AGENTS.md
<!-- omh: base:graph-rules -->   omh's own, from the base set
```

**omh's own sections come last**, generated from the
[base set](design/base-set.md) rather than written into a file. They describe
the sandbox — what git does here, where notes go, which graph answers what — and
a convention the project wrote down should not have omh's account of the box
sitting in front of it. Each is an entry: `omh why git-rules` states what it
costs and how to switch it off.

A repo that ran `omh init` before this still has those sections inside
`.omh/profile/AGENTS.md`, where init used to write them. They are composed as
part of that layer, so **all three of omh's sections currently appear twice** —
about 3.3 KB of duplicated context on every turn, and a safety notice the base
set treats as one string reaching the agent as two. Deleting the sections from
that file fixes it today; a planned migration off `.omh/profile`
([the profile](design/profile.md), P3) removes them for good.

Content comes from the worktree if the branch has a copy, otherwise from the
default branch — so a session that has just written its rules is governed by
them, and one that never had them still gets the project's. The marker says
which, because the two are not the same claim:

```
<!-- omh: <repo>/AGENTS.md -->   this branch's copy
<!-- omh: main:AGENTS.md -->     the branch has none; read from main
```

A **blank** rules file counts as no rules file. omh has to place an empty file
at each declared name for the mount to land on, so from a session's second
launch its own placeholder is sitting in the worktree — read as content it would
outrank the project's real rules under the canonical name.

A repo with `CLAUDE.md` and no `AGENTS.md` is composed anyway, and omh says
which file it read. Where both exist and differ, `AGENTS.md` wins and the other
is reported rather than silently dropped:

```console
$ omh claude
omh: composed CLAUDE.md — rename it to AGENTS.md
```

`carry_in` is for files git does **not** track. A tracked path is already in the
worktree, so listing one is a misconfiguration: omh says so at launch and does
not copy it, and `omh s commit` refuses to publish it if a running session got
one before the list was fixed.

Two traps here, both found by running it rather than reasoning about it:

- **`<worktree>/.git` is a *file***, not a directory — it points at the admin
  directory elsewhere. A test that builds a fake `.git/info` passes happily while
  the real thing does nothing.
- **git reads `info/exclude` from the *common* git dir**, not the per-worktree
  one. Checked empirically: a per-worktree exclude leaves `?? .env.local` in the
  status; the common one hides it. Worth naming the consequence — that file is
  shared with your main checkout. It is never committed, and carried paths are
  untracked there by definition, so the effect is invisible; but it is not
  scoped to the worktree, and that is a fact rather than an intention.
