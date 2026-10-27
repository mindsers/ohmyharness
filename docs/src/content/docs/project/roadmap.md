---
title: Roadmap
description: What omh builds next, in the order that gates it, and what has already landed.
---

What omh builds next, in the order that gates it, and what has already landed.
Ordered by what gates what, not by what is most fun.

**Status: intent, with ✅ for what landed.** Nothing here is built by virtue of being written down. The ✅ marks are the claim; anything unmarked is a plan. Work that shipped outside a milestone is in [retrospectives](/project/retrospectives/).

## At a glance

✅ landed · 🔶 partly landed · ⬜ not started

| Milestone | Status | Landed | Remains |
|---|---|---|---|
| [v0 — the base set, one harness](#v0--the-base-set-one-harness) | 🔶 | everything but memory's hub pages | memory's hub pages |
| [v0.5 — backends](#v05--backends) | 🔶 | the runtime trait; `podman`, and `sbx` rewritten from its spike — both opt-in since 0.10.0 | a live `omh doctor --harness claude` under `sbx`, which decides whether it becomes the default |
| [v1 — accountability](#v1--accountability) | 🔶 | `omh why` | cost accounting: what the whole set injects |
| [v1.5 — the work loop](#v15--the-work-loop--060) | ✅ | all twelve steps, in 0.6.0 | — |
| [v2 — portability](#v2--portability) | 🔶 | `omh import` beyond MCP (0.4.0), `omh eject` (0.8.0) | a second adapter driven for real work |
| [v3 — the unique capability](#v3--the-unique-capability) | ⬜ | — | the marketplace plugin port |
| [v4 — memory and graph, re-justified](#v4--memory-and-graph-re-justified) | ⬜ | — | a review of both against their measured cost |

**These are milestones, not crate versions.** The crate is at `0.15.0` and
milestone v1.5 is roughly what 0.6.0 contained. They are deliberately not kept
in lockstep — v0 has spanned fifteen minor releases already.

<details>
<summary>Why the crate is not called 1.0</summary>

A milestone moves when a body of work lands, and semver moves on every release.
Calling the crate `1.0` would imply a stability this project has not earned —
one verified harness, one verified runtime.

</details>

## v0 — the base set, one harness

✅ `omh init` that decides · ✅ images · ✅ sandbox + worktree · ✅ persistence ·
✅ `omh auth` · ✅ `omh sNN attach` · ✅ `omh doctor` · ✅ `carry_in` ·
✅ code graph · ✅ `omh graph` · ✅ stack hooks ·
🔶 [memory](/reference/memory/) — the store, retrieval and staleness
ship; hub pages do not

**Success criterion:** `omh init && omh new claude` is visibly better than raw
`claude`, with zero questions asked.

*Not in v0: further adapters, memory, breadth in the capability superset.*
[Breadth before depth is how distributions die.](/reference/adapters/#breadth-is-capped-on-purpose)

## v0.5 — backends

🔶 The runtime trait exists, and a `podman` backend ships beside Docker. `sbx`
ships too, rewritten from what its spike found.

It is **opt-in** (`runtime = "sbx"`); `auto` never picks it. What decides
whether it becomes the default is the full live acceptance — a real `omh doctor
--harness claude` under `sbx` — which has not run. Until it does, **Docker is
the only end-to-end-verified runtime** and the
[credential weakness](/concepts/risks/#security) `sbx` addresses stays unaddressed on
Docker.

<details>
<summary>What the sbx spike measured, and what changed because of it</summary>

The spike ran against `sbx` 0.39.0 — single-file mounts, chosen guest paths and
the image-delivery path are all measured — and the backend was rewritten from
what it found: stage-and-symlink for the mounts `sbx` cannot do natively,
`docker save | sbx template load` delivery, and label-free session reuse.

</details>

## v1 — accountability

✅ [`omh why`](/concepts/trust/) · ⬜ cost accounting

**`why` ships.** Every base-set entry answers who installed it, what it costs,
what was considered instead and how to remove it — and the four fields are
enforced by a test rather than by convention, so an entry cannot be added
without its reasoning. The base set moved out of the binary into a versioned
manifest to make that possible; every launch generates from it and `why` explains from it.

**What remains is the rollup:** what the **whole** set injects, measured in
bytes, deterministically and without an LLM in the loop. Per-entry costs are
recorded in the manifest today, and six of them — the grep nudge, the three
rules sections, `git-note`'s note and the memory server's tool list — are
computed by a test rather than typed, which is the shape the rest should take.
The ones needing a live MCP connection are `doctor` territory rather than a pure
function.

**What cost accounting buys** is the one thing genuinely missing: a reason for
the base set to ever **shrink**. Arguments for adding are always available;
*"this now costs 4.1 KB before you type anything, up from 2.3"* is the fact that
forces the other conversation, and it is free to produce.

<details>
<summary>Why this is deliberately not a benchmark</summary>

The reasoning is in
[measure the cost, argue the benefit](/concepts/trust/#measure-the-cost-argue-the-benefit)
— briefly, an eval suite over a stochastic metric costs real money per decision,
measures tokens rather than correctness, and becomes the thing the base set is
tuned to pass. Distributions have never earned legitimacy that way.

</details>

## v1.5 — the work loop ✅ (0.6.0)

The loop around the sandbox's repository, built. All twelve steps of
[its design](https://github.com/mindsers/ohmyharness/blob/afad736a609fd6e3921e09f784c420e05caca115/docs/design/git.md#order) landed across #53–#67, and that page — kept in the
repository's history, as it last stood — names the pull request behind each
one.

- `omh sNN log` makes the sandbox's commits visible, and `--turns` reads the
  end-of-turn snapshots omh takes for an agent that commits nothing.
- `diff` grew `-p` and checkpoint numbers.
- `--keep` takes the numbers `log` printed, and is repeatable from a recorded
  replay point.
- `sync` brings trunk in, merged on the host.
- `rm` refuses over work no branch has.
- `omh s` became a dashboard — state, staleness, the files two sessions are both
  changing, and the sandbox repositories a removed session left behind.
  `omh s01` is that dashboard scoped to one session.

**Success criterion:** a session's work can be read, curated and landed without
typing `git` and without knowing that a session is a worktree. **Met.**

**What this cost** is worth reading before proposing the next one: a third of
those pull requests were corrections to the ones before them, and the recurring
defect was not a wrong answer but a confident one — a question omh could not
answer rendering identically to an answer.
[The design](https://github.com/mindsers/ohmyharness/blob/afad736a609fd6e3921e09f784c420e05caca115/docs/design/git.md#foundations) records
each, with the measurement that settled it.

## v2 — portability

⬜ Second adapter driven for real work · ✅ full `omh import` (0.4.0), covering
rules, skills, hooks and commands rather than MCP alone · ✅
[`omh eject`](/concepts/trust/) (0.8.0)

None of these were ever gated on evidence — an earlier version of this roadmap
put them behind a benchmark, which parked three cheap, useful deliverables
behind a research project. See [`omh import`](/reference/commands/#omh-import-capability-harness).

## v3 — the unique capability

⬜ Marketplace plugin port: Claude Code plugins re-rendered for other harnesses.

This is the thing nothing else can do, and it makes curation nearly free —
inherit Anthropic's taste, port it everywhere. It sits at v3 rather than v1
because it is worth very little until the ports are verified, which needs v2's
portability work and v1's evidence.

## v4 — memory and graph, re-justified

⬜ Both are in the product on argument. The re-justification is a real review
against their measured cost and a stated criterion — not a p-value — and either
they earn their place or they go.

A distribution that cannot retire its own choices is just an opinion with a
changelog.

---

## What would change this order

- **The spike fails badly.** If `sbx` forces host-path mounts, the staging model
  needs rework rather than a tweak, and v0.5 grows. *The spike has run:* `sbx`
  0.39.0 mounts a workspace at its exact host path and cannot mount a single
  file, and the `sbx` backend absorbed that with stage-and-symlink — see
  [v0.5](#v05--backends).
- **The graph's cost grows faster than its usefulness.** Then v4 happens early
  and subtracts, which is the system working.
- **Someone actually uses this besides its author.** Real usage reorders roadmaps
  more reliably than reasoning does — every item on this list that has shipped
  was reordered at least once by running the tool.

## Next

<div class="cards">

- [Retrospectives](/project/retrospectives/)\
  Work that shipped outside a milestone, and what it cost.
- [Decisions](/project/decisions/)\
  The choices these milestones build on.
- [Risks](/concepts/risks/)\
  What is still weak, stated plainly.

</div>
