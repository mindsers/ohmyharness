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
codegraph — an omh feature, in the base set since 2026.06

  gathers     omh-codegraph   structural queries instead of re-grepping the repo every task
              graph-orient    the agent is given the module map instead of discovering it by reading files
              graph-first     structural questions answer in one call instead of an iterative grep
              graph-read      one symbol instead of a whole file, when the file is big enough for that to matter
              graph-refresh   the graph describes the code as it is now, not as the session found it
              graph-rules     a graph the agent has not been told about is a graph it never queries — the four hooks fire while it is already doing something, and this is what is true before it starts
  costs       omh-codegraph   0.46s to index this repo, cold             measured 2026-08-06
                              index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox
                              3.4 MB on disk                             measured 2026-08-06
                              the graph volume after a cold index of this repo
              graph-orient    2,300 B per context rebuild                measured 2026-08-06
                              get_architecture with four targeted aspects (2,138 B of payload) plus the hook's own framing prose
              graph-first     243 B per grep                             measured 2026-08-07
                              the nudge string for a `repo-sNN` project name, computed from the literals the hook ships — see the_grep_nudges_declared_cost_matches_the_string_it_ships
              graph-read      1,511 B when it fires                      measured 2026-08-06
                              get_code_snippet for one symbol, against reading the enclosing source file whole — this repo's largest was over 20x that. The file size is deliberately not recorded: it moves on every edit, and an earlier version of this line was already stale on the commit that wrote it.
                              0 B when it does not                       measured 2026-08-06
                              silent unless the target is source over 8,000 bytes
              graph-refresh   0.14s per turn                             measured 2026-08-06
                              incremental re-index of a session worktree at Stop
              graph-rules     673 B in the agent's context, every turn   measured 2026-08-12
                              the section as shipped, byte for byte. Computed by `every_rules_section_costs_what_it_says` against the string omh sends rather than typed in — a hand-written cost and the prose it describes have no relationship a test can check
  instead of  omh-codegraph   gitnexus                      PolyForm-Noncommercial licence
                              codegraphcontext              needs a Neo4j service running
                              @sdsrs/code-graph             close second; needs a node runtime rather than a static binary
              graph-orient    the `overview` aspect         6,173 B for the same orientation the four targeted aspects give in 2,138
              graph-first     blocking grep outright        grep is right for a literal string, and a hook that blocks correct work gets disabled
              graph-read      nudging on every Read         Read is the most frequent tool there is; a nudge on every call becomes noise the model tunes out
              graph-refresh   indexing once at init         a session's worktree is not the checkout it started from; the graph would describe code the agent has since changed
              graph-rules     the graph-orient hook alone   orientation is re-paid at every context rebuild and describes the modules; this names the tools and the project variable once. Different claims, and the hook's 2,300 B is the one that repeats
  remove      omh set codegraph off — the feature, its server and its hooks together. Nothing is uninstalled and the next repo gets it back; `omh settings set codegraph off` does the same for every repo `omh init` sets up after

  A feature is all or nothing: every part above is generated at
  launch and switched together, and none of them is a file of yours.

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

**omh's choices are features, and none of a feature is a file of yours.** A
server, its hooks and its section of the rules are generated from the manifest
at launch, so nothing omh chose sits in your catalogue looking like something
you added, and nothing you added can be mistaken for omh's. An earlier omh
seeded its servers into `~/.omh/mcp.json`, where they were byte-identical to
yours and the one part of a feature you could edit or delete on its own;
`omh upgrade` takes those copies out and names each.

Five answers, and the two negatives matter most:

- **your choice** — omh offers **no rationale**. A tool that answers "because it
  is in the base set" about something you added is lying about its own
  authorship, and telling the two apart is the whole feature.
- **written by omh init** — a `rust-format` hook derived from your `Cargo.toml`
  is omh's writing but not omh's opinion. Disowning it would be the same false
  claim pointing the other way.

Three more:

- **an omh feature** — asked by the feature's name, a part's, or a name a part
  used to have, the answer is the same: every part, what each is for, what each
  costs and what was considered instead, and the one switch. No part is
  answered for on its own, because no part can be had on its own. A file in
  your catalogue named like a part is a copy an older omh seeded, and the
  answer names it as not read.
- **considered, not in the base set** — which is how a rejection stops being
  re-litigated every time somebody rediscovers it.
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
