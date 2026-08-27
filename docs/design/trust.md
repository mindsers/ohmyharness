# Trust

The standard complaint about oh-my-zsh is opacity: a slow shell nobody can
diagnose. *"Without the hassle of understanding"* curdles into *"unable to
understand."*

That is the failure mode omh is most exposed to, because it makes more decisions
on your behalf than oh-my-zsh ever did — and some of them are security
decisions. Four commands exist to prevent it. Three are built.

The shape of the answer matters as much as the commands: omh's curation is
legitimate because it is **stated, visible, and reversible**, not because it is
proven. See [measure the cost, argue the benefit](#measure-the-cost-argue-the-benefit).

## `omh repo` — provenance ✅

Every value says where it came from and what it beat.

```console
$ omh repo
settings
  carry_in   [".env.local"]   ← local (overrides shared)
```

No competitor does this. It is what makes layered settings debuggable instead of
mysterious, and it is cheap — the resolver already knows the answer; the only
work was refusing to throw it away.

It grew a second half when selection landed, and the new half is the one people
will actually use. With a curated catalogue the interesting question stops being
"what is this set to" and becomes **"why is this skill not here"** — so `omh
repo` answers that too, naming what this checkout uses, what it does not, and
which of omh's features are off:

```console
using
  skills      review-diff   (1 not selected: refactor)
```

See [Configuration](../configuration.md#two-scopes-two-commands).

## `omh doctor` — verification ✅

Proves an adapter's claims against a real container. Covered in
[Troubleshooting](../troubleshooting.md).

Belongs in this list because it is the answer to *"how do I know omh actually
did what it said?"* — and because the answer is not "the tests pass."

## `omh why <thing>` — justification ✅

Provenance extended from *where* to *why*.

```console
$ omh why codegraph
codegraph — omh's choice, in the base set since 2026.06

  because     structural queries instead of re-grepping the repo every task
  costs       0.46s to index this repo, cold   measured 2026-08-06
              index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox
              3.4 MB on disk                   measured 2026-08-06
              the graph volume after a cold index of this repo
  instead of  gitnexus            PolyForm-Noncommercial licence
              codegraphcontext    needs a Neo4j service running
              @sdsrs/code-graph   close second; needs a node runtime rather than a static binary
  installed   this repo
  remove      omh settings mcp rm codegraph

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

Read the shape of that output carefully, because it encodes the whole position:
**the cost is measured and the benefit is argued.** Those are different kinds of
claim and the command does not blur them — every cost carries the date it was
taken and the method it was taken by, and one predating the current base-set
version is marked stale.

An earlier version compared against the entry's own `since`, which never moves —
so no shipped measurement could ever be flagged. A byte count in this repo went
wrong within a day and the check said nothing, because it was structurally
incapable of saying anything.

### Authorship is the point

The base set is *seeded* into your profile at `init` and then lives as ordinary
config, so an omh entry and one you added are byte-identical in the same file.
`why` recovers the difference by comparing against the manifest — derived, never
recorded, so there is no marker to go stale.

Seven answers, and the two negatives matter most:

- **your choice** — omh offers **no rationale**. A tool that answers "because it
  is in the base set" about something you added is lying about its own
  authorship, and telling the two apart is the whole feature.
- **written by omh init** — a `rust-format` hook derived from your `Cargo.toml`
  is omh's writing but not omh's opinion. Disowning it would be the same false
  claim pointing the other way.

A third: **omh's own, generated at launch** — the hooks and rules sections the
base manifest produces. They are not files anywhere, which is what lets a fix
reach a repo initialised a year ago, and it means there is nothing of yours to
compare: a file of that name is a leftover, and `omh why` names it as one.

A fourth: **not what omh ships now**. `init` seeds what it seeds once and never
rewrites it, while the shipped baseline moves every release — so omh genuinely
cannot tell an edit from an upgrade, and says so rather than picking the
accusing guess. An earlier version called this *"modified by you"* and told
every user they had edited a file they never opened.

The rest: *omh's choice*, *not installed here*, and *considered, not in the base
set* — which is how a rejection stops being re-litigated every time somebody
rediscovers it.

Full output in [Commands](../commands.md#omh-why-thing).

## Measure the cost, argue the benefit

The obvious missing piece here used to be `omh bench` — a fixed task suite
measuring tokens-to-first-correct-edit with each base-set component on and off.
It is not on the [roadmap](roadmap.md), and the reasoning is worth keeping
because it is easy to re-derive the wrong way.

**The metric does not survive contact with the thing it measures.**
Tokens-to-first-correct-edit is stochastic. Separating a 15% effect from
run-to-run variance takes enough repetitions that every base-set decision costs
real money and a day of wall clock. A benchmark that cannot distinguish signal
from noise is not neutral — it manufactures confidence, which is worse than the
argument it replaced.

**It also measures the cheap thing.** Fewer tokens is not the goal; the agent
doing the right thing without supervision is. A code graph could cut tokens
while making the agent *more confidently wrong* — a stale structural answer is
worth less than an honest grep — and the metric would score that as a win.
Twelve tasks in one Rust repo says little about a 400k-line TypeScript monorepo,
which is exactly where the graph would matter most.

And a fixed suite becomes the target. The base set gets tuned to pass it.

**The premise was wrong, too.** No distribution has earned legitimacy through
evals. Debian does not benchmark its package choices; oh-my-zsh does not A/B its
plugins. What makes their curation legitimate is stated criteria, visible
reasoning, and trivial reversibility — which is what the rest of this page is.

### What still needs solving

One real thing. **Without a feedback loop, a base set only grows.** Arguments
for adding are always available, nothing ever leaves, and that is precisely how
oh-my-zsh got slow. Taste alone does not subtract.

But the loop does not have to be an eval. Split it by what is honestly knowable:

| | approach | why |
|---|---|---|
| **Cost** | measured, deterministically | bytes injected per session, per tool call, per turn. No LLM, no variance, runs in CI. |
| **Benefit** | argued, in prose | it is a judgment call, and a number does not make it otherwise |

That asymmetry is the right way round: **cost is what creeps, benefit is what
you notice.** It also produces the retirement trigger that was actually wanted —
*"the base set costs 4.1 KB before you type anything, up from 2.3"* is a fact
that forces a conversation, and it is free to produce.

The numbers already in [code graph](../code-graph.md#current-used-and-visible)
are exactly this shape: 2,300 B for orientation, 1,511 bytes for one symbol
instead of a whole file, 0.14s to re-index. All real, none of them requiring an
eval harness — and the one that used to name a specific file's byte count is
gone, because it was stale within a day of being written.

This does mean the [weak spot](distribution.md#the-honest-weak-spot) is answered
by transparency rather than by proof. That is the honest position, and claiming
otherwise would need the benchmark to be trustworthy — which is where this
started.

## `omh eject` — the exit ⬜

Write out the raw per-harness config and step aside.

For an opinionated tool, **a credible exit is what makes adoption safe.** You are
choosing to hand a tool your rules, credentials and sandbox policy; being able
to leave with all of it is the difference between a default and a cage.

Nearly free to build, since omh already generates exactly these files.

---

Together these make the opinion a **default, not a cage**. An app store cannot
be overridden, because it never decided anything in the first place — the
freedom it offers is the freedom to do the work yourself.
