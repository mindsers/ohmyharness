# Getting started

From nothing to a sandboxed agent that already has your rules, skills and MCP
servers. Roughly five minutes, most of it the first image build.

## Requirements

- **Docker**, running. It is currently the only verified runtime — see
  [runtime backends](design/architecture.md#runtime-backends).
- **git**, and a repository to work in. omh refuses to run outside one.
- **Rust 1.85+**, to build omh itself. Declared as `rust-version` in
  `Cargo.toml` and checked by CI, so it is a tested floor rather than the
  version that happened to be on the maintainer's machine.

## Install

```console
$ git clone https://github.com/mindsers/ohmyharness && cd ohmyharness
$ cargo build --release
$ cp target/release/omh ~/.local/bin/      # or put target/release on PATH
```

## Set up a repo

```console
$ cd ~/code/your-project
$ omh init
```

```
omh init — decided, asked nothing

  harnesses  2 (claude, opencode)
  editors    4 (code, cursor, nvim, zed)
  harness    claude  (found on your host)
  stack      rust (from Cargo.toml) → test `cargo test`, format `cargo fmt`
  memory     seeded from 2 sources:
               README.md    Launch any coding harness, in a sandbox…
               Cargo.toml   stack: rust (test `cargo test`, format `cargo fmt`)
  image      omh/claude:a1240cb9 (built)
  graph      indexing in background → omh-cache-your-project

  base set  (2026.08)
    codegraph  structural queries instead of re-grepping the repo every task

  omh why <name>  what it costs, what was considered instead, how to remove it

not yet done: recall, cost accounting.
next: omh claude
```

The base set is [a versioned file](design/base-set.md), not something buried in
the binary — `init` seeds from it and `omh why` explains from it, which is why
they cannot disagree about what you just got.

`init` **decides and reports — it never asks.** Every question is hassle omh
promised to remove, so the bar for asking one is high enough that nothing has
cleared it yet: see [derive, never interrogate](#derive-never-interrogate).

### What it actually did

```
1  repo check                     fail fast, before any work
2  ensure ~/.omh + bundled adapters + your personal profile layer
3  detect the stack; read the host for a harness *preference*
4  write <repo>/.omh/profile: AGENTS.md, hooks, mcp.json, policy
5  ensure the image                ← the real blocker; nothing runs without it
6  index the code graph            background, resumable
7  seed memory by derivation       README, manifests, git log, existing rules
8  report every decision
```

Step 4 **never overwrites what you already wrote.** If you have a `CLAUDE.md`,
init leaves it alone and merges around it.

Step 5 is the slow one — about 30 seconds the first time, cached afterwards.
`init` is not finished until `omh <harness>` works, so it builds the image
rather than deferring it to your first launch.

Step 3 reads your host only to learn which harness you *prefer*. The harness
that runs is the one inside the sandbox, which is a different installation.

## Log in

```console
$ omh auth claude personal
```

This runs the harness's **own** login flow — the same `/login` you would use
normally — and captures the result into `~/.omh/creds/claude/personal/`. You do
this once per harness per account. See [Accounts](accounts.md) for several
logins, and for why capture is keyed by harness rather than by provider.

## Run

```console
$ omh claude
```

You now have a container holding a git worktree on its own branch, with your
profile mounted where Claude Code actually reads it. Your checkout is not
mounted and the agent cannot reach it.

```console
$ omh s diff              # what the agent changed
$ omh attach              # open the same session in your editor
$ omh graph               # browse the codebase as a graph
```

Closing your terminal does not kill the agent — running `omh claude` again
reattaches to the session you left. See [Sessions](sessions.md).

## Verify it worked

Do not trust the fact that it started.

```console
$ omh doctor
```

`doctor` launches the real image with the real mounts and inspects the paths the
harness actually reads. It is the only thing in the project that can prove an
adapter is correct — a passing unit suite cannot, and [Troubleshooting](troubleshooting.md)
explains why in more detail than you probably want until the first time it
matters.

## Derive, never interrogate

Every question `init` could ask is hassle omh exists to remove, and most answers
are already lying around: manifests name the stack, git log names what you work
on, the README names the project. Derived facts also refresh when the repo
changes, instead of going stale in a config file nobody revisits.

A question earns its place only if it is **not derivable**, **actionable** (omh
does something different with the answer), and **answerable well right now**.
"What is your job" fails all three.

The strongest permitted form is *derive, then confirm* — state the hypothesis
and make it correctable:

```
! 2 stacks detected; hooks were written for all of them.
  drop the ones you do not want: .omh/profile/hooks/
```

That is not a questionnaire. init still decided; it just showed its work.

## Next

- [Commands](commands.md) — the full surface
- [Configuration](configuration.md) — the three layers, and where your settings go
- [Why a distribution](design/distribution.md) — what omh is actually for
