# omh — oh-my-zsh for agentic coding

> Launch any coding harness, in a sandbox, with your setup already there.

[![verify](https://github.com/mindsers/ohmyharness/actions/workflows/ci.yml/badge.svg)](https://github.com/mindsers/ohmyharness/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/mindsers/ohmyharness)](https://github.com/mindsers/ohmyharness/releases)
[![licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

```console
$ omh init         # detects your stack, decides, reports. no questions.
$ omh new claude       # sandboxed, curated, your setup already inside
$ omh attach       # open that same session in your editor
$ omh graph        # browse your codebase as a graph
```

**Status: early.** `0.6.0`, one harness verified end to end. This release is the
work loop: reading a session's work, landing it in stages and staying current
with trunk, without typing `git`. Useful today if you want a sandboxed agent with
your config in it; not yet the finished distribution [the docs](docs/) describe —
[What isn't done](#what-isnt-done) is a real list, not a modesty ritual.

---

## The problem

A good agentic setup in 2026 is a pile of parts: a harness, rules, skills, MCP
servers, a sandbox, a code index, hooks, credentials. Each is a rabbit hole, so
most people stop at *"installed Claude Code, wrote a CLAUDE.md"* — not from
inability, but because assembling the rest is a research project nobody has
budget for.

The ecosystem's answer has been catalogues: [23,600+ skills and 12,700+ MCP
servers](https://claudemarketplaces.com/). That is a **problem statement**, not
an opinion. Nobody can evaluate 23,600 of anything.

omh is a **distribution**. Debian didn't write the kernel; oh-my-zsh didn't write
zsh — their genius was that installing them gave you a good system *immediately*.
The value is curation, integration and defaults, and the metric is **decisions
removed**, targeting zero.

## What you actually get

Running `omh new claude` instead of `claude` buys five things:

**A sandbox that protects your repo, not just your host.** The agent works in a
git worktree on its own branch. Your checkout is never mounted. Review with
`omh s diff`, ship with `omh s commit` and `omh s push`, discard by deleting a
branch. You never go near the worktree directory itself.

The agent gets git too — its own repository, holding one commit and none of your
history, so `stash` and `reset --hard` are its to use. `omh s commit --keep`
brings its commits onto your branch with the messages it wrote — all of them in
order, or the ones you name: `omh s01 commit --keep 1,3-4`.

**Several sessions, from one place.** `omh s` is every session with its state,
how far behind trunk it has fallen, and the files two of them are both about to
change — the collision you would otherwise meet at merge time. `omh s01` is that
same row, scoped to one.

```console
$ omh s
  s01  omh/s01  stopped  2 uncommitted
  s02  omh/s02  stopped  2 uncommitted

  s01 and s02 both change shared.rs
```

`omh s01 sync` brings trunk in, merged on the host rather than inside the
sandbox. A conflict still lands in the worktree with its markers — labelled
`main` and `s01`, so which side is yours is obvious — and `omh s commit` refuses
to land a file that still holds them. An agent that commits nothing leaves a
timeline anyway: omh photographs the worktree at the end of every turn, and
`omh s01 log --turns` reads them back.

**Your setup, in any harness.** Rules, skills, MCP servers, commands, subagents
and hooks are declared once and rendered into whatever shape each harness reads.
Switch from Claude Code to opencode and everything follows.

**A code graph that is current and actually used.** Indexed per session,
refreshed after every turn (0.14s), with hooks that point the agent at it when
it is about to grep or read a whole file.

**Your editor attached to the same place.** `omh attach` opens VS Code, Zed,
Cursor or Neovim over SSH *into the sandbox* — one dependency tree, shared with
the agent, instead of a second one on your host that silently diverges.

## Install

Requires **Docker** and **git**.

```console
$ brew install mindsers/tap/omh
```

macOS and Linux, arm64 and x86_64. `brew upgrade` keeps it current afterwards,
which is the part the script below cannot do.

Without Homebrew:

```console
$ curl -fsSL https://raw.githubusercontent.com/mindsers/ohmyharness/main/install.sh | sh
```

Picks the build for your machine, checks it against the published
`SHA256SUMS`, runs it once to confirm it works here, and moves it into
`~/.local/bin`. A failed install never replaces a working `omh`. Read it first
if you would rather — it is [one file](install.sh). Re-run it to update.

From source, which needs Rust 1.85+:

```console
$ git clone https://github.com/mindsers/ohmyharness && cd ohmyharness
$ cargo build --release
$ cp target/release/omh ~/.local/bin/      # or add target/release to PATH
```

## Quick start

```console
$ cd ~/code/your-project
$ omh init
```

```
omh init — decided, asked nothing

  harnesses  3 (claude, omp, opencode)
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

not yet done: cost accounting.
next: omh new claude
```

`init` **decides and reports** — it never asks. Every question is hassle the tool
promised to remove, and most answers are already lying around: manifests name the
stack, git log names what you work on, the README names the project.

Then log in once and go:

```console
$ omh auth claude --name personal   # runs the harness's own login, captures it
$ omh new claude                   # sandboxed, logged in, configured
```

## Commands

```
omh init                          set this repo up
omh new <harness> [-- args…]      start a session, run an agent in it
omh s01 resume [harness]          rejoin it · claude · omp · opencode
omh attach [editor]           a   open the session in your editor, over SSH
omh graph [--stop]                browse the code graph in a browser
omh auth <harness> [-n <acct>]    log in once; repeat for several accounts
omh doctor [--harness <name>] d   verify a harness really sees your profile
omh why <thing>                   who put this here, and on what grounds
omh info                          harnesses, editors, sessions
omh sessions [resume|log|diff|commit|push|sync|down|rm]  s   omh s, omh s01 diff
omh config [set|unset|edit|mcp] c you: your defaults and your catalogue
omh repo [enable|disable|set|unset] this checkout: what it uses and why
omh use|unuse <capability> <name> omh use skills tdd, omh use --all
```

Noun-verb groups with single-letter aliases. Every command is named — a bare
word is not a launch, so no adapter can shadow one. Harnesses and editors each
live under their own verb: `omh new claude`, `omh attach zed`.

## How it works

### Sessions

A session is a running container, a git worktree, and a branch — which many
harnesses take turns inhabiting.

```
       omh new claude ──┐
       omh new opencode ┼── exec ──┐
       omh attach ──────┘  (ssh)   │
                               ▼
 ┌──────────────────────────────────────────────────────┐
 │ SESSION  omh-<repo>-s01          detached, long-lived │
 │  sshd 127.0.0.1 ──── your editor attaches here        │
 │  /work  ← worktree, the code you get back             │
 │  staged profile, read-only                            │
 │  graph cache ← volume keyed by REPO, not harness       │
 └──────────────────────────────────────────────────────┘
```

Harnesses run under `dtach`, so closing your terminal doesn't kill the agent —
`omh s01 resume` puts you back in the one you left running.

### One catalogue, and it is personal

```
~/.omh/
  rules/  skills/  commands/  subagents/  hooks/   the only place these live
  mcp.json
  settings.toml                                    your defaults
```

A repo holds configuration, and one kind of content:

```
<repo>/.omh/
  settings.toml        committed: settings, and which of omh's features are on
  settings.local.toml  gitignored: your overrides, and the secrets the other must not hold
  memory.toml          committed: how the note store keys and expires
  hooks/               committed: hooks that only make sense in this repo
<repo>/AGENTS.md       the project's own rules — tracked, and actually read
```

A project cannot declare a skill, an MCP server, a command or a subagent; it
**names** ones from your catalogue. Hooks are the exception, being the one
capability whose scope is genuinely the repo — `cargo test` here, `pnpm test`
next door, one name and two bodies.

Naming them is one table, and one mechanism — an allowlist, so removing
something is deleting its name:

```toml
# <repo>/.omh/settings.toml
[use]
rules  = ["tdd", "commit-style"]   # for rules, the list is the order
skills = ["review-diff"]
mcp    = ["*"]                     # keep following the catalogue as it grows
```

Absent means everything, so upgrading changes nothing and a new checkout is
useful before it's configured. Two scopes, so two commands: `omh config` means
you, `omh repo` means this checkout, and they want opposite defaults —
[Configuration](docs/configuration.md#two-scopes-two-commands) has the rest.

### The base set is data too, and it has to justify itself

omh's opinion lives in a versioned file, not in the binary — `init` seeds from
it and `omh why` explains from it, so the two can't disagree about what is
installed or why:

```console
$ omh why codegraph
codegraph — omh's choice, in the base set since 2026.06

  because     structural queries instead of re-grepping the repo every task
  costs       0.46s to index this repo, cold   measured 2026-08-06
              index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox
  instead of  gitnexus            PolyForm-Noncommercial licence
  remove      omh config mcp rm codegraph

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

**Cost is measured; benefit is argued.** Those are different kinds of claim and
the output never blurs them — every number carries the date it was taken and the
method, while `because` is a judgement you're free to reject.

The four fields aren't a convention, they're a **test**: an entry that can't say
what it costs, what it buys, what was considered instead and how to remove it
fails the build. `omh why` answers for things omh *rejected* too, so a candidate
turned down over its licence isn't re-litigated every time someone rediscovers
it — and it offers no rationale at all for something *you* added, because it has
none. Telling those apart is the entire point.

### Adapters are data

Adding a harness is a TOML file, not a code change:

```toml
name    = "claude"
bin     = "claude"
install = "npm install -g @anthropic-ai/claude-code"

[capabilities.rules]
path   = "/work/CLAUDE.md"
also   = ["/work/AGENTS.md"]
render = "concat"

[capabilities.mcp]
path   = "/work/.mcp.json"
render = "mcp-json"
verify = "claude mcp list"   # and how omh knows it worked
ready  = "Connected"
```

**An absent key means the harness cannot do that thing.** Degradation is a
missing map entry rather than special-case logic, and it is announced once:

```console
$ omh new opencode
omh: opencode on omh/s01 — dropped hooks: graph-first (no `search` tool),
     graph-orient (no `session-start` moment),
     graph-read (no way to inject text before a tool runs)
```

Editors work the same way — `~/.omh/editors/zed.toml` is four lines.

### The code graph

Every session is indexed into a graph ([codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp),
MIT, a static binary with no runtime or database). Four hooks make it something
the agent uses rather than something merely installed:

| Hook | When | Cost | Buys |
|---|---|---|---|
| `graph-orient` | session start | 2.3 KB | modules, layers, boundaries, entry points |
| `graph-first` | before Grep/Glob | 243 B | structural questions in one call |
| `graph-read` | before Read | 0 unless it speaks | **1,511 bytes** for one symbol, not the whole module |
| `graph-refresh` | end of turn | 0.14s | a graph describing the code as it is *now* |

They are nudges, never walls: grep is right for a literal string, and a hook that
blocks correct work gets disabled. `graph-read` stays silent unless a symbol
lookup would genuinely be cheaper.

```console
$ omh graph
omh: graph at http://127.0.0.1:56286
  every session's graph for this repo, in one place
```

### Credentials

```console
$ omh auth claude --name personal
$ omh auth claude --name work
$ omh -a work new claude      # or, per project: omh set account work
```

Accounts are per harness, and which one a session uses is a project-level
setting — because that is how it actually varies: this repo is work, that one is
personal. Ambiguity is an error, never a guess: two identities and no stated
preference stops the launch rather than sending work traffic through a personal
account.

### Files your worktree needs

A worktree holds only tracked files — no `.env`, no certs, so the agent lands
somewhere that cannot run your app.

```toml
carry_in = [".env.local", "certs/"]
```

An explicit allowlist, because **this is the only path by which a secret reaches
the agent**. A listed path that doesn't exist is reported, not skipped.

It is for files git does **not** track. A tracked path is already in the
worktree, so listing one would replace the branch's copy with whatever your
checkout holds right now — usually an uncommitted edit, on the one path a secret
travels. omh says so at launch and does not copy it.

## Verify it yourself

`omh doctor` is the only thing that can prove an adapter is right. It launches
the real image with the real mounts and inspects the paths the harness actually
reads:

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

A green unit suite proves omh mounts a path faithfully; it proves nothing about
whether anything reads it. That gap is what `doctor` closes.

## What isn't done

| | |
|---|---|
| **Memory** | mostly [built](docs/commands.md#omh-memory-) — the store, its schemas, retrieval, the team layer and `remember` / `recall` as MCP tools all ship. What remains is hub pages, whose lint needs a threshold the design refuses to let anyone guess. |
| **Cost accounting** | each base-set entry should report what it injects, in bytes, so the set has a reason to shrink. Not a benchmark — [here's why](docs/design/trust.md#measure-the-cost-argue-the-benefit). |
| **`omh eject`** | a credible exit: write out the raw per-harness config and step aside. |
| **`sbx` backend** | the trait exists and declares capabilities; the spike that resolves file-mounts, guest paths and IDE attach has not run. Docker is the only verified runtime. |
| **Egress allowlist** | designed, not wired. |
| **Other harnesses** | `opencode` and `omp` pass `doctor`, but only `claude` has been driven for real work. |

Known rough edges: the graph store is shared across sessions of one repo, so an
agent can query another session's graph (mitigated, not prevented);
`.claude.json` is a file mount that cannot be atomically replaced; `omh s rm`
drops a session branch only when it has no commits.

## Contributing

```console
$ cargo test -- --include-ignored   # the ignored set needs a container runtime
$ omh doctor                       # the only thing that verifies an adapter
```

Two rules do most of the work, and [`CONTRIBUTING.md`](.github/CONTRIBUTING.md)
has the rest with the invariant list. **[TDD,
always](.github/CONTRIBUTING.md#tdd-always)** — a green suite is not evidence on
its own, so a bug fix's test goes red before the fix lands. And **adapters
assert facts about external software**: almost every bug this project has
shipped lived at that boundary and none was catchable in-process, so if you
change an adapter, run `omh doctor`.

## Documentation

Full docs live in [`docs/`](docs/README.md), which indexes them.

Start with [Getting started](docs/getting-started.md) — install to first session.
[Commands](docs/commands.md) is the whole surface and what each one prints;
[Configuration](docs/configuration.md), [Sessions](docs/sessions.md),
[Accounts](docs/accounts.md) and [Editors](docs/editors.md) cover the parts in
depth.

Read the [design pages](docs/README.md#understanding-omh) before changing
architecture — most record something that was tried and cost something, and
[Risks](docs/design/risks.md) states plainly what is still weak.

## Licence

MIT — see [`LICENSE`](LICENSE).

One dependency, `option-ext` (transitive via `dirs`), is MPL-2.0; everything else
in the tree is Apache-2.0, MIT, BSD, ISC or Unicode-3.0.
