# Roadmap

Ordered by what gates what, not by what is most fun.

**These are milestones, not crate versions.** The crate is at `0.5.0` and
milestone v0 is roughly what those releases contain. They are deliberately not
kept in lockstep — v0 has spanned seven releases already: a milestone moves when
a body of work lands, semver moves on every release, and calling the crate `1.0`
would imply a stability this project has not earned — one verified harness, one
verified runtime.

## v0 — the base set, one harness

✅ `omh init` that decides · ✅ images · ✅ sandbox + worktree · ✅ persistence ·
✅ `omh auth` · ✅ `omh attach` · ✅ `omh doctor` · ✅ `carry_in` ·
✅ code graph · ✅ `omh graph` · ✅ stack hooks · ⬜ [memory](memory.md)

**Success criterion:** `omh init && omh claude` is visibly better than raw
`claude`, with zero questions asked.

*Not in v0: a second adapter, memory, breadth in the capability superset.*
[Breadth before depth is how distributions die.](adapters.md#breadth-is-capped-on-purpose)

## v0.5 — backends

The runtime trait exists and declares capabilities. What is missing is the
spike: build an opencode kit, try a single-file mount, try attaching an IDE.

It is roughly an afternoon, and it decides whether `sbx` becomes the default or
stays opt-in hardening. Until it runs, **Docker is the only verified runtime**
and the [credential weakness](risks.md#security) stays unaddressed.

## v1 — accountability

✅ [`omh why`](trust.md) · ⬜ cost accounting

`why` ships: every base-set entry answers who installed it, what it costs, what
was considered instead and how to remove it — and the four fields are enforced
by a test rather than by convention, so an entry cannot be added without its
reasoning. The base set moved out of the binary into a versioned manifest to
make that possible; `init` seeds from it and `why` explains from it.

What remains is the rollup: what the **whole** set injects, measured in bytes,
deterministically and without an LLM in the loop. Per-entry costs are recorded
in the manifest today, and one of them — the grep nudge — is computed by a test
rather than typed, which is the shape the rest should take. The ones needing a
live MCP connection are `doctor` territory rather than a pure function.

This is deliberately *not* a benchmark. The reasoning is in
[measure the cost, argue the benefit](trust.md#measure-the-cost-argue-the-benefit)
— briefly, an eval suite over a stochastic metric costs real money per decision,
measures tokens rather than correctness, and becomes the thing the base set is
tuned to pass. Distributions have never earned legitimacy that way.

What cost accounting buys instead is the one thing genuinely missing: a reason
for the base set to ever **shrink**. Arguments for adding are always available;
*"this now costs 4.1 KB before you type anything, up from 2.3"* is the fact that
forces the other conversation, and it is free to produce.

## v2 — portability

Second adapter driven for real work, [`omh eject`](trust.md), full `omh import`
covering rules, skills, hooks and commands rather than MCP alone.

None of these were ever gated on evidence — an earlier version of this roadmap
put them behind a benchmark, which parked three cheap, useful deliverables
behind a research project.

## v3 — the unique capability

Marketplace plugin port: Claude Code plugins re-rendered for other harnesses.

This is the thing nothing else can do, and it makes curation nearly free —
inherit Anthropic's taste, port it everywhere. It sits at v3 rather than v1
because it is worth very little until the ports are verified, which needs v2's
second adapter and v1's evidence.

## v4 — memory and graph, re-justified

Both are in the product on argument. The re-justification is a real review
against their measured cost and a stated criterion — not a p-value — and either
they earn their place or they go.

A distribution that cannot retire its own choices is just an opinion with a
changelog.

---

## What would change this order

- **The spike fails badly.** If `sbx` forces host-path mounts, the staging model
  needs rework rather than a tweak, and v0.5 grows.
- **The graph's cost grows faster than its usefulness.** Then v4 happens early
  and subtracts, which is the system working.
- **Someone actually uses this besides its author.** Real usage reorders roadmaps
  more reliably than reasoning does — every item on this list that has shipped
  was reordered at least once by running the tool.
