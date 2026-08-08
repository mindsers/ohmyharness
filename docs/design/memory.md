# Memory — specification

> **Status: M1 is built; §9's agent surface is not.** The store, its schemas,
> the key templates and `omh memory` / `lint` / `rm` ship — see
> [commands](../commands.md#omh-memory-) and §14 for what M1 deferred. Retrieval,
> the MCP surface, `promote` and `stale` remain specification. The reasoning
> behind each decision, the survey that picked the server, the benchmark that
> reversed six of these choices, and the alternatives not taken are in
> [how the design got here](memory-rationale.md).
>
> One thing here is still **unverified** and gates the build rather than sitting
> inside it. A second was verified in [M0](memory-m0.md), and cost this design
> four of the claims it rested on — see [open questions](#15-open-questions).

## 1. What this is

A **knowledge graph of linked Markdown notes**, scoped to a repo, that the agent
queries during a task and writes to when something surprises it. It survives
session removal and harness switches, which is what makes it memory rather than
context.

It is **not** a fact store keyed by topic, not a conversation log, and not a
replacement for `AGENTS.md`. `AGENTS.md` keeps the imperatives; the graph holds
what a question can reach.

The honest one-line case, since [`distribution.md`](distribution.md) demands one:
against an agent with `grep`, this wins on **cost, latency and growth — not
accuracy** — and it is the only way to keep what a removed session learned.

## 2. Scope

**In, v1**

- the note store, its layout and its two layers
- schemas and lint — the guards
- `remember` and `recall` over an omh-owned MCP surface
- stub ingestion of what the repo already documents
- `omh memory` — list, promote, rm, stale, lint

**Out, v1** — each with the reason, so it can be argued with:

| | why not now |
|---|---|
| **hub pages** | where the vendor's entire remaining quality gap lived; needs a calibrated lint first. [v2](#14-build-order) |
| **a curator pass** | omh has no session transcript to replay, and capturing one is a much larger commitment with secret-handling consequences ([rationale](memory-rationale.md#a-curator-pass-instead-of-in-task-writing)) |
| **a personal (`~/.omh`) layer** | machinery bought before anyone named a note needing it; `~/.omh/profile` already carries preferences as instructions |
| **hooks** | not harness-agnostic. An enhancement where present, never load-bearing ([§9.4](#94-hooks-are-an-enhancement-only)) |
| **semantic conflict sweeps** | measured to fail — rewriting wording breaks the phrasing retrieval matched on |

## 3. Vocabulary

| term | meaning |
|---|---|
| **note** | one Markdown file. One *topic*, richly filled — **not** one idea |
| **key** | a note's primary key, derived from metadata. Identity, not a title |
| **store** | all notes in one layer |
| **stub** | a note that points at a repo document rather than restating it |
| **hub** | a note whose job is joining others. v2 |
| **layer** | `team` (committed) or `local` (gitignored) |

## 4. Storage

```
<repo>/.omh/notes/              team  — COMMITTED, teammates get these
~/.omh/notes/<repo>/local/    local — outside the checkout, yours alone
```

> **Corrected during M1.** This section first put both layers in the checkout.
> That does not work, and the code says why: a session is a **git worktree**,
> which holds tracked files only, so a gitignored `<repo>/.omh/local/notes`
> never exists in the sandbox — and anything the agent writes under `/work` is
> destroyed by `git worktree remove --force` when the session is removed. That
> contradicts §1's *survives session removal* and §12's own example.
>
> The two layers therefore live apart, because their lifecycles differ. `team`
> is committed, so it is *tracked* — which is what makes it retrievable in a
> fresh clone, and what puts it inside every worktree for free, needing no
> mount. `local` is keyed by repo under `~/.omh`, exactly as the graph cache
> is, and is bind-mounted into the sandbox at `/omh/notes/local`.
>
> Deliberately **not** under `/work`: the code graph would index notes as
> source, `git status` would show them, and an agent running `git add -A` would
> commit local notes onto its session branch — the exact leak [§9.1](#91-remember)
> forbids.

`omh init` creates both. The committed layer needs no ignore rule; the local one
needs no ignore rule either, because there is no git where it lives. The earlier
claim that `init` adds `.omh/local/` to `info/exclude` was doubly wrong:
`info/exclude` is per-clone and never travels, so it could not have hidden a
store from a teammate anyway. `init` already writes a committed `.omh/.gitignore`,
which is the mechanism that does travel.

**Layers do not merge.** `policy.toml` merges key by key because a setting has one
value; a note is a *claim*, and two claims about one topic are two facts. So the
layer is **part of a note's identity**: `team/deploy` and `local/deploy` are
different notes and both retrieve. Shadowing would hide a teammate's note behind
yours and the reader would never learn it existed.

## 5. Note format

Frontmatter is required and schema-enforced. A note carries, at minimum:

```markdown
---
key: credentials-file-mount-returns-ebusy
type: surprise | topic | stub
source: session s03, claude
recorded: 2026-08-07
invalidated_by: image:sha256-4f2a…       # optional; see §8
---

# Mounting a credential file returns EBUSY

**Expected** a bind mount of the token file to persist the login.
**Observed** the harness rewrites in place; a file mount is one inode, so the
write fails with `EBUSY`.

Mount the *directory*, never the token file.

Related: [[credentials-are-a-named-volume]]
```

Rules that are **schema-enforced** (a violating write is refused):

- `key`, `type`, `source`, `recorded` present **and non-blank**; `recorded` is a
  date that exists — `\d{4}-\d{2}-\d{2}` was the rule written here first, and it
  is the weak version: it accepts `2026-13-45`. Month and day ranges are
  calendar facts, not calibrated thresholds, so the strong version costs nothing
- sections are **`## Name` headings**, never words appearing somewhere in the
  body. `body.contains("Expected")` is satisfied by a sentence, which is the
  failure this repo already shipped once as a staleness guard
- required sections by name, per `type`
- **budgets per section**, never a flat per-page cap
- structural shape where it substitutes for a threshold — *bullets only, no prose
  blocks* in any list-like section

Rules that are **prompt-only**, because nothing can check them:

- store uncertainty, not false precision — record the relative wording *with* its
  resolution, and never invent precision the source did not give
- date by **occurrence**, not by mention
- record evolving state as **dated status lines**, not by rewriting in place

## 6. Identity

**A key is a primary key.** Writing to an existing key is a conflict error whose
message says *update instead*, never a silent second copy.

Keys are **derived from metadata by configured templates**, not chosen by the
agent, and omh ships the templates at `init`:

```toml
[keys]
surprise = "surprise/{{slug}}"
stub     = "docs/{{path}}"
```

Derivation must be **canonical** — any input with two spellings needs a pinning
rule (alphabetical order, normalised case). *Both keys being unique is not the
same as the identity being unique.*

`--if-exists` makes retry policy an argument: `skip` for idempotent ingest,
`suffix` or `override` only when named. **Skip-if-exists is an explicit mode, not
a fallback.**

This replaces *"search before writing a note"* as a rule. That imperative stays
as a backstop only; it was measured not to hold as a prompt.

## 7. Guards

Three enforcement layers, each owning what it can actually check:

| layer | owns | on violation |
|---|---|---|
| **schemas** | shape — sections, per-section budgets, block types | **refused write** |
| **lint** | links — dangling, orphans, near-duplicates | **warning, re-fires every session** |
| **prompts** | semantics — which date, what deserves a line | nothing |

Two laws attach, and both are build requirements:

**Gate what you cannot afford to lose; let the rest apply pressure.** Agents
negotiate with warnings and cannot negotiate with a refused write.

**Every threshold is calibrated against a known-good store before it ships.** A
budget that cannot separate a store you believe in from one you don't does not
ship. Prefer the check with no number in it.

`omh memory lint` is also the **store-quality meter**: violation counts are a
write-time proxy for store quality, available with no questions asked and no LLM
pass.

## 8. Expiry

`invalidated_by` takes one of a closed set omh can evaluate itself:

| kind | invalid when |
|---|---|
| `file:<path>@<hash>` | the file's hash changes |
| `image:<digest>` | the sandbox image is rebuilt |
| `base:<version>` | the base set is re-cut |
| `symbol:<name>` | the code graph no longer contains it |
| *(absent)* | never automatically — carries only its date |

`omh memory stale` is a **join against facts omh already holds**, not a
judgement. Notes with no `invalidated_by` are the honest residue: derived from
experience, with a date and nothing else.

**Deletion never cascades.** `omh memory rm` removes one note and reports what
linked to it. A dangling link is visible and the lint already finds it; a silently
pruned neighbourhood is neither. This is the same rule that makes `omh s rm` keep
a branch holding commits — [fail toward the recoverable
mistake](memory-rationale.md#fail-toward-the-recoverable-mistake).

## 9. The agent surface

omh exposes **its own MCP server**, proxying iwe, with two tools. This is a
commitment, not a convenience — see [§9.5](#95-why-omh-owns-the-surface).

### 9.1 `remember`

```
remember(
  expected,         # what you thought would happen
  observed,         # what actually happened
  evidence,         # the command, the error, the file
  relates_to[],     # keys of notes this connects to
  invalidated_by,   # optional; §8
)
```

The signature *is* the discipline. An agent with nothing to put in `expected` has
learned nothing worth recording, so the filter runs for free, and provenance
becomes a parameter that cannot be omitted rather than a rule that can be
violated.

`relates_to` takes **keys, not titles** — a key is computable before its target
exists.

Writes go to **`local` only.** An unattended writer that could reach the committed
layer would push wrong facts to teammates through git, where they arrive with the
authority of a reviewed change.

**Strict mode is always on over MCP** and opt-in on the CLI: a human with git
behind them should not pay ceremony, and the population most likely to skip a
guard is the one that needs it.

### 9.2 `recall`

One call. Rank, expand one hop, return the neighbourhood:

```
credentials-are-a-named-volume              team · 2026-06-12
├─ credentials-file-mount-returns-ebusy     local · 2026-08-07   ← the why
├─ accounts-are-single-path-components      team · 2026-06-14
└─ the-image-ends-unprivileged              team · 2026-07-02
```

**Every result carries its date and its layer.** This is the invariant the whole
feature rests on ([§11.1](#11-invariants)) — a note presented without age and
origin cannot be judged, and unattended writing then becomes a machine for
laundering guesses into facts.

**Retrieval never picks a winner.** Contradicting notes both return; the agent
reconciles with layers and dates in hand, which is what an LLM is good at and an
indexer is not. When it must choose: **layer outranks recency** — a promoted note
passed human review, a local one did not — and within a layer, recency and task
context decide. Ask the user only when two *promoted* notes disagree or the
conflict blocks the task.

### 9.3 The index rides in the tool description

The agent has **direct access** to the graph; injection only solves the trigger
problem. omh generates `recall`'s description per repo, at launch:

> `recall(question)` — search this repo's accumulated notes. The store holds 31
> notes: 9 credentials, 8 sandbox, 6 code graph, 8 other. Most exist because an
> assumption turned out wrong. Query before assuming how something here works.

**Counts, not titles**, so the injected cost stops growing with the graph. This
carries index and trigger together, in every harness, with no rules file — and it
arrives attached to the call rather than competing inside a document that decays
as context grows.

Consequently **`AGENTS.md` carries nothing for retrieval**. It keeps only what no
tool description can: *record what surprised you*, *rename through the tool,
never `mv`*.

### 9.4 Hooks are an enhancement only

`claude` declares rules, mcp and hooks; `opencode` declares rules and mcp. Only
**rules and MCP are universal**, so nothing load-bearing may sit on a hook. Where
one exists, a `Stop` hook sharpens write timing. Ingestion needs no hook at all —
the server watches the directory, so a `git pull`, a branch switch, a human's
editor and the agent's writes are all just files changing.

### 9.5 Why omh owns the surface

The provenance envelope in §9.2 **cannot be enforced on a server the agent talks
to directly.** That is the reason; the rest are benefits: the per-session tool tax
drops from [iwe's 14 tools](memory-m0.md#7-the-per-session-tool-tax-measured) to
2, the one-shot shape gets implemented rather than prompted, and one proxy can
front both graphs.

The cost, stated so it can be revisited: it puts omh **in the request path**, and
it edges toward the abstraction layer [`distribution.md`](distribution.md) says
omh is not. The defence is that a proxy adding provenance is not a knowledge
graph. **If that defence stops holding, this is the first thing to cut.**

## 10. Ingestion

`docs/`, ADRs and `detect::seeds()` become **stubs** — one note per document, one
line, a link, the questions it answers. Not summaries: curation summarises away
the verbatim detail coding work needs (the flag, the path, the error string), and
a distilled copy drifts from `docs/` undetectably.

This makes the store useful on day one. It is the **floor**; agent writing is the
growth path, and the growth path is where the feature is irreplaceable.

It also closes a live loop: `detect::seeds()` already derives these facts and
throws them away in a `println!`.

## 11. Invariants

Each is a build requirement with its enforcement named. **Unit-testable** ones are
guards in `cargo test`; the rest are honest about not being.

| # | invariant | enforced by |
|---|---|---|
| 1 | a note is never retrieved without its date and layer | unit test on the proxy's render |
| 2 | a committed note links only to committed notes | unit test; checked again at `promote` |
| 3 | `remember` writes only to `local` | unit test |
| 4 | every note has `key`, `type`, `source`, `recorded` | schema; refused write |
| 5 | a key collides at most once — writing an existing key errors | unit test |
| 6 | key derivation is canonical: one input, one key | unit test over spelling variants |
| 7 | `rm` never cascades | unit test |
| 8 | every schema threshold was calibrated against a known-good store | **process**, recorded in the entry |
| 9 | the harness reads the tool description per session | **`omh doctor` proves the precondition only** — see below |
| 10 | the agent actually writes notes worth keeping | **observable only.** See [§13](#13-measurement) |

**Invariant 9 was overstated, corrected during M2.** `doctor` *cannot* enforce
it: it replaces the launch command with its probe, so no harness ever starts,
and a tool description is consumed by a model rather than written anywhere
inspectable. What `doctor` does prove is that the server omh configured
actually starts where the harness will spawn it, answers `tools/list`, and
names both tools — reporting the store's own census, because `0 notes` is what
a wrong mount looks like and a blank detail hides it. Whether a *harness*
re-reads the description per session is [§15.2](#15-open-questions), and it is
a dated measurement per harness, not a check. Claiming more than `doctor` can
prove is the failure [contributing](../contributing.md) names.

Invariants 8–10 are the ones that will be tempting to fake with a weak test. This
repo has shipped that failure — a date guard that only checked a date was
*present*, a `GUEST_HOME` guard matching `const` but not `pub const`. **Write the
guard red first, then reintroduce the defect to confirm it bites**
([contributing](../contributing.md)).

## 12. CLI

```console
$ omh memory                    # list, by layer, with reference counts
$ omh memory promote <key>      # local → team; checks invariant 2 first
$ omh memory rm <key>           # one note; reports inbound links
$ omh memory stale              # join against §8 events
$ omh memory lint               # schema + hygiene violations
```

`promote` is the **only place a human gates anything**, because it is the only
place a wrong note reaches somebody else. Everything else is invisible: no
approval during work, no interruption — a memory you have to approve is a
notebook, and nobody keeps one.

The review moment rides on something already happening rather than a ritual
nobody performs:

```console
$ omh s rm s03
removed s03 (2 commits, merged)
3 notes recorded during this session — `omh memory` to review
```

## 13. Measurement

[`base-set.md`](base-set.md) requires a measured cost, a stated benefit,
alternatives, and a removal command before this ships as an entry. The removal
command is `omh config mcp rm memory`; the alternatives are in
[the survey](memory-rationale.md#survey-what-already-exists).

The measurement is **this repo's own history** as a question set — `EBUSY` on a
bind-mounted token file, the installer ignoring `CBM_VARIANT`, `--aspects` on a
comma list, `info/exclude` in the common git dir — asked of a session with the
graph and one without.

**Whoever writes the questions must not write the curation prompt.** Knowing the
answers while authoring the instructions is exactly the contamination that cost
the vendor 0.15 J, and it stays invisible until somebody re-reads the prompt
against the corpus.

Until that runs, this feature's status is **unverified**, however good the design
reads.

## 14. Build order

Store quality dominates and tooling comes second, so retrieval is built last.

| | ships | done when |
|---|---|---|
| **M1** | layout, schemas, key templates, `remember`, `omh memory` / `lint` / `rm` | the store has run on this repo for two weeks and been read by a human |
| **M2** | stub ingestion, `recall`, the generated tool description | invariant 1 holds under test; invariant 9 checked by `doctor` |
| **M3** | the `team` layer, `promote`, invariant 2 | a note promoted here is retrievable in a fresh clone |
| **M4** | `invalidated_by`, `omh memory stale`, hub pages | `stale` fires on a real image rebuild |

**M1's gate is a read, not a green test.** If the store is bad, no retrieval
architecture rescues it — and discovering that after M1 is cheap.

**What M1 actually shipped**, recorded here because invariant 8 makes the
deferrals *process, recorded in the entry* rather than a silence:

- **Zero numeric thresholds.** Every rule is satisfied-or-not: fields present
  and non-blank, dates that exist, types in a closed set, sections present as
  headings, *bullets only* in list sections, keys matching filenames, set
  membership for links.
- **Deferred: per-section budgets.** They need a corpus. The last 300-word cap
  tried in this space became a fossil and *caused* the newest failure class.
- **Deferred: the near-duplicate lint.** §7 lists it under `lint` and §14 puts
  `lint` in M1, so this is a real scope deviation and not an oversight. It needs
  a similarity threshold, a threshold cannot be calibrated against a store that
  does not exist, and an uncalibrated one is a guess wearing the authority of a
  gate.
- **No refused write for the agent yet.** §7 gives schemas the power to refuse,
  and over MCP they will. M1 has no MCP surface, so the agent writes Markdown
  with its ordinary tools and `omh memory lint` is the guard. The schema still
  refuses on omh's own write path; it simply is not yet in front of the agent.
  This is the one place M1 is weaker than §7 describes, and M2 closes it.
- **The agent picks its own key.** §6 derives keys from templates, and
  `remember` does — but M1's only writer *for the agent* is a hand-written
  file, and the staged rules tell it to key the note after the filename. So
  two identity schemes share the store until the MCP surface lands.
  `DuplicateKey` makes the collision visible; it does not prevent it.
- **Deferred: a `Key` type.** A key is a primary key with a canonicality rule,
  and it lives as `String` throughout. `expand_key` validates what it mints, so
  no path can leave the store — but nothing distinguishes a canonical key from
  arbitrary text in a signature. M2 adds a second key-minting writer (`stub =
  "docs/{{path}}"`), which is where this stops being cosmetic.

**What M2 actually shipped**, with the deviations named:

- **Retrieval is omh's own**, not a proxy over iwe. §9 said "proxying iwe";
  [M0](memory-m0.md) found iwe will not run on omh's base image, that `rename`
  produces notes omh's schema refuses, and that it structurally cannot carry a
  note's layer. `recall::search` is the seam an iwe-backed retriever would
  replace without touching the tool surface or invariant 1, so the choice stays
  open — and §13 is what should settle it.
- **`recall` returns the tree, not note bodies.** §9.2's own example is a tree.
  Prose cannot pass the strict, total parser that guards invariant 1, and
  weakening that parser to admit it would make the guard vacuous. If a second
  call turns out to be common, that is evidence to add bodies — not a guess.
- **The stub key template is `{{path}}`, not §6's `docs/{{path}}`.** The path
  already carries its directory; the latter keys `docs/design/memory.md` as
  `docs/docs/design/memory`.
- **Ranking is by term rarity over whole words.** The first version compared
  substrings and retrieved a ten-note store in full for the question `"a"`.
  No stopword list: rarity is derived from the store, so nobody decides which
  words are noise.
- **Not done: the §15.2 caching experiment**, which needs a real harness launch
  to measure, and **§13's benchmark**, which needs a question set written by
  somebody who did not write the store.

## 15. Open questions

One blocks the build; one is answered and kept for the record. The rest are
recorded so they are not rediscovered.

**Blocking**

1. ~~**iwe has never been run in an omh container**~~ — **answered 2026-08-08**,
   in [M0](memory-m0.md). It was worth running: four of the claims this design
   rested on did not survive it — one the survey made, three carried here. iwe
   **does not run on omh's base image at all** (needs glibc 2.39, bookworm ships
   2.36, and no musl build is published); it is three dynamic binaries totalling
   64 MB rather than one static one; `iwe rename` breaks the identity model in
   §6, and names guards §6 and §7 still owe; and the tool count is 14, not 13.
   Apache-2.0 and the no-service claim hold. The
   decision this gates is *whether to adopt iwe*, not whether to build — the
   store is plain files omh owns, and `recall::search` is the seam a different
   retriever would replace.
2. **Does each harness read MCP tool descriptions per session, or cache them?**
   §9.3 fails if they cache. Fallback: the index returns to the staged
   `AGENTS.md`, which omh regenerates every launch anyway.

**Non-blocking**

3. Two notes on topics that never co-occur in a query can contradict
   indefinitely. The near-duplicate lint catches the clustered case; this one
   stays open, and the obvious fix was measured to fail.
4. Whether the surprise trigger produces enough notes to matter — [§13](#13-measurement).
5. Whether a cheap model should write while a strong one reads. Not actionable
   while omh [does not choose your model](decisions.md#decisions-deliberately-not-made).
