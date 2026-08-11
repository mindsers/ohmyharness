# Configuration

Your setup is declared once and rendered into whatever shape each harness reads.
This page covers where it lives, how the layers merge, and how to change things.

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
