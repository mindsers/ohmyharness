# Code graph

Every session is indexed into a graph the agent can query structurally, instead
of re-deriving the shape of your codebase by grepping it every task.

The server is [codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp)
— MIT, a single static binary with no runtime and no database. Measured on this
repo: **0.46s to index, 3.4MB on disk, 821 nodes / 3813 edges.**

## Why this one

Six candidates were surveyed. What eliminated most of them was not quality:

| | licence | external deps | verdict |
|---|---|---|---|
| gitnexus | **PolyForm-Noncommercial** | — | most users, deepest integration, **disqualified** |
| codegraphcontext | MIT | **Neo4j** | a service to run is a decision to make |
| codegraph (npm), mcp-code-graph | — | — | a year stale |
| **codebase-memory-mcp** | MIT | **none** | chosen |
| @sdsrs/code-graph | MIT | node | close second |

**A noncommercial licence cannot be a default.** Every omh user writing code at
work would be in violation of a dependency they never chose — a distribution
doing that is doing something *to* its users rather than for them. gitnexus is a
legitimate opt-in profile and never a base-set entry.

What was left decided itself: one static binary, no database, `linux/arm64`, 158
languages.

## Three design consequences

- **The graph lives in the base image**, not a harness layer. It is
  harness-agnostic, and every session should get the same one.
- **The cache volume is keyed by repo, not harness.** That is what keeps the index
  warm across a switch from Claude Code to opencode. Tested.
- **The checkout mounts read-only while indexing.** An indexer that can write into
  your repo is a sandbox hole for no benefit.

Indexing runs *inside* the sandbox, because the cache is a container volume — an
index built on the host lands where no session can read it.

## Current, used, and visible

An MCP server on its own is inert: indexed once, never refreshed, never reached
for. Four hooks and a section of the rules are what make this a base-set entry
rather than an installed package — and all six are one feature, `codegraph`,
removed and disabled together.

| Hook | Fires | Cost | Buys |
|---|---|---|---|
| `graph-orient` | `session-start` | 2,300 B per context rebuild | modules, layers, boundaries, entry points |
| `graph-first` | `before-tool`, `search` | 243 B per grep¹ | structural questions in one call |
| `graph-read` | `before-tool`, `read` | 0 unless it speaks | **1,511 bytes** for one symbol instead of a whole module |
| `graph-refresh` | `turn-end` | 0.14s per turn | a graph describing the code as it is *now* |

