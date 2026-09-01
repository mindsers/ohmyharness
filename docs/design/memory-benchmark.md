# Measuring retrieval

**Status: designed, not run.** The harness exists as `scripts/bench-recall.sh`; the comparison it was written to settle has not been performed, so nothing on this page is yet a measurement.

> `./scripts/bench-recall.sh [--with-iwe] [--answers DIR]`

Every remaining question about [memory](memory.md) is empirical — is our
retrieval good enough, would iwe's be better, does declaring a note's questions
help, is a 90 MB embedding model worth it — and none of them can be settled by
argument. This is how they get numbers instead.

## What it does

1. **Builds a corpus from this repo's git history.** A commit body becomes a
   note; it already reads as *we expected X, we got Y*, which is the shape a
   `surprise` note has. Only commits with a real body qualify, which is 40 of
   them at the pinned ref.
2. **Builds the questions from the same commits' subject lines.** Same fact,
   different words — roughly 50% overlap. That is what a half-remembered
   question is, and it is the thing memory has to survive.
3. **Asks both engines every question**, and counts how often the right note
   came back first, in the top 3, and in the top 8 — plus how many results each
   returned to get there, since an agent has to read them.

The ref is pinned (`BENCH_REF`, default `1f351a4`) so the corpus does not drift
as the repo grows, and two runs a month apart stay comparable.

## Why it can be trusted

**Neither side is written by whoever wrote the ranker.** The notes are your
commit bodies and the queries are your commit subjects. Nobody chooses which
questions get asked, and nobody chooses what counts as the right answer — the
commit that produced the note *is* the answer.

That is not a nicety. §13 of the spec requires it: *"whoever writes the
questions must not write the curation prompt."* The first attempt at this
measurement ignored it and was worthless — queries were lifted verbatim out of
document bodies, which is exact-token matching's home turf, and omh "won" 83% to
66%. Inflecting the wording collapsed omh to 12.5% while BM25 held flat. The
number was real and meant nothing.

## Results so far

Measured 2026-08-09, 40 notes, 40 queries.

| engine | P@1 | top-3 | top-8 | results returned |
|---|---|---|---|---|
| omh `recall` | 47.5% | 65.0% | 90.0% | **8** |
| iwe `--lexical` (BM25) | 55.0% | 75.0% | 95.0% | 25.9 |

McNemar p = 0.51 — **statistically indistinguishable.** iwe is nominally ahead
by three queries out of forty, and buys its top-8 by returning 26 of the 40
notes where omh returns 8.

Two things follow, and the second matters more:

- **The engine is not the risk.** Adopting iwe would not measurably improve
  retrieval, and it cannot carry a note's layer, does not run on omh's base
  image, and rewrites identity on `rename` ([M0](memory-m0.md)).
- **Neither engine is good at this.** ~50% P@1 means the right note is not
  first half the time, because paraphrase is what a lexical ranker is worst at
  and neither does semantic retrieval. The mitigation is the one
  [§9.2](memory.md#92-recall) already specifies: return the neighbourhood, not
  the node. 90% of the time the answer is inside the budget of 8.

## The open experiment

A `surprise` note records `## Answers` — the questions it answers, in the
writer's words — on the theory that matching a question against a question
survives a paraphrase where matching a question against prose does not. The
evidence for the mechanism is indirect but strong: an index of nothing but
titles and headings scored 95.9% P@1 on question-shaped queries where the full
180 KB text scored 56%.

**It is untested for this application**, because it needs questions written by
somebody other than whoever wrote the ranker. Ranking them above prose was
tried, cost 10 points of P@1, and was reverted — a weight applies whether or
not the declared question is any good.

So the script emits notes with an **empty** `## Answers`, ready to be filled in:

```console
$ ./scripts/bench-recall.sh                       # writes target/bench/notes
$ # fill in each note's `## Answers` with scripts/answers-prompt.md
$ ./scripts/bench-recall.sh --answers <that dir>
```

### Result, 2026-08-09 — directionally positive, and underpowered

Questions written by an agent that had not seen the ranker, using
`scripts/answers-prompt.md`:

| | P@1 | top-3 | top-8 |
|---|---|---|---|
| no declared questions | 47.5% | 65.0% | **90.0%** |
| with declared questions | **55.0%** | **67.5%** | 87.5% |

Paired, the P@1 gain is **3 questions fixed and 0 broken**. That direction is
clean — a change that only helps looks different from a wash, where fixes and
breaks come out roughly symmetric — but three disagreements out of forty gives
p = 0.25, and top-8 moved slightly the *wrong* way (1 fixed, 2 broken), which
is what extra text pushing a marginal hit out of the budget looks like.

**This corpus cannot settle it.** Forty-one notes is everything the repo's
history affords; lowering the body-length threshold gains one. Detecting an
effect this size needs several hundred questions. The bar of "beat 47.5%" was
set without regard for statistical power, and clearing it is not the same as
demonstrating anything.

So the resting position is unchanged and, on this evidence, correct: **the
declared questions are recorded and indexed, and not weighted.** They cost
nothing, they are discipline, and a human reading the store wants them. The
weight stays out until a store exists that can justify it — which is what M1's
two-week gate produces, and what §13 wants for the real measurement anyway.

Two ways to get power, when it matters: run this against another repo with a
substantial commit history, or wait for a real store of agent-written notes.

## What this does not measure

- **Whether an agent answers better with memory than without.** That is §13's
  actual question and it needs a harness running real sessions.
- **Scale.** Forty notes. BM25's length normalisation matters more at a
  thousand.
- **Ingested doc stubs.** Those are a different question shape — navigational
  rather than experiential — and a different corpus.
