---
title: Trust
description: Provenance, evidence, and a credible exit.
---

This page explains how omh keeps the decisions it makes for you inspectable and
reversible — the four commands that do it, and why it measures costs but argues
benefits. Read it to decide whether to trust what omh set up.

**In short**

- **omh's curation is legitimate because it is stated, visible and reversible**
  — not because it is proven. → [Measure the cost, argue the benefit](#measure-the-cost-argue-the-benefit)
- **Every value says where it came from, and every entry says whose it is.**
  `omh info --repo` gives provenance; `omh why` gives the reason, and never
  claims your choice as omh's. → [`omh why`](#omh-why-thing--justification-)
- **"The tests pass" is not the proof.** `omh doctor` checks the setup against a
  real container. → [`omh doctor`](#omh-doctor--verification-)
- **Costs are measured; benefits are argued.** A benchmark would manufacture
  confidence, so none is planned — the open problem is a base set that only
  grows. → [What still needs solving](#what-still-needs-solving)
- **There is a way out.** `omh eject` writes the raw per-harness config, which
  makes the opinion a default, not a cage. → [`omh eject`](#omh-eject--the-exit-)

:::note[Status: built, except cost accounting]
`omh why` ships and answers for what omh installed, what it rejected and what
you added, and [`omh eject`](/reference/commands/#omh-eject-harness---to-dir)
ships as of 0.8.0. What does not is the rollup that would measure what the whole
set injects — the one thing here that would give the base set a reason to
shrink.
:::

## Opacity is the failure mode

The standard complaint about oh-my-zsh is opacity: a slow shell nobody can
diagnose. *"Without the hassle of understanding"* curdles into *"unable to
understand."*

**That is the failure mode omh is most exposed to**, because it makes more
decisions on your behalf than oh-my-zsh ever did — and some of them are
security decisions. Four commands exist to prevent it, and all four are built
as of 0.8.0:

| Command | The question it answers |
|---|---|
| [`omh info --repo`](#omh-info---repo--provenance-) | where did this value come from, and why is this skill not here? |
| [`omh doctor`](#omh-doctor--verification-) | did omh actually do what it said? |
| [`omh why <thing>`](#omh-why-thing--justification-) | why is this here, what does it cost, and whose choice was it? |
| [`omh eject`](#omh-eject--the-exit-) | how do I leave, with all of it? |

**The shape of the answer matters as much as the commands**: omh's curation is
legitimate because it is **stated, visible, and reversible**, not because it is
proven. See [measure the cost, argue the benefit](#measure-the-cost-argue-the-benefit).

## `omh info --repo` — provenance ✅

Every value says where it came from and what it beat.

```console
$ omh info --repo
this repo /Users/you/proj/.omh

settings
  carry_in  [".env.local"]  ← local (overrides shared)
```

**No competitor does this.** It is what makes layered settings debuggable
instead of mysterious, and it is cheap — the resolver already knows the answer;
the only work was refusing to throw it away.

**It grew a second half when selection landed**, and the new half is the one
people will actually use. With a curated catalogue the interesting question
stops being "what is this set to" and becomes **"why is this skill not here"** —
so `omh info --repo` answers that too, naming what this checkout uses, what it
does not, and which of omh's features are off:

```console
using
  skills     review-diff   (1 more in your catalogue)

1 catalogue entry is not selected here: skills/refactor
  omh use skills refactor    ·    omh use --all
```

The table counts what else your catalogue holds; the line under it names what
this repo could take and has not.

See [Configuration](/reference/configuration/#two-scopes-two-commands).

## `omh doctor` — verification ✅

Proves an adapter's claims against a real container. Covered in
[Troubleshooting](/guides/troubleshooting/).

It belongs in this list because it is the answer to *"how do I know omh actually
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
  remove      omh set codegraph off — the feature, its server and its hooks
              together. Nothing is uninstalled and the next repo gets it back

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

**Read the shape of that output carefully, because it encodes the whole
position: the cost is measured and the benefit is argued.** Those are different
kinds of claim and the command does not blur them.

**Every cost carries its date and its method** — when it was taken, and how —
and one predating the current base-set version is marked stale.

<details>
<summary>Why staleness is judged against the base-set version</summary>

An earlier version compared against the entry's own `since`, which never moves
— so no shipped measurement could ever be flagged. A byte count in this repo
went wrong within a day and the check said nothing, because it was structurally
incapable of saying anything.

</details>

### Authorship is the point

**The base set is *seeded* into your profile at `init`** and then lives as
ordinary config, so an omh entry and one you added are byte-identical in the
same file. `why` recovers the difference by comparing against the manifest —
derived, never recorded, so there is no marker to go stale.

Nine answers, and the two negatives matter most:

- **your choice** — omh offers **no rationale**. A tool that answers "because it
  is in the base set" about something you added is lying about its own
  authorship, and telling the two apart is the whole feature.
- **written by omh init** — a `rust-format` hook derived from your `Cargo.toml`
  is omh's writing but not omh's opinion. Disowning it would be the same false
  claim pointing the other way.

Five more:

- **omh's own, generated at launch** — the hooks and rules sections the base
  manifest produces. They are not files anywhere, which is what lets a fix reach
  a repo initialised a year ago, and it means there is nothing of yours to
  compare: a file of that name is a leftover, and `omh why` names it as one.
- **not what omh ships now** — `init` seeds what it seeds once and never
  rewrites it, while the shipped baseline moves every release. So omh genuinely
  cannot tell an edit from an upgrade, and says so rather than picking the
  accusing guess. An earlier version called this *"modified by you"* and told
  every user they had edited a file they never opened.
- **omh's choice**.
- **not installed here**.
- **considered, not in the base set** — which is how a rejection stops being
  re-litigated every time somebody rediscovers it.

The last two are not about an entry at all:

- **an omh feature** — a feature with no entry of its own, such as
  `git-notice`. Feature names are what you type under `[omh]`, so a name omh
  tells you to type and then does not recognise would be this command failing
  on its own terms.
- **nothing recorded** — omh lists the names it does know rather than guessing.

Full output in [Commands](/reference/commands/#omh-why-thing).

## Measure the cost, argue the benefit

**omh does not benchmark its base set, on purpose.** The obvious missing piece
here used to be `omh bench` — a fixed task suite measuring
tokens-to-first-correct-edit with each base-set component on and off. It is not
on the [roadmap](/project/roadmap/), and the reasoning is worth keeping because
it is easy to re-derive the wrong way.

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

**It does not travel.** Twelve tasks in one Rust repo says little about a
400k-line TypeScript monorepo, which is exactly where the graph would matter
most.

**A fixed suite becomes the target.** The base set gets tuned to pass it.

**The premise was wrong, too.** No distribution has earned legitimacy through
evals. Debian does not benchmark its package choices; oh-my-zsh does not A/B its
plugins. What makes their curation legitimate is stated criteria, visible
reasoning, and trivial reversibility — which is what the rest of this page is.

### What still needs solving

One real thing. **Without a feedback loop, a base set only grows.** Arguments
for adding are always available, nothing ever leaves, and that is precisely how
oh-my-zsh got slow. Taste alone does not subtract.

**But the loop does not have to be an eval.** Split it by what is honestly
knowable:

| | approach | why |
|---|---|---|
| **Cost** | measured, deterministically | bytes injected per session, per tool call, per turn. No LLM, no variance, runs in CI. |
| **Benefit** | argued, in prose | it is a judgment call, and a number does not make it otherwise |

**That asymmetry is the right way round: cost is what creeps, benefit is what
you notice.** It also produces the retirement trigger that was actually wanted —
*"the base set costs 4.1 KB before you type anything, up from 2.3"* is a fact
that forces a conversation, and it is free to produce.

**The numbers already in [code graph](/guides/code-graph/#current-used-and-visible)
are exactly this shape**: 2,300 B for orientation, 1,511 bytes for one symbol
instead of a whole file, 0.14s to re-index. All real, none of them requiring an
eval harness — and the one that used to name a specific file's byte count is
gone, because it was stale within a day of being written.

This does mean the [weak spot](/concepts/distribution/#the-honest-weak-spot) is
answered by transparency rather than by proof. That is the honest position, and
claiming otherwise would need the benchmark to be trustworthy — which is where
this started.

## `omh eject` — the exit ✅

Write out the raw per-harness config and step aside.

**For an opinionated tool, a credible exit is what makes adoption safe.** You
are choosing to hand a tool your rules, credentials and sandbox policy; being
able to leave with all of it is the difference between a default and a cage.

It was nearly free to build, since omh already generates exactly these files.

**Together, these four make the opinion a default, not a cage.** An app store
cannot be overridden, because it never decided anything in the first place — the
freedom it offers is the freedom to do the work yourself.

## Next

<div class="cards">

- [`omh why`, in full](/reference/commands/#omh-why-thing)\
  Every answer it gives, and every flag.
- [The base set](/reference/base-set/)\
  What omh ships, with each entry's cost and how to remove it.
- [Troubleshooting](/guides/troubleshooting/#omh-doctor)\
  What each row of `omh doctor` checks, and what to do when one fails.
- [Why a distribution](/concepts/distribution/)\
  The thesis these commands keep honest.

</div>
