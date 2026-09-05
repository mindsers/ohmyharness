# omh — the safe way to run an agent against your repo

> A sandboxed session, your setup already in it, and a branch you can review.

[![verify](https://github.com/mindsers/ohmyharness/actions/workflows/ci.yml/badge.svg)](https://github.com/mindsers/ohmyharness/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/mindsers/ohmyharness)](https://github.com/mindsers/ohmyharness/releases)
[![licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

```console
$ omh init              # detects your stack, decides, reports. no questions.
$ omh new claude        # sandboxed, curated, your setup already inside
$ omh sessions attach   # open that same session in your editor
$ omh graph             # browse your codebase as a graph
```

**Status: early, and one harness deep.** `0.9.0`. **Claude Code is the only
harness anyone has done real work through.** `opencode` and `omp` pass
`omh doctor`, which proves their paths are right and nothing whatever about
their behaviour — so *declare once, switch harness* is the shape the
architecture is built for and not yet a thing anybody has done.

What is verified is the loop below: a sandboxed session with your config
already inside it, and a branch you can read before it touches your checkout.
This release closes the two silent failures that would have got worse with
every user — two checkouts of the same name no longer share sessions, and the
carried-file scan now says what it could not read — and adds
[`omh eject`](docs/commands.md#omh-eject-harness---to-dir), which writes the
config out and steps aside.

[What isn't done](#what-isnt-done) is a real list, not a modesty ritual.

---

## What you actually get

Running `omh new claude` instead of `claude` buys five things:

**A sandbox that protects your repo, not just your host.** The agent works in a
git worktree on its own branch. Your checkout is never mounted. Review with
`omh sessions diff`, ship with `omh sessions commit` and `omh sessions push`,
discard by deleting a branch. You never go near the worktree directory itself.

> **`sessions` is the command; `s` is its alias, and a session id can lead the
> line.** So `omh sessions --session s01 diff`, `omh s -s s01 diff` and
> `omh s01 diff` are three spellings of one thing. The short ones are used from
> here on.

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

**Your setup, declared once.** Rules, skills, MCP servers, commands, subagents
and hooks live in one place and are rendered into whatever shape each harness
reads — a harness is a TOML file, not a code change. Whether that survives
switching harness is the claim this project has not yet earned: `opencode` and
`omp` render and pass `omh doctor`, and nobody has worked in them. Believe the
part you can check, which is that your setup is in one place and
[`omh eject`](docs/commands.md#omh-eject-harness---to-dir) hands it back.

**A code graph that is current and actually used.** Indexed per session,
refreshed after every turn (0.14s), with hooks that point the agent at it when
it is about to grep or read a whole file.

**Your editor attached to the same place.** `omh s attach` opens VS Code, Zed,
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

  harness    claude  (found on your host)
  image      omh/claude:b4edd8fd15d4d669 (already built)
  image      omh/claude:8eae0d5c1511fa89 (this repo's toolchain)
  stack      rust (from Cargo.toml)
  hooks      2 selected  (4 more in your catalogue)
  provision  rust/linker
  provision  rust/toolchain
  memory     2 notes written, 0 already there
  graph      indexing in background → omh-cache-your-project-a8c4d1cd

  catalogue  /Users/you/.omh
  this repo  /Users/you/code/your-project/.omh  (committed)

  base set (2026.08)
    codegraph  structural queries instead of re-grepping the repo every task
    memory     what a session learned outlives it — a removed session leaves no transcript to grep

  omh why <name>  what it costs, what was considered instead, how to remove it

  next
    omh new claude  start a session
    omh s resume    rejoin it later
    omh s attach    open it in your editor
```

`init` **decides and reports** — it never asks. Every question is hassle the tool
promised to remove, and most answers are already lying around: manifests name the
stack, git log names what you work on, the README names the project.

Then log in once and go:

```console
$ omh auth claude --name personal   # runs the harness's own login, captures it
$ omh new claude                    # sandboxed, logged in, configured
```

`omh new` hands you the harness. Close the terminal whenever you like — it runs
under `dtach`, so the agent keeps going and `omh s01 resume` puts you back.

### Getting the work back

When the agent has done something, you are on the host looking at a branch:

```console
$ omh s
  s01  omh/s01  up  2 uncommitted

$ omh s01 diff
 README.md   | 2 +-
 src/main.rs | 6 +++++-
 2 files changed, 6 insertions(+), 2 deletions(-)

$ omh s01 commit -m "Add a greeting"
committed to omh/s01 (1 commit on the branch)

$ omh s
  s01  omh/s01  up  1 to push
```

`omh s01 diff -p` is the patch, `omh s01 log` is what the agent committed inside
the sandbox, and `omh s01 push` puts the branch on origin under a name a
reviewer can read. Nothing here touches your checkout — the work is on
`omh/s01` until you merge it, and `omh s01 rm` throws the whole session away.

## The problem

A good agentic setup in 2026 is a pile of parts: a harness, rules, skills, MCP
servers, a sandbox, a code index, hooks, credentials. Each is a rabbit hole, so
most people stop at *"installed Claude Code, wrote a CLAUDE.md"* — not from
inability, but because assembling the rest is a research project nobody has
budget for.

The ecosystem's answer has been catalogues: [23,600+ skills and 12,700+ MCP
servers](https://claudemarketplaces.com/). That is a **problem statement**, not
an opinion. Nobody can evaluate 23,600 of anything.

## Where this is going

omh is built to be a **distribution**. Debian didn't write the kernel;
oh-my-zsh didn't write zsh — their genius was that installing them gave you a
good system *immediately*. The value is curation, integration and defaults,
and the metric is **decisions removed**, targeting zero.

That is the intent, and it is why the architecture looks the way it does:
adapters are data so a harness is a TOML file, the base set is a versioned
manifest so `omh why` and `omh init` cannot disagree, and every entry has to
say what it costs. **It is not yet a description of what you get.** The
curated set is eleven entries across three features, with five rejections
recorded beside them, and one person picks all of it —
[risks](docs/design/risks.md) names that, rather than any technical problem,
as the thing most likely to kill this project. The submission
standard is [published](docs/design/base-set.md#proposing-an-entry) so it does
not have to stay that way.

## Commands

```
omh init                          set this repo up
omh new <harness> [-- args…]      start a session, run an agent in it
omh auth <harness> [-n <name>]    log in once; repeat for several accounts

omh set <key> <value>             a setting, or a feature on|off — this repo
omh unset <key>                   drop it, or hand the feature back to omh
omh use|unuse <capability> <name>
                                  omh use skills tdd · omh use --all
omh settings [set|unset|edit|mcp] …
                                  you: your defaults, and your MCP servers

omh info [--repo]                 this machine · --repo, this checkout
omh why <thing>                   who put this here, and on what grounds
omh doctor [--harness <name>]     prove a harness really sees your profile
omh graph [--stop]                browse the code graph in a browser

omh memory [remember|stale|lint|rm|promote] …
                                  notes that outlive a session
omh import <capability> <harness>
                                  bring a setup you already have into omh
omh eject <harness> --to <dir>    write out the raw config and step aside
```

### Sessions

The loop the rest of it exists for. `sessions` is the noun, and everything below
takes the session id in front of it:

```
omh s                             every session: state, drift, collisions
omh s01                           that one, and what to run next

omh s01 log [--turns]             what the agent committed in the sandbox — or,
                                  if it committed nothing, what omh photographed
omh s01 diff [n] [-p]             what changed: the whole session, or checkpoint n
omh s01 commit [-m …]             land its work as one commit on the branch
omh s01 commit --keep [n,m-o]     or replant the agent's own commits, with the
                                  messages it wrote — all, or the ones you name
omh s01 push [name]               that branch to origin, named for a reviewer
omh s01 sync                      bring trunk in, merged on the host

omh s01 resume [harness]          rejoin it, running the harness it ran before
omh s01 attach [editor]           open it in your editor, over SSH
omh s01 down                      stop the sandbox; the worktree and branch stay
omh s01 rm                        remove it — container, worktree and staging
```

Three shorthands, and they compose: `sessions` is `s`, a leading session id
replaces `-s`, and every session verb takes one. So `omh sessions -s s01 diff`,
`omh s -s s01 diff` and `omh s01 diff` are the same line — use the last.

Everything after `--` belongs to the harness: `omh new claude -- --verbose`
passes `--verbose` to claude, not to omh.

`--dry-run` runs everything and writes nothing; a command that cannot yet
answer it refuses the flag rather than running.

**[Commands](docs/commands.md) is the full reference** — every flag, and what
each command prints.

## How it works

### Sessions

A session is a running container, a git worktree, and a branch — which many
harnesses take turns inhabiting.

```
    omh new claude ───────┐
    omh new opencode ─────┼── exec ──┐
    omh s attach ─────────┘  (ssh)   │
                                     ▼
 ┌───────────────────────────────────────────────────────┐
 │ SESSION  omh-<repo>-s01          detached, long-lived │
 │  sshd 127.0.0.1 ──── your editor attaches here        │
 │  /work  ← worktree, the code you get back             │
 │  staged profile, read-only                            │
 │  graph cache ← volume keyed by REPO, not harness      │
 └───────────────────────────────────────────────────────┘
```

Harnesses run under `dtach`, so closing your terminal doesn't kill the agent —
`omh s01 resume` puts you back in the one you left running.

### One catalogue, and it is personal

```
~/.omh/
  rules/  skills/  commands/  subagents/  hooks/   the only place these live
  mcp.json
  default.toml                                     what a new repo starts from
```

A repo holds configuration, and one kind of content:

```
<repo>/.omh/settings.toml        committed: settings, and which of omh's features are on
<repo>/.omh/settings.local.toml  gitignored: your overrides, and the secrets the other must not hold
<repo>/.omh/memory.toml          committed: how the note store keys and expires
<repo>/.omh/hooks/               committed: hooks that only make sense in this repo
<repo>/AGENTS.md                 the project's own rules — tracked, and actually read
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
useful before it's configured. Settings are `omh set <key> <value>`, and the
key decides which file it lands in: one that can name a credential is kept out
of git, everything else is committed so a teammate cloning gets it —
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
  remove      omh set codegraph off — the feature, its server and its hooks
              together. Nothing is uninstalled and the next repo gets it back

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

Adding a harness is a TOML file, not a code change — where each capability
lands, how it is rendered, and the command that proves it loaded:

```toml
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
[Adapters](docs/design/adapters.md) is the whole schema.

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
graph at http://127.0.0.1:50257
  omh graph --stop
every session's graph for this repo, in one place
```

### Credentials

```console
$ omh auth claude --name personal
$ omh auth claude --name work
$ omh set account work        # which one this project uses
$ omh new claude
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
reads — including asking `claude mcp list` whether the servers loaded, rather
than trusting that a file landed:

```console
$ omh doctor
checking claude in omh/claude:8eae0d5c1511fa89 — no account, so credentials go unchecked…
  ✓  rules            /work/CLAUDE.md
  ✓  skills           /home/agent/.claude/skills
  ✓  mcp              /work/.mcp.json
  ✓  mcp-loaded       /work (claude mcp list)
  ✓  commands         /home/agent/.claude/commands
  ✓  subagents        /home/agent/.claude/agents
  ✓  hooks            /home/agent/.claude/settings.json
  …
```

Above those, in the same table, are the **host's** rows: whether the container
runtime is answering, the stacks detected here, settings omh does not read, what
omh has left behind, which omh set this checkout up, disk, and the host's git.
They are gathered before any container work, so a machine that cannot build a
sandbox still gets them instead of a single error.

A clean run ends `all N checks passed — claude's adapter paths are verified`;
anything else fails the command and the tally goes to stderr. With an account
chosen there is a credential check too, and the header names which account it
used.

A green unit suite proves omh mounts a path faithfully; it proves nothing about
whether anything reads it. That gap is what `doctor` closes.

## What isn't done

| | |
|---|---|
| **Memory** | mostly [built](docs/commands.md#omh-memory-) — the store, its schemas, retrieval, the team layer and `remember` / `recall` as MCP tools all ship. What remains is hub pages, whose lint needs a threshold the design refuses to let anyone guess. |
| **Cost accounting** | each base-set entry should report what it injects, in bytes, so the set has a reason to shrink. Not a benchmark — [here's why](docs/design/trust.md#measure-the-cost-argue-the-benefit). |
| **`sbx` backend** | the trait exists and declares capabilities; the spike that resolves file-mounts, guest paths and IDE attach has not run. Docker is the only verified runtime. |
| **Egress allowlist** | **unrestricted by design on Docker.** Egress policy is the backend's, not omh's — [decisions](docs/design/decisions.md) has recorded it as inherited from the runtime throughout, and `sbx` carries it. It arrives with that backend or not at all, together with the credential weakness it shares a fix with. |
| **`--dry-run` everywhere** | it runs everything and writes nothing on the commands that can answer it. `init` and the session verbs refuse the flag instead — each has to compute what it *would* do, and a preview that guessed would be worse than none. |
| **Other harnesses** | `opencode`, `omp` and `codex` pass `doctor`, but only `claude` has been driven for real work. |

Known rough edges: the graph store is shared across sessions of one repo, so an
agent can query another session's graph (mitigated, not prevented);
`.claude.json` is a file mount that cannot be atomically replaced; `omh s rm`
drops a session branch only when it has no commits; upgrading to 0.8.0 leaves
the old `omh-cache-<name>` volume and `omh-<name>` network behind and nothing
reports them (`docker volume ls | grep omh-cache-` finds them); and `omh s`
lists any directory under a session's worktree root as a session, so stray
clutter there shows up as one.

## Contributing

```console
$ ./scripts/check.sh --all         # format, lints, suite — `--all` needs a container runtime
$ omh doctor                       # the only thing that verifies an adapter
```

Two rules do most of the work, and [`CONTRIBUTING.md`](.github/CONTRIBUTING.md)
has the rest with the invariant list. To propose something for the base set,
[what an entry has to say](docs/design/base-set.md#proposing-an-entry) is the
bar — four fields, enforced by the build rather than by review. **[TDD,
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
