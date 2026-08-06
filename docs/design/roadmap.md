# Roadmap

Ordered by what gates what, not by what is most fun.

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

## v1 — evidence

[`omh bench`](trust.md), then `omh why` reading from it.

This gates every subsequent base-set decision, which is exactly why it comes
before more features rather than after them. Adding a seventh base-set entry
before `bench` exists means adding it on the same evidence as the first six —
none — and the [weak spot](distribution.md#the-honest-weak-spot) gets wider
instead of narrower.

## v2 — portability

Second adapter driven for real work, [`omh eject`](trust.md), full `omh import`
covering rules, skills, hooks and commands rather than MCP alone.

## v3 — the unique capability

Marketplace plugin port: Claude Code plugins re-rendered for other harnesses.

This is the thing nothing else can do, and it makes curation nearly free —
inherit Anthropic's taste, port it everywhere. It sits at v3 rather than v1
because it is worth very little until the ports are verified, which needs v2's
second adapter and v1's evidence.

## v4 — memory and graph, re-justified

Both are in the product on argument. Once `bench` exists they are measured, and
either earn their place or are dropped.

A distribution that cannot retire its own choices is just an opinion with a
changelog.

---

## What would change this order

- **The spike fails badly.** If `sbx` forces host-path mounts, the staging model
  needs rework rather than a tweak, and v0.5 grows.
- **`bench` says the graph does not pay.** Then v4 happens early and subtracts,
  which is the system working.
- **Someone actually uses this besides its author.** Real usage reorders roadmaps
  more reliably than reasoning does — every item on this list that has shipped
  was reordered at least once by running the tool.