Those are omh's words, not a harness's — see [writing a hook](configuration.md#writing-a-hook).
What `session-start` is called inside Claude Code is the adapter's business.

¹ For a `ohmyharness-s01`-length project name, which the nudge interpolates
twice — the figure moves two bytes per character of `<repo>-<session>`.
Computed from the shipped literals rather than typed in.

**Kept current.** A session's worktree is not the checkout it started from; it
holds whatever the agent has since written. Each session indexes its own, and
`graph-refresh` re-indexes when a turn ends. Measured at **0.45s cold, 0.14s
incremental** — cheap enough that a stale graph is a choice rather than a
constraint.

**Orientation beats interception.** `graph-orient` is the only one that costs
nothing per tool call: the agent is *given* the module map instead of
discovering it by reading files. Everything else fires only when the agent is
already about to do something more expensive.

**Nudges, never walls.** Grep is right for a literal string, and a hook that
blocks correct work gets disabled. `graph-read` goes further and stays **silent**
unless a symbol lookup would actually be cheaper — a source file large enough to
be worth not reading whole. `Read` is the most frequent tool there is, and a
nudge on every call becomes noise the model learns to skip.

Hooks reference `$OMH_GRAPH_PROJECT` rather than baking in a session, so one
hook serves every session and the agent is told *which* graph is its own at the
moment it is choosing.

They are **generated from the base manifest at launch**, not files you can edit
— which is what lets omh ship a fix to one. Switching them off means switching
off the graph: `omh config mcp rm codegraph` to remove it, or
`[omh] codegraph = false` in `.omh/settings.toml` for one repo. There is
deliberately no way to keep the graph and drop `graph-refresh`, because that is
the combination that manufactures confident wrong answers.

### What the hook contract cost

Three things, none of them guessable:

- **A hook reaches the model through `hookSpecificOutput.additionalContext`**,
  injected as a system reminder. Bare stdout on exit 0 is not that channel — the
  first nudge shipped that way and **may never have been seen**. Found by reading
  the spec, not by anything failing.
- **`SessionStart` re-fires on resume and compact**, so orientation is paid every
  time context is rebuilt, not once. That is why it sends four targeted aspects
  (2,138 B) rather than `overview` (6,173 B).
- **`--aspects` does not take a comma-separated list.** That form returns *empty*,
  silently, which looks exactly like a hook that never fired. The flag repeats.
  Pinned by a test.

## Which tools earn a hook

The build ships **14** tools (`check_index_coverage`, from the marketing copy,
is not among them). Most are plumbing omh drives; a few are worth intercepting.

| Hooked | Because |
|---|---|
| `get_architecture` | orientation, and the only one free per call |
| `search_graph` | replaces iterative grepping |
| `get_code_snippet` | replaces reading a whole file for one symbol |
| `index_repository` | keeps the rest honest |

| Candidate | Trigger |
|---|---|
| `trace_path` | "how does A reach B" — the alternative is reading every file in the chain |
| `detect_changes` | reviewing a diff: structural impact rather than changed lines |

**`search_code` is deliberately not hooked.** It looks like a cheaper grep, but
`--pattern 'fn unfilled' --mode compact` returned `total_results: 0` against a
symbol that demonstrably exists. Routing literal search through it would make
things worse, and the claim stays unverified until that is explained.

`query_graph` (Cypher) is a power tool the agent should reach for deliberately.
`manage_adr`, `ingest_traces`, `get_graph_schema`, `index_status`,
`list_projects` and `delete_project` are plumbing.

## `omh graph`

```console
$ omh graph
omh: graph at http://127.0.0.1:56286
  every session's graph for this repo, in one place
  stop with: omh graph --stop
```

**Scope follows the data.** Every session's graph lives in one volume, so a
per-session server showed every other session's graph anyway — N identical
websites on N ports. A repo-scoped service removes the duplication, survives
sessions coming and going, and needs no session to exist at all.

It also mounts **only the index** — no worktree, no credentials, no profile. The
per-session version ran inside a container holding a writable worktree and live
credentials, which was exposure for no purpose.

Lifecycle became `docker run` / `docker rm`, idempotent by construction. That
deleted the `pgrep` guard, the detached `exec` and the `pkill` the per-session
version needed — each of which had been a bug before it worked. Choosing the
right scope removed them rather than fixing them.

### What verifying the UI cost

Five things were wrong, none findable from the documentation:

- **The npm package cannot deliver the UI.** `CBM_VARIANT=ui` is documented, but
  the published 0.9.0 installer hardcodes the variant and never reads it —
  verified in a container, which still reported *"built without the embedded
  UI"*. omh fetches the release tarball directly, checksum-verified, with the
  arch from `TARGETARCH`.
- **The UI dies with stdio.** Backgrounded with stdin closed, it logs
  `ui.serving` then `server.shutdown`. It runs behind `sleep infinity |`.
- **It binds container loopback**, with no bind-address flag, so a published port
  forwards to nothing: `HTTP 200` inside, no response outside. `socat` bridges it.
- **A detached service cannot be spawned and abandoned.** `A & exec B` left the
  server running and the bridge dead.
- **The base image tag was a mutable `:latest`**, so rebuilds were skipped and
  base recipe changes had *never* shipped. Adding `socat` silently did nothing
  until the base tag got its own recipe digest — the same fix already applied to
  harness images, left half-done. Harness layers now pin an exact base.

## What the agent can still see

All graphs for one repo share a volume. That is what keeps the index warm across
a Claude Code → opencode switch, and cross-*repo* isolation is intact.

But `list_projects` shows a session every other session's graph for that repo,
and querying the wrong one answers confidently about code that is not in this
worktree.

Three mitigations, no guarantee: `AGENTS.md` names the project, the hook repeats
it at the moment of choosing, and `omh s rm` drops a session's graph along with
the code it describes.

Hard isolation would need a volume per session — a full re-index each time, and
no ability to compare branches. Worth doing only if this is observed to actually
go wrong.

## The honest caveat

None of the token numbers above are a benchmark. They are measurements of
individual operations — what the graph **costs** and what a single lookup
replaces — not evidence that it makes an agent better at tasks overall.

That distinction is deliberate and permanent. omh measures cost and argues
benefit, rather than running an eval suite over a stochastic metric that would
cost real money per decision and become the thing the base set is tuned to pass:
[measure the cost, argue the benefit](design/trust.md#measure-the-cost-argue-the-benefit).

So the honest claim for the graph is *"it costs 2.3 KB per context rebuild and
0.14s per turn, and structural questions beat re-grepping"* — the first half
measured, the second half a judgment you are free to disagree with, and to
[remove](design/trust.md) in one command.
