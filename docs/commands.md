# Commands

```
omh init                          set this repo up
omh <harness> [args…]             claude · opencode   ← bare name = run an agent
omh attach [editor]           a   open the session in your editor, over SSH
omh graph [--stop]                browse the code graph in a browser
omh auth <harness> [account]      log in once; repeat for several accounts
omh doctor [harness]          d   verify a harness really sees your profile
omh why <thing>                   who put this here, and on what grounds
omh ls                            harnesses, editors, sessions
omh sessions ls|rm|down|diff  s   omh s ls, omh s diff s01
omh config [set|unset|edit|mcp] c omh c mcp import claude
```

## The shape of the CLI

Noun-verb groups with single-letter aliases: `omh s ls`, `omh c mcp ls`,
`omh d claude`.

**A bare name is always a harness.** Editors live under `attach`, so `omh claude`
and `omh attach zed` cannot be confused for one another — the bare slot means
exactly one thing.

That creates a hazard: since `omh <anything>` is a harness, an adapter could
shadow a real command. A `RESERVED` list prevents it, and rather than trusting
anyone to keep that list current, a test introspects the CLI definition and
fails if any command or alias is missing from it.

---

## `omh init`

Sets up the current repo. Decides everything, asks nothing, reports all of it.
Covered in [Getting started](getting-started.md#what-it-actually-did).

Safe to re-run: it never overwrites files you have edited.

## `omh <harness> [args…]`

Runs a harness inside a session, creating one if needed. Arguments after the
name are passed through to the harness untouched.

```console
$ omh claude
$ omh claude --resume
$ omh opencode
```

Running it a second time **reattaches** rather than starting a second agent.
Use `--new` to force a fresh session.

If the harness cannot express part of your profile, that is announced once at
launch rather than silently dropped:

```console
$ omh opencode
omh: opencode on omh/s01 — dropped 1 subagents, 2 hooks (unsupported)
```

`-a <account>` selects a credential set for this launch. See [Accounts](accounts.md).

## `omh attach [editor]` · `a`

Opens the session in an editor over SSH. With no argument it resolves
`$OMH_EDITOR` then `$EDITOR`, and falls back to printing every recipe:

```console
$ omh attach
session s01 is up

  ssh://omh-ohmyharness-s01/work
  ssh omh-ohmyharness-s01

  VS Code / Cursor   code --remote ssh-remote+omh-ohmyharness-s01 /work
  Zed                zed ssh://omh-ohmyharness-s01/work
  JetBrains          Gateway → SSH → omh-ohmyharness-s01
```

An editor that is not installed is **not an error** — omh says so and prints the
URL. See [Editors](editors.md).

## `omh graph [--stop]`

Serves the code graph for this repo in a browser.

```console
$ omh graph
omh: graph at http://127.0.0.1:56286
  every session's graph for this repo, in one place
  stop with: omh graph --stop
```

One service per repo, not per session, and it needs no session to exist. See
[Code graph](code-graph.md#omh-graph).

## `omh auth <harness> [account]`

Runs the harness's own login and captures the result. `account` defaults to
`default`.

```console
$ omh auth claude personal
$ omh auth claude work
```

See [Accounts](accounts.md).

## `omh doctor [harness]` · `d`

Launches the real image with the real mounts and checks the guest paths the
adapter claims. The only thing that can verify an adapter. See
[Troubleshooting](troubleshooting.md).

## `omh why <thing>`

Who put this here, and on what grounds. Needs no session and no container.

```console
$ omh why codegraph
codegraph — omh's choice, in the base set since 2026.06

  because     structural queries instead of re-grepping the repo every task
  costs       0.46s to index this repo, cold   measured 2026-08-04
              3.4 MB on disk                   measured 2026-08-04
  instead of  gitnexus            PolyForm-Noncommercial licence
              codegraphcontext    needs a Neo4j service running
              @sdsrs/code-graph   close second; needs a node runtime rather than a static binary
  installed   shared
  remove      omh config mcp rm codegraph
```

**Cost is measured; benefit is argued.** Every cost line carries the date it was
taken, and a measurement older than the entry itself is marked stale. The
`because` line is a judgment you are free to disagree with — see
[measure the cost, argue the benefit](design/trust.md#measure-the-cost-argue-the-benefit).

### Six answers, because authorship differs

| | |
|---|---|
| **omh's choice** | in the base set, and your copy matches what omh ships |
| **modified by you** | omh's entry, changed — both values are shown |
| **not installed here** | in the base set, absent from your profile. Not an error |
| **written by omh init** | derived from your repo, like `rust-format` from `Cargo.toml`. omh's writing, not omh's opinion |
| **your choice** | you added it. **No rationale is offered** — omh does not have one and will not invent one |
| **considered, not in the base set** | rejected, with the reasoning. `omh why gitnexus` explains the licence problem |

That last row is why rejections are recorded at all: without them the same
candidate gets re-litigated every time somebody rediscovers it.

A name matching nothing prints what *is* known rather than guessing — the same
rule as `omh attach emacs`.

## `omh ls`

Everything omh knows about: harnesses, editors, sessions and their state.

## `omh sessions …` · `s`

```
omh s ls              sessions, their branches and state
omh s diff [id]       what the agent changed
omh s down [id]       stop the container, keep the worktree and branch
omh s rm [id]         remove the session — the branch is kept on purpose
```

**`rm` never deletes the branch.** Unreviewed agent work must be unloseable, so
the branch outlives the session that produced it. (It is kept even when the
session produced no commits, which is [a known rough edge](design/risks.md).)

## `omh config …` · `c`

```
omh config                            effective settings, with provenance
omh config set <key> <value>          → the gitignored layer by default
omh config unset <key> [--layer]      lets the layer beneath resurface
omh config edit [--layer]             $EDITOR escape hatch

omh config mcp ls
omh config mcp add <name> <cmd> [args…] [--env K=V]
omh config mcp rm <name> [--layer]
omh config mcp import <harness> [--file] [--force]
```

Every value reports where it came from and what it beat:

```console
$ omh config
policy:
  carry_in         [".env.local"]     ← local (overrides shared)
  idle_timeout     30m                ← personal
```

MCP lives under `config` because MCP servers **are** configuration, resolved
through the same three layers as everything else. See
[Configuration](configuration.md).
