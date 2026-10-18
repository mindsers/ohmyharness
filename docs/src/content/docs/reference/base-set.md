---
title: The base set
description: omh's opinion as a versioned data file — how to propose an entry, and what every field of one has to say.
---

Propose an entry for the base set — what every omh session is built from — and
look up what each field of an entry has to say.

**Status: built.** The set lives in a versioned manifest, `init` seeds from it and `omh why` explains from it, and the four-field test is enforced by the build. What is not built is cost accounting — the rollup that would give the set a reason to shrink.

**This is the product.** Everything else in omh is a place to put it.

It lives in a versioned TOML file — `~/.omh/base/2026.08.toml`, shipped with the
binary and installed by `init` the same way [adapters](/reference/adapters/) and editors
are. **Every session is built from it and [`omh why`](/reference/commands/#omh-why-thing)
explains from it**, so the two cannot disagree about what is installed or why.
`2026.08` holds eleven entries in three features — `codegraph`, `git-notice` and
`memory` — and five rejections.

## Proposing an entry

An entry earns its place by filling in four fields. **The build enforces them**:
`every_base_set_entry_states_its_case` in `src/base.rs` fails on an entry that
cannot, rather than leaving it to a reviewer's patience.

| Field | What it has to say | What makes one fail |
|---|---|---|
| `because` | what it buys, in one sentence | a restatement of what the thing is |
| `measured` | a value, the method, and the date it was taken | a number with no method, or an estimate |
| `instead_of` | what was considered and why it lost | an empty list where alternatives obviously exist |
| `remove` | the command that takes it out, and what survives | "uninstall it" |

**Costs are measured; benefits are argued.** They are different kinds of claim
and the output never blurs them, so a proposal should not either. `0.46s to
index this repo, cold — index_repository --mode fast, 821 nodes / 3813 edges,
in the sandbox, 2026-08-06` is a cost. *"Structural queries instead of
re-grepping the repo every task"* is a benefit, and you are free to reject it.

**A typed number is a claim nobody will re-check.** If the cost can be computed
in-process, compute it in a test rather than typing it. Six of the eleven
entries already have a cost a test computes, and that is the shape the rest
should take.

**Rejections are entries too.** If you evaluated something and it lost, that
belongs in `[[rejected]]` rather than in a pull-request comment nobody will find
— see [Rejections are artifacts](#rejections-are-artifacts).

### Why this is published

Curation is the part of a distribution that never finishes, and
[risks](/concepts/risks/) names it — not any technical problem — as the thing most
likely to kill this project. A base set only one person can extend has a
ceiling set by that person's afternoons.

The standard above already existed; it was just never written down as one. Put
plainly, it turns proposing an entry from research into review, which is the
only version of curation a project this size can sustain.

## Adding an entry

<div class="steps">

1. Add it to the newest manifest, with all four fields and its feature.

   The manifests are in [`base/`](https://github.com/mindsers/ohmyharness/tree/main/base).
   Name the feature the entry serves. If none of the existing three fits, the
   entry is introducing a feature, and that is a bigger claim than adding a
   package.

2. Measure the cost rather than estimating it, and record how.

   If it can be computed in-process, compute it in a test instead of typing it.

3. If it is a hook or a rules section, add it to `base::hooks()` or `base::sections()` too.

   The drift guard will tell you if you forget. A section's size is computed
   from the string it ships, so the manifest has to agree with the prose rather
   than with your estimate of it.

4. Run `cargo test`.

   If the curation test fails, the entry has not earned its place yet — that is
   the test working, not an obstacle.

</div>

If a further entry needs a paragraph to justify itself, it belongs in a profile
rather than the base set.

## An entry

```toml
version = "2026.08"

[[entry]]
name    = "codegraph"
kind    = "mcp"                 # mcp | hook | rules
feature = "codegraph"           # what it is part of; `[omh]` switches features, not entries
since   = "2026.06"
because = "structural queries instead of re-grepping the repo every task"
remove  = "omh set codegraph off — the feature, its server and its hooks together…"
command = "codebase-memory-mcp" # what init seeds; also the baseline `why` compares against

  [[entry.measured]]
  what  = "to index this repo, cold"
  value = "0.46s"
  how   = "index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox"
  on    = "2026-08-06"

  [[entry.instead_of]]
  name = "gitnexus"
  why  = "PolyForm-Noncommercial licence"
```

| Field | What it says | Required |
|---|---|---|
| `version` | the manifest's version, `YYYY.MM`. omh reads the newest by this, never by file name | yes |
| `name` | the entry's name, as `omh why <name>` takes it | yes |
| `kind` | `mcp`, `hook` or `rules` | yes |
| `feature` | what it is part of — see [A feature, not a flat list](#a-feature-not-a-flat-list) | yes |
| `since` | the base-set version it arrived in | yes |
| `because` | what it buys: argued, not measured | yes |
| `remove` | the way out, printed by `omh why` | yes |
| `command`, `args` | for an `mcp` entry, what `init` seeds — also the baseline `omh why` compares your copy against | for `mcp` |
| `[[entry.measured]]` | a cost: `what`, `value`, `how`, and `on` — the date, `YYYY-MM-DD` | at least one |
| `[[entry.instead_of]]` | an alternative, by `name`, and `why` it lost | at least one |
| `[[rejected]]` | a candidate turned down: `name`, `considered` (a version) and `because` | no |

`deny_unknown_fields`, matching `Adapter`, and for the same reason: a misspelled
key must fail loudly rather than parse into a base set that quietly contains
less than you think.

## A feature, not a flat list

`feature` is what an entry belongs to, and the feature is the unit you switch,
remove and ask about. `codegraph` is one thing: the MCP server, the four hooks
that make it *used* rather than merely installed, and the section of the rules
that tells the agent it exists.

| | |
|---|---|
| **removal** | `omh set codegraph off` takes the feature, its server and its hooks together. A test runs every `remove` command and asserts the feature ends up off |
| **disabling** | `[omh] codegraph = false` in `<repo>/.omh/settings.toml`. Per repo, and nothing is uninstalled — your `mcp.json` is untouched and the next repo gets it back |
| **explaining** | `omh why graph-first` answers "part of codegraph"; `omh why codegraph` lists what it brought |

**`[omh]` takes feature names only.** `graph-first = false` is refused, naming
the feature it belongs to — which is also how the grouping is discoverable
without reading this file.

<details>
<summary>Why half a feature is not allowed, and how removal got here</summary>

Half of a feature is not a smaller version of it — a graph with no
`graph-refresh` keeps answering confidently about code the session has since
rewritten. Taking features only keeps "graph on, refresher off" unrepresentable
rather than warned about. An earlier draft that allowed it needed a warning at
every launch, a manifest field declaring whether disabling cost correctness, and
a rule about which kind may switch off silently. Taking the granularity away
took all three with it.

Removal did not always take the whole feature. Before, it left the hooks behind,
nudging the agent toward something that was gone — and for a release the field
named the server's removal, which removed the server alone and left the feature
on.

The grouping lived as a comment header in the manifest — the one claim in that
file no test could check, while every other claim an entry makes is a field
with a guard demanding it be filled. `every_base_set_entry_names_its_feature`
is that guard, and it is load-bearing rather than documentary: an entry
belonging to nothing is an entry nobody can switch off.

</details>

## The curation standard is a test, not a convention

`every_base_set_entry_states_its_case` rejects:

- a blank `because`, `remove` or `since`
- an empty `instead_of` — *"an entry with no alternatives was not chosen, it was
  defaulted to"*
- an empty `measured`, or a measurement with a blank `what`, `value`, `how` or `on`
- a blank, unparseable, or impossible date: not `YYYY-MM-DD`, before the
  repository's first commit, or in the future — added after
  [the failure below](#cost-is-measured-benefit-is-argued)

<details>
<summary>Why a test, rather than a rule written down</summary>

[Why a distribution](/concepts/distribution/) says every entry states what it costs, what it
buys, what was considered instead, and how to remove it — and that anything
unable to fill in all four is taste pretending to be curation.

That was aspiration in a document nothing enforced. As a test, an entry cannot
be added without its reasoning. The cheapest moment to demand a justification is
before it ships, and the only moment anyone reliably does is when something
turns red.

</details>

### Cost is measured; benefit is argued

The manifest keeps the two halves apart. `because` is a judgement. `measured` is
a recording, and carries the date it was taken and the method — because a number
printed bare reads as a fact about right now, which is exactly the fabricated
authority this file exists to avoid.

**Where a cost can be computed, it is.** `grep_nudge()` builds `graph-first`'s
string from the same literals the hook ships, and a test recomputes it and fails
if the manifest disagrees. The three rules sections, `git-note`'s note and the
`memory` server's tool list are held the same way. That is the shape every
in-process cost should take. The rest need a container, which is
[`doctor`](/guides/troubleshooting/) territory — the same boundary that makes
adapter paths unverifiable in-process.

<details>
<summary>The failure behind the date rule: measurements typed, not taken</summary>

**This is not theoretical.** Every `on` in the first version of this manifest
read `2026-08-04`, one day before the repository's first commit. Three of those
entries claimed to measure this repo; one measured a file that did not exist
yet. They were typed, not taken. The guard walking every measurement checked a
date was *present* and never that it was true.

Two values were wrong independently: `~40 B` for the grep nudge was really 243
(the tilde is the tell — it made the number unfalsifiable), and a byte count
naming a specific file was stale on the commit that introduced it.

</details>

## Rejections are artifacts

```toml
[[rejected]]
name       = "gitnexus"
considered = "2026.06"
because    = "PolyForm-Noncommercial. Every omh user writing code at work would be in violation of a dependency they never chose."
```

`omh why gitnexus` answers from this. Without it the same candidate gets
re-litigated every time somebody rediscovers it — and the reasoning that
disqualified it, which is often a licence or an operational dependency rather
than a quality judgement, has to be reconstructed from memory.

## Versioning

The base set **expires**. A distribution's real work is re-choosing as the
catalogue churns, so the file is versioned and older ones are kept rather than
deleted. omh enforces three rules:

- **Selection is by parsed version**, not filename sort. A file whose `version`
  cannot be read is not a candidate.
- **A manifest declaring no entries is an error.**
- **Staleness is measured against the manifest version**, not the entry's
  `since`. Re-cutting the set marks every carried-over measurement stale, which
  is the prompt to re-take or re-affirm each one.

<details>
<summary>The three wrong answers filename sorting gave, and why <code>since</code> could never go stale</summary>

Sorting filenames was three silent wrong answers at once:

- any stray `.toml` sorting after the real manifest became the base set
- `2027.2` beat `2027.10`, so zero-padding was load-bearing and unenforced
- nothing compared a file's declared `version` to anything

The first of those was the worst: a `notes.toml` in `~/.omh/base` made `init`
seed `{"mcpServers": {}}` and report every decision cleanly, while the hooks —
which come from code, not the manifest — still pointed the agent at a server
that was not installed.

Against `since`, staleness could never fire: a measurement is taken at or after
the entry was added and `since` never moves, so no shipped number was flaggable
in 2027 or 2035. Against the version it fires whenever the base set is re-cut,
which is exactly when carried-over numbers should be re-taken or re-affirmed.

</details>

## Seeded, or generated

| | How it reaches a session | What follows |
|---|---|---|
| MCP servers, in `mcp.json` | **seeded**: `init` copies them into your profile with `write_if_absent` and never revisits | your edits survive, and your copy drifts from the manifest by design |
| hooks and rules sections | **generated**: written into the session at launch, from the manifest; they exist as a file nowhere | omh can ship a fix to its own machinery |

Two consequences of seeding, and both caused bugs:

- **An omh-seeded server and one you added are byte-identical in the same
  file.** Nothing marks which is which, so `omh why` recovers authorship by
  comparing against the manifest rather than by reading a marker.
- **The manifest is refreshed on upgrade while your profile is not.** `omh why`
  reports *"not what omh ships now"* and declines to say who caused the
  difference — it cannot know.

<details>
<summary>Why generation is the only way to fix a hook, and what guessing authorship did</summary>

Generation is the only arrangement in which omh can ship a fix to its own
machinery: seeding never revisits, so `git-unavailable` — rewritten once, after
the old pattern was found to miss the multi-line scripts agents most often emit
— would have gone on running broken in every repo initialised before the fix.

An earlier version of `omh why` that guessed who changed a seeded server told
every user they had edited files they never opened.

</details>

### Nothing in the base set refuses a call

Every hook in the set advises, and `graph-first` states the rule: *a nudge, not
a wall — a hook that blocks correct work gets disabled.* The distinction became
load-bearing when a second harness arrived, because advising and blocking are
one field apart on Claude Code and two different mechanisms on opencode; see
[`refuse`](/reference/configuration/#advising-is-not-blocking).

<details>
<summary>The one entry that did refuse, and why it was retired</summary>

One entry did until 2026.08: `git-unavailable` blocked every shell command
containing `git`, and it was right for exactly as long as git could not work —
the worktree's `.git` points at an admin directory omh does not mount. As a
*notice* the agent read "git does not work here" and ran `git status` anyway,
spending a tool call to reach an error omh already knew was coming, so it became
a refusal.

The sandbox has a repository of its own now, so the same hook refuses the
feature rather than explaining it, and it is retired. `omh why git-unavailable`
still answers, from the `rejected` table — *should omh stop the agent running
git?* is a question someone rediscovers, and the answer changed rather than
having been wrong the first time.

</details>

### `git-turn` and `git-note`

**`git-turn` is the cheapest entry in the set to justify**, because it is a
`run`, so it injects nothing. Its cost is ~80ms and ~1.3 KB of somebody's disk
per turn, not bytes of the agent's context. A `run` and an `inject` at the same
moment are not the same kind of expense — which is why it could be argued for
in a paragraph where `git-note` had to be argued for by the byte.

**`git-note` speaks once, after a sync, and then is silent.** It is the one
`SessionStart` hook whose cost is not paid per context rebuild: it captures a
file omh leaves after `omh sNN sync`, and the capture *removes* it. On a start
that follows no sync it never speaks at all.

**It is also the entry with the widest gap between harnesses.** Claude Code and
Codex deliver it. opencode maps no `session-start` at all, and omp has the moment
but no advisory channel there, so on both the sentence is never delivered. Both
say so by name at launch.

<details>
<summary>How a once-only event can live at a moment that repeats, and what is lost without it</summary>

The only other `SessionStart` hook, `graph-orient`, injects on every resume and
every compact. `git-note` captures a file that exists only after a sync and
removes it as it reads — that is how an event describing something that happened
once can afford to live at a moment that repeats.

Where the sentence is not delivered, the `base moved to <sha>` commit `sync`
writes is still in the sandbox's own log either way — what is lost is the prompt
to go and read it.

</details>

### A manifest hook name is omh's

A manifest **hook** name is omh's, on or off. **A file in `<repo>/.omh/hooks/`
answering to one is an error naming both**, never an override.

<details>
<summary>Why an error rather than an override, or a silent skip</summary>

Not an override: with the feature on, the generated hook would win anyway, and
with it off there would be nothing to override the file with, so switching a
feature off would leave the disabled thing running.

Not a silent skip, because `<repo>/.omh/hooks/` is somewhere people write on
purpose. A hook that is committed, reviewed and quietly never runs is worse than
one that refuses to start — and a repo that could replace `graph-refresh` could
make the graph lie while looking installed.

</details>

## Data versus code

MCP servers are pure data and live entirely in the manifest. Hook **commands**
and rules **bodies** stay in `src/base.rs`. So the manifest carries *curation
metadata* while the code carries the thing itself — two sources describing one
base set, which can drift.

**A test asserts the name sets match exactly, in both directions**, because the
drift is silent in the worst way: `omh why` confidently explaining an entry that
is no longer installed, or an entry shipping with no explanation at all.

<details>
<summary>Why hook commands and rules bodies stay in code</summary>

Hook commands are intricate shell that interpolates `GRAPH_BIN` and
`$OMH_GRAPH_PROJECT`, and flattening them into TOML would break that
compile-time coupling. `git-note` is the same argument in a second shape: it
interpolates `shadow::GUEST_NOTE`, so the path the hook reads is the one
`shadow::note_file` writes rather than a second copy of it — and a hook reading
a path omh never writes fails by saying nothing at all, forever.

Rules bodies stay there for the same reason. `memory-rules` interpolates
`memory::GUEST_LOCAL_NOTES`, which is the notes store and a different thing
despite the name, and `git-rules` reads `shadow::ARRANGEMENT`, which the
sandbox's seed commit reads too — and two copies of the same sentence drift,
with the one that drifts never being the one you are reading.

</details>

## Next

<div class="cards">

- [`omh why`](/reference/commands/#omh-why-thing)\
  Ask any entry what it costs, what was considered instead, and how to remove it.
- [Why a distribution](/concepts/distribution/)\
  The thesis the base set is the product of.
- [Trust](/concepts/trust/)\
  Measure the cost, argue the benefit — and the way out.
- [Roadmap](/project/roadmap/)\
  Cost accounting, the rollup that would let the set shrink.

</div>
