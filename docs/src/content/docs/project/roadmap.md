---
title: Roadmap
description: What ships when, and what gates what.
---

**Status: intent, with ✅ for what landed.** Nothing here is built by virtue of being written down. The ✅ marks are the claim; anything unmarked is a plan. Work that shipped outside a milestone is in [retrospectives](/project/retrospectives/).

Ordered by what gates what, not by what is most fun.

**These are milestones, not crate versions.** The crate is at `0.14.1` and
milestone v1.5 is roughly what 0.6.0 contained. They are deliberately not
kept in lockstep — v0 has spanned thirteen minor releases already: a milestone moves when
a body of work lands, semver moves on every release, and calling the crate `1.0`
would imply a stability this project has not earned — one verified harness, one
verified runtime.

## v0 — the base set, one harness

✅ `omh init` that decides · ✅ images · ✅ sandbox + worktree · ✅ persistence ·
✅ `omh auth` · ✅ `omh sNN attach` · ✅ `omh doctor` · ✅ `carry_in` ·
✅ code graph · ✅ `omh graph` · ✅ stack hooks ·
🔶 [memory](https://github.com/mindsers/ohmyharness/blob/main/docs/design/memory.md) — the store, retrieval and staleness
ship; hub pages do not

**Success criterion:** `omh init && omh new claude` is visibly better than raw
`claude`, with zero questions asked.

*Not in v0: further adapters, memory, breadth in the capability superset.*
[Breadth before depth is how distributions die.](/reference/adapters/#breadth-is-capped-on-purpose)

## v0.5 — backends

The runtime trait exists, and a `podman` backend ships beside Docker. The `sbx`
spike ran against `sbx` 0.39.0 — single-file mounts, chosen guest paths and the
image-delivery path are all measured — and the backend was rewritten from what
it found: stage-and-symlink for the mounts `sbx` cannot do natively, `docker
save | sbx template load` delivery, and label-free session reuse.

It is **opt-in** (`runtime = "sbx"`); `auto` never picks it. What decides
whether it becomes the default is the full live acceptance — a real `omh doctor
--harness claude` under `sbx` — which has not run. Until it does, **Docker is
the only end-to-end-verified runtime** and the
[credential weakness](/concepts/risks/#security) `sbx` addresses stays unaddressed on
Docker.

## v1 — accountability

✅ [`omh why`](/concepts/trust/) · ⬜ cost accounting

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
[measure the cost, argue the benefit](/concepts/trust/#measure-the-cost-argue-the-benefit)
— briefly, an eval suite over a stochastic metric costs real money per decision,
measures tokens rather than correctness, and becomes the thing the base set is
tuned to pass. Distributions have never earned legitimacy that way.

What cost accounting buys instead is the one thing genuinely missing: a reason
for the base set to ever **shrink**. Arguments for adding are always available;
*"this now costs 4.1 KB before you type anything, up from 2.3"* is the fact that
forces the other conversation, and it is free to produce.

## v1.5 — the work loop ✅ (0.6.0)

[Git](https://github.com/mindsers/ohmyharness/blob/main/docs/design/git.md), built. All twelve steps in that page's ledger landed across
#53–#67, and the page names the pull request behind each one.

`omh sNN log` makes the sandbox's commits visible, `--turns` reads the
end-of-turn snapshots omh takes for an agent that commits nothing, `diff` grew
`-p` and checkpoint numbers, `--keep` takes the numbers `log` printed and is
repeatable from a recorded replay point, `sync` brings trunk in merged on the
host, `rm` refuses over work no branch has, and `omh s` became a dashboard —
state, staleness, the files two sessions are both changing, and the sandbox
repositories a removed session left behind. `omh s01` is that dashboard scoped
to one session.

**Success criterion:** a session's work can be read, curated and landed without
typing `git` and without knowing that a session is a worktree. **Met.**

What this cost is worth reading before proposing the next one: a third of those
pull requests were corrections to the ones before them, and the recurring defect
was not a wrong answer but a confident one — a question omh could not answer
rendering identically to an answer. [Git](https://github.com/mindsers/ohmyharness/blob/main/docs/design/git.md) records each, with the
measurement that settled it.

## v2 — portability

Second adapter driven for real work, ✅ [`omh eject`](/concepts/trust/) (0.8.0), full `omh import`
covering rules, skills, hooks and commands rather than MCP alone.

None of these were ever gated on evidence — an earlier version of this roadmap
put them behind a benchmark, which parked three cheap, useful deliverables
behind a research project.

## v3 — the unique capability

Marketplace plugin port: Claude Code plugins re-rendered for other harnesses.

This is the thing nothing else can do, and it makes curation nearly free —
inherit Anthropic's taste, port it everywhere. It sits at v3 rather than v1
because it is worth very little until the ports are verified, which needs v2's
portability work and v1's evidence.

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
