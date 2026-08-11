# Memory — how the design got here

Companion to [the specification](memory.md). That page says **what to build**;
this one says **why**, what was tried, and what this design got wrong on the way.

Kept rather than deleted because most of it is reversals, and a reversal is the
most useful thing a design document holds — [decisions](decisions.md) makes the
same argument. If you are about to change something in the spec, the reason is
probably here.

## The reframing

An earlier version described memory as a **layered fact store**: personal facts,
project facts, team facts, merged with provenance like `omh config`. That
answered the wrong question.

The idea is closer to the code graph, pointed at everything that is not code: a
graph of linked Markdown notes the agent can query and grow. Concepts,
definitions, decisions, conventions, things learned the hard way, each linked to
the others.

Two things it buys:

**A smaller `AGENTS.md`, holding only instructions.** Everything that is a *fact*
moves out; what stays is what the agent must know before it acts.

**Retrieval instead of recitation.** A large `AGENTS.md` is loaded whole into
every session and models attend poorly to the middle of long text. A graph is
queried: three relevant notes instead of four hundred lines.

That second point was written as the real argument, and it is the same one that
justified the code graph. It needed qualifying — see
[the benchmark](#what-the-benchmark-changed): measured against a competent agent
with `grep`, a curated graph wins on **cost, latency and growth**, not on
accuracy.

## Instructions are not knowledge

The word "replace" hides a boundary that has to be drawn explicitly.

**Instructions must already be in context**, because the agent cannot look up a
rule it does not know it needs. *"Write the failing test first"* has to arrive
before it starts writing code; nothing will prompt it to go and ask.

**Knowledge can be fetched**, because a question triggers it. *"What does
`carry_in` do?"*, *"why was gitnexus rejected?"* — all reachable on demand.

So `AGENTS.md` does not disappear. It shrinks to imperatives.

### What that actually changes, which is not what it sounds like

Today's shared `AGENTS.md` is **52 lines in four sections — and all four are
imperatives or routing instructions**: prefer the graph for structural questions,
the stack's commands, TDD always, honesty about coverage.

There is almost nothing to move out. So the win is **not shrinking the file**. It
is preventing the growth that happens when knowledge has nowhere else to go —
every fact the agent learns gets appended until the file is four hundred lines
and its middle is unread, which is precisely the failure this feature exists to
avoid.

Stated as a rule: **`AGENTS.md` is a closed set of instructions, not an open
ledger.** If it is growing, something belongs in the graph.

## The hard problem: retrieval requires knowing to retrieve

This repo already learned this once. From
[code graph](../code-graph.md#current-used-and-visible): *"An MCP server on its
own is inert — indexed once, never refreshed, never reached for."* Four hooks
were what turned it into something used.

Knowledge is **harder** than code, because the trigger is less obvious. An agent
about to grep knows it is looking something up. An agent that does not know a
convention exists has no reason to ask whether one does.

## Survey: what already exists

The base set was chosen by surveying candidates and disqualifying most of them
([code graph](../code-graph.md#why-this-one)). Memory got the same treatment.

This space is **crowded**, which is itself a finding — it argues against building
a server and for choosing one.

| | What it claims |
|---|---|
| [Meshnote](https://github.com/TensorBlock/awesome-mcp-servers/blob/main/docs/knowledge-management--memory.md) | agent-maintained wiki for coding agents: Markdown, YAML frontmatter, `[[wikilinks]]`, BM25 search, backlink graph, per-topic "brains" each seeded with a `schema.md`, version history, valid Obsidian vault on disk |
| [iwe](https://github.com/iwe-org/iwe) | Markdown knowledge graph exposed as **both an LSP and an MCP** — the editor and the agent read the same graph |
| [Basic Memory](https://mcpmarket.com/server/basic-memory) | persistent knowledge graph from conversations, Markdown on disk, LLM reads and writes |
| [mwe-mcp](https://github.com/Fr4nZ82/mwe-mcp) | Markdown wiki, multi-user with per-fragment ACL, self-organising overnight. **AGPL** |
| [agent-wiki](https://github.com/xinhuagu/agent-wiki) | turns documents, code and project context into portable retrievable memory |
| [linksee-memory](https://github.com/michielinksee/linksee-memory) | local-first cross-agent memory with a token-saving file diff cache |
| [Obsidian Memory MCP](https://lobehub.com/mcp/yunaga224-obsidian-memory-mcp) · [mcp-obsidian](https://github.com/Piotr1215/mcp-obsidian) | the read side against an existing vault |

### Evaluated on the criteria that decided codegraph

Not on features — features were never what disqualified a candidate there.

| | licence | runtime | storage | active | writes? |
|---|---|---|---|---|---|
| **[iwe](https://github.com/iwe-org/iwe)** | **Apache-2.0** | **none — Rust, single binary** ([wrong](memory-m0.md#2-it-is-not-a-single-static-binary): three, dynamic) | plain `.md`, **no database** | 308 commits, current | yes |
| [Basic Memory](https://github.com/basicmachines-co/basic-memory) | **AGPL-3.0** | **Python 3.12+** | `.md` + **SQLite** | 1,649 commits, 3.6k stars | yes |
| [mwe-mcp](https://github.com/Fr4nZ82/mwe-mcp) | **AGPL-3.0** | not checked | not checked | not checked | yes |
| others | not checked | | | | |

**iwe wins**, and not narrowly on the criteria this project uses:

- **Apache-2.0.** No repeat of the gitnexus problem, where a noncommercial
  default would have put every user writing code at work in violation.
- **Rust, single static binary, no database.** Exactly what won it for codegraph
  — *"a service to run is a decision `omh init` promised to remove"*.
  ([M0](memory-m0.md#2-it-is-not-a-single-static-binary) later measured this:
  the no-database half holds, the single-static-binary half is wrong.)
- **It is also an LSP.** The editor [attached to the session](../editors.md) and
  the agent read the same graph. Nothing else on the list does this.

Basic Memory is the more established project by a wide margin, and that is a real
argument. But AGPL as a *distribution default* is a decision omh has already
refused once, and a Python runtime is a cost the base image does not pay.

**This was reading, not running.** Every claim above comes from documentation,
which is precisely the class of claim this project treats as unverified: the
graph server's own docs advertised a `CBM_VARIANT=ui` switch its published
installer ignored. That is why
[running iwe unattended in a container](memory.md#15-open-questions) was a gate on
the spec rather than an assumption inside it.

**It has since been run**, and the gate earned its keep: [M0](memory-m0.md) cost
this design four of the claims it rested on, one of them from the table above.

## What the benchmark changed

iwe published *The benchmark that built the tools* ([iwe.md](https://iwe.md),
12 July 2026): a LOCOMO run measuring a markdown knowledge graph **as agent
memory**, with the answering agents, the curator and the judge all running
through `claude -p`. The harness, prompts, judge and full run ledger are
published alongside it.

A companion piece, *Designing edit operations for AI agents* (1 August 2026),
gives the mechanism behind the guards — and is the more directly useful of the
two, because it shows which disciplines became **configuration** rather than
prompt text.

These are the vendor's own numbers, on the vendor's own product, and that is
normally where this project stops reading. Three things make it an exception:

- **its own baseline beats it.** Plain `grep` over the raw source outscores the
  knowledge graph on accuracy, and the post leads with that.
- **it retracts its best number.** A worked example in the curation prompt had
  been lifted from the corpus and contained a gold answer. They quarantined the
  run, re-curated, and watched 0.90 fall to 0.75 — *after* drafting conclusions
  from it. This repo shipped seven fabricated dates in a single session; a team
  that catches its own contamination and publishes the quarantined runs is making
  the same argument omh makes with `measured … on`.
- **it separates what survived reruns from what did not**, explicitly.

What it is *not* is evidence about our setting. LOCOMO is months of text messages
between two friends. Coding work is a different corpus, a different question
distribution, and a different write cadence. Every number below transfers as a
**hypothesis**.

### The headline is uncomfortable, and it should stay that way

On sealed test data, 351 questions:

| arm | J |
|---|---|
| grep over the raw transcripts | **0.812** |
| grep over the curated notes | 0.764 |
| the multi-turn graph-tool agent | 0.735 |

The knowledge graph lost to `grep`, and lost again to `grep over its own notes`.
Rebuilding retrieval as a single search-and-expand call closed the accuracy gap
but never opened one: the defensible claim is **parity on accuracy, an order of
magnitude on economics**.

| | one-shot over notes | grep agent |
|---|---|---|
| turns per question | 1.0 | 4.6 |
| context read | 20.8k tok | 77.8k tok |
| latency p50 | 1.9 s | 11.6 s |
| growth as history grows | **bounded by construction** | linear |

That last row is the whole argument, and it is the one a 199-question benchmark
is structurally unable to show.

**This produced a split the design did not have.** omh's graph has two halves,
and only one has a `grep` baseline to lose to:

- **Ingested repo docs.** An agent in an omh session *has the repo* and can grep
  it. Per this benchmark it would do about as well that way, at four times the
  turns. The graph's value over this half is cost and latency, not answers.
- **What the agent learned** — `EBUSY` on a bind-mounted token file, an installer
  ignoring its documented variant. **There is no source to grep.** The session is
  removed; the transcript is gone. Without a note this is simply lost.

So the "useful on day one" floor is weaker than it was written, and the growth
path is stronger. **The experiential half is where the feature is
irreplaceable.**

### Guards beat instructions, measured

A **store lint** — per-page-kind token budgets plus store-wide checks for
dangling links, orphan pages and near-duplicates, with violations riding back on
every write — moved the score 0.70 → 0.77 in one run. Three successive prompt
revisions had failed to move it at all. The page the prompt could not keep under
control went from 2,408 words to 770; the hand-built reference is 767.

The mechanism is not obedience. The curator **argued with about half the
warnings** — *"these are expected here"*, *"this is the intended architecture"* —
and partial compliance was still enough, because:

> A linter differs from an instruction in one decisive way: it re-fires every
> session. Prompts decay as context grows; pressure that reapplies itself
> doesn't.

That sentence is why this feature cannot rest on `AGENTS.md` wording, and it
generalises well past memory.

**The enforcement taxonomy**, which is the reusable part:

| layer | owns | failure mode |
|---|---|---|
| **schemas** | shape — required sections, budgets per section, block types | hard gate: the write is refused |
| **graph hygiene** | links — dangling, orphans, near-duplicates | warning that re-fires until fixed |
| **prompts** | semantics — which date is the event's, what deserves a line | nothing can check it |

With a rule attached: *agents negotiate with warnings but cannot negotiate with a
refused write.* Put what you cannot afford to lose behind a gate; let the rest
apply pressure.

**And a calibration law this repo has already paid for twice.** Every numeric
threshold they tried flagged the *known-good* store first — line-length caps had
to be dropped for exactly that reason. Structural rules were robust on arrival;
numeric ones were not.

> A budget that cannot separate a known-good store from a known-bad one doesn't
> ship.

That is [the weak-guard problem](../../.github/CONTRIBUTING.md) in a new costume: a check
that passes everything and a check that flags everything are the same failure.

One free result worth stealing outright: validating three generations of stores
against the same schema **ranked them exactly as their benchmark scores had** —
zero violations / 0.81, 22 / 0.77, 272 / 0.70. A store-quality signal available
at write time, with no questions asked and no LLM pass.

### Identity is configuration, not discipline

**A note's key is a primary key.** Creating at an existing key is a conflict
error whose message says *update instead* — not a silent second copy. Derive the
key from metadata rather than title wording and a whole failure class disappears
structurally: the same event cannot be recorded twice, a crashed ingest retries
idempotently, and links become computable before their target exists.

They learned it from its absence: keys minted from title wording made every retry
and rephrasing a fresh identity, and *"the store quietly accumulated
near-duplicates that no prompt admonition prevented."*

The guarantee is only as strong as the derivation is canonical — their first run
created `joanna-and-nate` and `nate-and-joanna` as two pages. **Both keys were
unique; the identity wasn't.** Fixed by requiring alphabetical order, for the
same reason compound database keys fix a column order.

**And the derivation is configuration.** A `key_template` in project config —
`journal/{{today}}`, `people/{{slug}}` — mints the key from metadata, so the
canonicalization rule *"lives in project config where the agent cannot re-decide
it"*. Collisions fail by default, and `--if-exists` turns the retry policy into
an argument: `skip` for idempotent ingest, `suffix` or `override` only when
named. **Skip-if-exists is an explicit mode, not a fallback you hope for.**

That converts the strongest guard in this design from something omh must build or
prompt for into a config file `omh init` writes — which is exactly the shape a
distribution wants: the opinion ships as data.

### Enforcement lives at the surface, not the grammar

> Humans get freedom, agents get strictness, the language means the same thing
> everywhere.

Concretely: `expect` guards are opt-in on the CLI (`--strict`), because *"a human
with git behind them shouldn't pay ceremony"*, and **always on over MCP**,
because *"the population most likely to skip the guard is exactly the one that
needs it"*.

This matters here because omh has **both audiences on one store, by design**. The
[editor is SSH'd into the same session](../editors.md) as the agent, so a human
edits notes by hand in exactly the directory the agent writes to over MCP. A tool
enforcing uniformly would have to pick which of the two to annoy.

The workflow the guards create on the agent side is **locate, count, pin,
mutate** — find the targets, learn the count, write the count into the edit, and
the store refuses an edit whose shape surprises you.

### Fail toward the recoverable mistake

Their first draft gave update operators subtree semantics: replace a header,
replace its whole section. Testing showed a data-loss trap — an agent "renaming"
a header by replacing it silently destroys everything beneath. Shipped semantics
follow text-editing intuition instead.

> The failure modes now point in the safe direction — the recoverable mistake
> (leftover visible content) instead of the silent one (a vanished subtree).

That principle is already load-bearing in omh under other names, and is worth
recognising as one rule rather than three coincidences:

- [`omh s rm`](../sessions.md) keeps a branch that holds commits
- [idle reaping](risks.md) leaves a session with no recorded use alone, because
  *"stopping a container on a guess is worse than one extra container"*
- only the worktree and credentials mount writable

### Retrieval is one call, not a walk

The multi-turn tool agent was the **worst and most expensive arm** — 8.1 turns,
176k tokens, $0.153 a question, 0.735. Replacing it with a **one-shot dossier**
— rank once, take the top pages, expand what they link to, inline it, answer in
one turn holding zero tools — cost a nickel a question and scored higher.

> Everything graph-aware moves to write time and to that one retrieval call;
> nothing agentic remains at answer time.

It is a partial fit, and the difference matters. Their agent answers a question
and stops; an omh agent is doing a coding task where retrieval is incidental, and
cannot pre-fetch a dossier for a question nobody asked. What transfers is the
shape: **one call should return enough that a second is rarely needed** — the
neighbourhood, not the node.

### The graph is structure, not an interface

The one-shot result reads like an argument against graphs and is the opposite.

| | verdict |
|---|---|
| agent walks node → node over 8 turns | **measured worst arm** — 0.735, $0.153/question |
| one call ranks, then expands what the top pages link to | the shipped architecture |
| linter audits topology — dangling, orphans, near-duplicates | largest single improvement |

What died is the graph as something the agent *browses*. Their account of why:
*"the graph already encodes which pages belong together"* — so the traversal
happens once, on the agent's behalf, instead of being handed over as eight turns
of exploration.

### The curation rules that moved the number

Prompt rules were not useless — they were where *semantics* lived, and they took
the temporal category from 0.60 to 1.00 on their own. Three earned it:

- **Store uncertainty, not false precision.** Record the relative wording *with*
  its resolution — *"the Sunday before 25 May 2023 (21 May 2023)"* — and never
  invent day-precision the source did not give.
- **Date by occurrence, not by mention.** An event is dated by when it happened,
  never by when it came up.
- **Record evolving state as dated status lines**, rather than rewriting a fact
  in place.

All three transfer cleanly. *"`--aspects` returns empty on a comma list"* has a
date it was discovered and a version it was true of; a note saying only
*"`--aspects` is broken"* is the false-precision failure in another form.

### Hub pages

Their multi-hop answers came from pages whose only job is to **join threads that
live apart** — a relationship page recording what two people have in common,
created because the answer was scattered across two person pages.

Two things to carry:

- **This is where their entire remaining quality gap lived.** Hub bloat survived
  three prompt versions; capping it moved the bloat to the relationship page;
  demanding terseness made the agent squeeze out the dates and collapsed the
  temporal category. It was fixed by a lint, not by wording.
- **The pattern that closed their last gap:** the event's date as the *link
  text*, the page as the *target* — `finished the migration on [2026-06-12]` — so
  the hub line answers *when* without opening anything, and the link still points
  at the evidence.

### When compliance looks random, suspect the product

The defect that survived every guard: hub entries kept carrying the full title of
the page they linked to, where the curator had been told — in three successive
prompt versions — to write just the date.

It was the renderer. iwe regenerated link text from the graph on every write that
touched a page, overwriting what the curator wrote, whatever the prompt said. The
tell was there the whole time: *"the only links the renderer couldn't rewrite
were the ones pointing at pages that didn't exist yet. Obedience correlated with
broken links."*

> A unit test checks what you thought to assert; this defect was invisible to
> every test because the renderer was doing what it was designed to do.

This repo keeps relearning the same shape — a date guard that checked a date was
*present*, a `GUEST_HOME` guard that matched `const` but not `pub const`, a
staleness test satisfied by the wrong call site. **Model non-compliance that
looks random is the signal to go and read the write path.**

### Write cheap, read well

Haiku curated nineteen sessions for **$2.37** — 4.6× cheaper than Sonnet — with
*"the same perfect structural hygiene"*. Answering from that store, Haiku dropped
only ~2.5 points but spent a median of ~640 thinking tokens per answer against
Sonnet's 10.

> Structural curation quality is model-insensitive when the tools enforce it.

Half of that conclusion was drafted before the retraction and did not survive the
rerun. The half that did: **writing memory tolerates a cheap model once guards
keep it honest; reading it is where a strong model earns its cost.**

Not actionable today — omh
[does not choose your model](decisions.md#decisions-deliberately-not-made) — but
it is the argument that would justify a cheap curator later.

### A safety result, with its footnote

Across answering sessions where the agent **held a mutation-capable tool**, the
store diff showed **zero stray writes** — strict mode held in the field, not just
in tests. Directly relevant, because this design hands an unattended agent a
write tool.

The footnote: the two posts disagree on the sample. The benchmark says *forty*
answering sessions, the edit-operations post says *hundreds*. Nothing else
conflicts and the claim is qualitative either way — but this page cites the
**smaller** figure, because a number that grew between two tellings is exactly the
kind this project has learned to distrust in its own writing.

## What this design got wrong

Six reversals, all of them from the benchmark. Each is now settled the other way
in [the spec](memory.md).

| was | is now | why it flipped |
|---|---|---|
| **one idea per note** | one *topic* per page, richly filled | co-location beat atomicity: *"facts that answer questions together must live together"* |
| **`maxTokens: 400` per page** | budgets per *section*; structural rules preferred | their 300-word cap became a fossil and *caused* the newest failure class |
| **"search before writing" as a rule** | derived keys + collision error | the discipline was measured not to hold as a prompt |
| **index injected into `AGENTS.md`** | index in the tool description | it arrives attached to the call instead of competing in a decaying document |
| **ingestion driven by hooks** | the server watches the directory | hooks are not harness-agnostic, and iwe indexes on change |
| **distil `docs/` into notes** | ingest stubs that point at them | curation *"summarized away"* the verbatim detail coding work needs |

The first two are the instructive ones. **"One idea per note" was an aesthetic
inherited from Obsidian**, where a human reads whole pages, and it does not
survive contact with an agent that retrieves by ranking. The token budget was
imported alongside it, praised for *"creating pressure to split"* — which is
precisely the harm the benchmark measured.

Size bounds are not atomicity anyway. A twenty-token note can carry three claims;
an eight-hundred-token note can explain one carefully. **No schema can count
ideas** — and it should not try, because the thing worth optimising is
co-location.

What survived their calibration, and is what the spec adopts:

- **budgets per *section*, not per page.** A hub can sit legally under a page
  budget while its timeline is 92% of it.
- **structural rules over numeric ones.** *Bullets only in a timeline, no prose
  blocks* was the real re-narration detector; the token cap per entry flagged the
  hand-built store's best lines.
- **required sections by name, and a title pattern** keeping dated pages dated.

The general form, which is not specific to notes: **prefer the check that has no
number in it.** A structural rule is either satisfied or not; a threshold has to
be tuned against something you already believe is good, and if you have not done
that tuning you have shipped a guess with the authority of a gate.

## Alternatives considered and not taken

### A curator pass instead of in-task writing

Their notes are not written by the agent doing the work. A **separate,
question-blind curator** replays sessions one at a time and distils them.

omh could do this with no harness capability at all, because **omh owns the
session lifecycle** — a headless pass at session end, no hook, no rule, no
reliance on the working agent noticing anything.

| | in-task writing | curator pass |
|---|---|---|
| depends on | agent behaviour | omh's own lifecycle |
| sees | the moment of surprise | the whole session, in hindsight |
| costs | nothing extra | a model pass per session |
| misses | what the agent didn't notice | what the transcript didn't show |

Not taken for v1, for one blocking reason: **omh has no transcript to replay.**
Reconstructing one means capturing session output — a much larger commitment than
anything in the spec, and one with obvious secret-handling consequences, since a
transcript contains everything the agent saw.

Also worth knowing: their curator is append-only per session, so it cannot merge
an arc spanning sessions, and the hindsight pass meant to fix that failed twice.

### A periodic semantic sweep for contradictions

Tried by them, **failed twice**, kept as a negative result: an editor pass
rewrites wording, search matches words, so every rewrite risks breaking a
question→page match the original phrasing carried.

> Guards during writing beat editing after writing.

What the spec keeps instead is the near-duplicate check in the lint, which
catches pages *about* the same thing — where contradictions actually cluster.

### Building our own server

The space is crowded, and building would add the one part of this that several
projects have already solved. **The server is the commodity; the integration is
the product** — three-layer storage, the index, the provenance envelope, the
`AGENTS.md` boundary, none of which comes free with any candidate.

### Skills

**Skills already do progressive disclosure in Markdown.** A harness loads a
skill's name and description up front and its body only when relevant. That is
the retrieval half of this feature, already implemented, already layered by omh,
already portable.

**They cannot be the implementation, because they only go one way.** Skills are a
*distribution* mechanism — an author writes, the agent reads. Memory is a
*feedback* mechanism. An agent can drop a `SKILL.md` on disk, but there is no
tool for it, no validation, no index to update, and the harness has already
loaded its list: the write is blind and invisible until restart.

That is categorical, and settles it more cleanly than the softer objections —
that skills hold procedures rather than facts, and have no link graph. Both are
conventions. The write direction is not.

What skills are good for here is **evidence that the mechanism works**. The
index-at-session-start design is not speculative: it is what a harness already
does with skills, in production. What is missing from skills is only the write
path — which is exactly what an MCP server supplies.

## Evidence gaps

Carried into [the spec's open questions](memory.md#15-open-questions), listed here
with what would close each:

- **The corpus is wrong for us.** Personal-relationship messages, not a codebase.
  Closed by the [dogfood measurement](memory.md#13-measurement).
- **The one-shot architecture assumes a question.** omh's agent has a task.
- **The numbers are the vendor's**, however honestly reported.
- ~~**Nothing was run in an omh container**~~ — closed by [M0](memory-m0.md),
  which ran iwe in exactly that container and found it does not execute there.
  What stays open is narrower: retrieval quality, `iwes`, and anything above ten
  notes. The bar [`doctor`](../troubleshooting.md) exists to enforce is unchanged.
