# Memory

> **Status: in design.** Nothing here ships. This page is being rewritten as the
> shape of the feature changes — it currently records a reframing, a survey, and
> the questions still open, rather than a settled design.

## The reframing

An earlier version of this page described memory as a **layered fact store**:
personal facts, project facts, team facts, merged with provenance like
`omh config`. That answered the wrong question.

The actual idea is closer to the code graph, pointed at everything that is not
code: **a graph of small linked Markdown notes — Obsidian-shaped — that the
agent can query and grow.** One note, one idea. Concepts, definitions,
decisions, conventions, things learned the hard way, each linked to the others.

Two things it buys:

**A smaller `AGENTS.md`, holding only instructions.** Everything that is a
*fact* moves out; what stays is what the agent must know before it acts.

**Retrieval instead of recitation.** A large `AGENTS.md` is loaded whole into
every session and models attend poorly to the middle of long text. A graph is
queried: three relevant notes instead of four hundred lines, and the cost scales
with the question rather than with the size of what you know.

That second point is the real argument, and it is the same one that justified
the code graph — structural queries instead of re-reading files.

## Instructions are not knowledge

The word "replace" hides a boundary that has to be drawn explicitly.

**Instructions must already be in context**, because the agent cannot look up a
rule it does not know it needs. *"Write the failing test first"* has to arrive
before it starts writing code; nothing will prompt it to go and ask.

**Knowledge can be fetched**, because a question triggers it. *"What does
`carry_in` do?"*, *"why was gitnexus rejected?"*, *"how does staging work?"* —
all reachable on demand.

So `AGENTS.md` does not disappear. It shrinks to imperatives, which is a smaller
and much sharper document than it is today.

## The hard problem: retrieval requires knowing to retrieve

This repo already learned this once. From
[code graph](../code-graph.md#current-used-and-visible): *"An MCP server on its
own is inert — indexed once, never refreshed, never reached for."* Four hooks
were what turned it into something used.

Knowledge is **harder** than code, because the trigger is less obvious. An agent
about to grep knows it is looking something up. An agent that does not know a
convention exists has no reason to ask whether one does.

The pattern that already works here is `graph-orient`: inject the **index**, not
the content. Fifty note titles with a one-line summary each is a few kilobytes —
cheap enough to send at session start, and it tells the agent *what exists* so it
can decide what to fetch. The notes themselves stay out of context until asked
for.

## What already exists

The base set was chosen by surveying candidates and disqualifying most of them
([code graph](../code-graph.md#why-this-one)). Memory gets the same treatment.

This space is **crowded**, which is itself a finding — it argues against
building a server and for choosing one.

| | What it claims |
|---|---|
| [Meshnote](https://github.com/TensorBlock/awesome-mcp-servers/blob/main/docs/knowledge-management--memory.md) | agent-maintained wiki for coding agents: Markdown, YAML frontmatter, `[[wikilinks]]`, BM25 search, backlink graph, per-topic "brains" each seeded with a `schema.md` telling the agent how to maintain it, version history, valid Obsidian vault on disk |
| [iwe](https://github.com/iwe-org/iwe) | Markdown knowledge graph exposed as **both an LSP and an MCP** — the editor and the agent read the same graph, which fits omh's "your editor attached to the same session" model unusually well |
| [Basic Memory](https://mcpmarket.com/server/basic-memory) | persistent knowledge graph from conversations, Markdown on disk, LLM reads and writes |
| [mwe-mcp](https://github.com/Fr4nZ82/mwe-mcp) | Markdown wiki, multi-user with per-fragment ACL, self-organising overnight. **AGPL** |
| [agent-wiki](https://github.com/xinhuagu/agent-wiki) | turns documents, code and project context into portable retrievable memory |
| [linksee-memory](https://github.com/michielinksee/linksee-memory) | local-first cross-agent memory with a token-saving file diff cache |
| [Obsidian Memory MCP](https://lobehub.com/mcp/yunaga224-obsidian-memory-mcp) · [mcp-obsidian](https://github.com/Piotr1215/mcp-obsidian) | the read side against an existing vault |

### Evaluated on the criteria that decided codegraph

Not on features — features were never what disqualified a candidate there.

| | licence | runtime | storage | active | writes? |
|---|---|---|---|---|---|
| **[iwe](https://github.com/iwe-org/iwe)** | **Apache-2.0** | **none — Rust, single binary** | plain `.md`, **no database** | 308 commits, current | yes: `new`, `extract`, `inline`, `rename`, `delete` |
| [Basic Memory](https://github.com/basicmachines-co/basic-memory) | **AGPL-3.0** | **Python 3.12+** | `.md` + **SQLite** | 1,649 commits, 3.6k stars | yes |
| [mwe-mcp](https://github.com/Fr4nZ82/mwe-mcp) | **AGPL-3.0** | not checked | not checked | not checked | yes |
| others | not checked | | | | |

**iwe is the provisional pick**, and it is not close on the criteria this
project actually uses:

- **Apache-2.0.** No repeat of the gitnexus problem, where a noncommercial
  default would have put every user writing code at work in violation.
- **Rust, single static binary, no database.** Exactly what won it for
  codegraph — *"a service to run is a decision `omh init` promised to remove"*.
  Basic Memory needs a Python runtime in the base image and carries SQLite.
- **It is also an LSP.** The editor [attached to the session](../editors.md) and
  the agent read the same graph. Nothing else on the list does this, and it fits
  omh's model unusually well.

Basic Memory is the more established project by a wide margin — 3.6k stars
against a few hundred commits — and that is a real argument. But AGPL as a
*distribution default* is the shape of decision omh has already refused once,
and a Python runtime is a cost the base image does not currently pay.

**This is reading, not running.** Every claim above comes from documentation,
which is precisely the class of claim this project treats as unverified: the
graph server's own docs advertised a `CBM_VARIANT=ui` switch its published
installer ignored. Before adopting, iwe has to be run in a container,
unattended, and shown to write notes an agent can then retrieve.

## The server is the commodity; the integration is the product

Worth stating before choosing, because it changes what "build it ourselves"
would even mean. Whichever server wins, **omh still has to build**:

- **three-layer note storage** — personal notes that follow you, committed notes
  the team shares, gitignored notes that are yours alone
- **index injection at session start**, so the agent knows what exists without
  loading it
- **the hooks that make retrieval happen** — the lesson from the code graph is
  that a server nothing reaches for is inert
- **the `AGENTS.md` boundary** — deciding what is an imperative and what is a
  fact, and moving the second out

Provenance was on this list until iwe's
[document schemas](#document-schemas-make-the-invariant-enforceable) turned out
to enforce it, which is one fewer thing to build and a better mechanism than
omh would have written.

What remains is still larger than the server, and none of it comes free with any
candidate. Building the server too would add the one part of this that several
projects have already solved.

## The poisoning problem, and why the format helps

The previous version of this page said it plainly: *a memory store that
accumulates confident wrong facts is worse than no memory store.*

That is not hypothetical. In one session this project shipped seven fabricated
measurement dates, a byte count that was wrong on arrival, and a deprecated
config key — each stated with complete confidence, none caught by a test. An
agent with unattended write access does that **into a store that is then loaded
into every future session**.

The Obsidian shape helps more than a monolith would:

- one idea per file means a wrong fact is **one file to delete**, not a
  paragraph buried in a document — where the convention is actually followed;
  nothing verifies it
- `git log` shows when a note appeared and what it replaced
- links make an unsupported claim visible: a note nothing links to, and which
  links to nothing, is a fact with no context

What it does not solve is a confidently wrong note being *retrieved and
believed*. That needs the same answer the base set got: **every fact carries
where it came from and when**, so it can be judged rather than trusted. A note
without provenance reintroduces exactly the problem [`omh why`](trust.md) exists
to remove.

## Decided

**The agent writes unattended.** It records what it learns without asking. This
is the feature — a memory you have to approve is a notebook, and nobody keeps
one. It is also the risk, and the risk is not hypothetical: in a single session
this project produced seven fabricated dates, a byte count wrong on arrival and
a deprecated config key, each stated with complete confidence. Unattended
writing turns that into notes loaded into every future session.

So the mitigations are not optional extras, they are the price of this choice:

- **one idea per file**, so a wrong fact is one deletion — a convention, not a
  guarantee: nothing can verify a note holds a single claim, and a note holding
  three with one wrong cannot be cleanly removed
- **provenance on every note** — what produced it, when, from what. A fact that
  cannot say where it came from cannot be judged, and the whole
  [trust](trust.md) argument is that omh does not state things it cannot support
- **`git log` as the audit trail**, which comes free from notes being files
- **expiry**, hardest and least designed: a note true in June and false in
  September looks identical to one still true

**The graph ingests what the repo already documents.** `docs/`, ADRs and
existing notes become part of it, not just what the agent accumulates. Two
reasons: it is useful on day one rather than after weeks of use — which is when
somebody decides whether omh is worth keeping — and the answers are already
written. *"Why was gitnexus rejected?"* has lived in `code-graph.md` since
August; the agent simply has no way to find it.

This also closes a loop: `detect::seeds()` already derives the README tagline
and stack facts and currently throws them away in a `println!`. They become the
graph's first notes.

**Adopt if something fits; build only with a documented reason.** The next step
is the pass that decided codegraph — licence, external dependencies,
maintenance, and whether it runs unattended in a container. Features were never
what disqualified a candidate there, and are unlikely to be what disqualifies
one here.

## Layering

The three layers map straight across:

```
~/.omh/notes/              personal — facts about you, every project
<repo>/.omh/notes/         shared   — COMMITTED, your team sees these
<repo>/.omh/local/notes/   local    — GITIGNORED, yours alone
```

But **notes do not merge the way the profile does**, and the difference is not
cosmetic. `policy.toml` merges key by key with later layers winning, because a
setting has one value. A note is a *claim*, and two claims about one topic are
two facts, not one overriding the other.

So **the layer is part of a note's identity**, not a precedence order:
`team/deploy` and `local/deploy` are different notes and both are retrievable.
Shadowing would silently hide a teammate's note behind yours — the reader would
never learn the other existed.

### The agent writes to `local`, and only there

Same rule as `omh config set` defaulting to the gitignored layer, for a stronger
reason. An unattended writer that can reach the committed layer pushes wrong
facts to teammates **through git**, where they arrive with the authority of a
reviewed change and nobody remembers approving them.

Promotion is a human act — `omh memory promote <note> --to shared|personal`. That
puts review at exactly the boundary where it matters and nowhere else: the
proposed-not-written model, applied only where the blast radius leaves you.

Personal is promotion-only for the same reason, inverted. A wrong personal fact
poisons every repo you own, and a client's detail that should never have been
global is worse than a global fact that should have been local.

### Links may cross layers, in one direction

A committed note may link **only to committed notes**. Anything else dangles for
a teammate who does not have your local layer or your home directory — a
reference to a note they can never open.

Local and personal notes may link anywhere, because only you can follow them.

That is a testable invariant, and belongs with the others in
[contributing](../contributing.md): *a committed note links only to committed
notes.*

## Two graphs, not one

Not because the schemas differ — though they do, one being symbols and calls and
the other concepts — but because the **lifecycles are incompatible**.

The code graph indexes *this session's worktree*, is refreshed after every turn,
and is dropped with the session: `omh s rm` deletes it along with the code it
described. Notes must **survive session removal**; that is what makes them
memory. They are scoped to a repo and to you, never to a session.

Merging them would force one of two bad outcomes: notes that die when you remove
a session, or a code graph that outlives the code it describes.

The cost is real and worth stating: two MCP servers means two tool sets in every
session's context. That argues for keeping the notes server's surface small —
search, retrieve, create, link — rather than exposing everything iwe can do.

One consequence is useful rather than costly: **the code graph can check notes
about code.** A note naming a symbol that no longer exists is detectably stale.
See below.

## The `AGENTS.md` boundary

The test is not "is this a fact" but **"does the agent need it before it acts,
with nothing to prompt a lookup?"**

- *"Write the failing test first"* — **imperative**. Nothing prompts an agent to
  ask whether it should test first; by the time a question could arise, the code
  is written.
- *"gitnexus was rejected for its licence"* — **fact**. A question reaches it.
- *"test with `cargo test`"* — **fact, but carried anyway**. Needed most sessions,
  and fetching costs a round trip every time. Frequency beats purity.

### What this actually changes, which is not what it sounds like

Today's shared `AGENTS.md` is **52 lines in four sections — and all four are
imperatives or routing instructions**: prefer the graph for structural
questions, the stack's commands, TDD always, honesty about coverage.

There is almost nothing to move out.

So the win is **not shrinking the file**. It is preventing the growth that
happens when knowledge has nowhere else to go. Without a graph, every fact the
agent learns gets appended here until it is four hundred lines and the middle is
unread — which is precisely the failure this feature exists to avoid. The graph
is where that growth goes instead.

Stated as a rule: **`AGENTS.md` is a closed set of instructions, not an open
ledger.** If it is growing, something belongs in the graph.

## Expiry

Start with the honest part: **there is no general solution.** A team convention
that quietly changed cannot be detected by any mechanism available here. What
can be done is to separate the staleness that *is* checkable from the staleness
that is not, and to never present the second as certain.

**Checkable — derived from a file.** A note records what it was derived from and
that source's content hash. A note derived from `Cargo.toml` is suspect the
moment `Cargo.toml` changes. Free, deterministic, and loud.

**Checkable — derived from code.** A note naming a symbol the code graph no
longer contains is stale. This is the payoff of running two graphs: one can
validate the other's claims about the thing it indexes.

**Not checkable — derived from experience.** *"The staging deploy needs the
VPN"* has no source to watch. It carries a date, and that is all.

### Document schemas make the invariant enforceable

iwe has [document schemas](https://github.com/iwe-org/iwe/blob/master/crates/iwe/docs/schema.md):
a JSON-Schema-aligned declaration of the shape a note must have — which
frontmatter fields it carries, which sections in what order, and how large each
part may grow. `iwe schema validate` checks them, so in its own words *"a
store's conventions become machine-checked policy in the loop write → validate →
fix."*

That converts the central invariant from a hope into a check:

```yaml
# .iwe/schemas/note.yaml
frontmatter:
  type: object
  required: [source, recorded]
  properties:
    source:   { type: string }                            # what produced this
    recorded: { type: string, pattern: "^\\d{4}-\\d{2}-\\d{2}$" }
maxTokens: 400
```

A note without provenance **fails validation**. This is the same move the base
set already made: `every_base_set_entry_states_its_case` turned *"the shortlist
must be earned"* from a sentence in a document into a test that fails the build.
The lesson there was that an unenforced convention is not a convention, and it
cost seven fabricated dates to learn.

`maxTokens` bounds **size**, which is not the same as one idea per note and
should not be confused with it. A twenty-token note can carry three claims; an
eight-hundred-token note can explain one carefully. **No schema can count
ideas.**

What the budget does buy is real but narrower:

- **retrieval stays cheap** — a note has a known worst case
- **notes cannot drift back into being documents**, which is the failure this
  feature exists to escape
- **it creates pressure to split**, because staying under budget means
  extracting — and `extract` is an operation iwe already has

Structure gets closer to atomicity than size does. A schema can require exactly
one top-level section with `maxContains: 1` and `additionalSections: false`,
which pushes a note toward one topic. Still not a guarantee — one section holds
as many claims as you put in it — but nearer than a token count.

**One idea per note stays a convention.** It has mechanical pressure behind it
now, and no enforcement.

Schemas bind by glob and **compose**, so layers can differ in strictness:

```toml
[schemas.note]        # everything the agent writes
match = "**"

[schemas.shared]      # additionally, for the committed layer
match = ".omh/notes/**"
```

A committed note can be required to carry more than a local one — which is the
layering decision, enforced rather than trusted.

Most usefully: **write → validate → fix is a feedback loop around unattended
writing**, which is what the design was missing. The agent writes a note, the
schema rejects it for having no source, the agent supplies one. Nothing here
depends on the agent choosing to be careful.

### The invariant that decides whether this is trustworthy

**A note is never retrieved without its date and its source.**

This is the same discipline the base set already runs on: `measured 2026-08-06`
appears on every cost because a number printed bare reads as a fact about right
now. A six-month-old note is not necessarily wrong — but presenting it as
equally current as one written today is the fabricated-authority pattern, and
this project has already shipped that failure once, with seven invented dates
that a test walked past because it only checked a date was *present*.

If retrieval surfaces age and origin, the agent can weigh a note. If it does
not, unattended writing becomes a machine for laundering guesses into facts.

### Making deletion cheap

Detection only matters if acting on it is trivial: `omh memory stale` lists
notes whose source moved, whose symbol vanished, or that predate the current
base-set version, and removing one is deleting one file. One idea per note is
what buys that — a wrong paragraph inside a long document has no such affordance.

## Hooks are not harness-agnostic, and this feature cannot rest on them

The requirement is that the agent fills the graph **whatever harness is
running**. Checking what the shipped adapters actually declare:

| | rules | mcp | hooks |
|---|---|---|---|
| claude | ✅ | ✅ | ✅ |
| opencode | ✅ | ✅ | **✗** |

Only **rules and MCP are universal**. An earlier version of this design put
ingestion, index injection *and* write-prompting on hooks — which would have
worked on Claude Code and degraded, on opencode, to precisely the inert server
the [code graph](../code-graph.md#current-used-and-visible) lesson warns about:
indexed once, never reached for, and nobody notices.

So each mechanism has to be placed on the lowest layer that can carry it.

### Ingestion belongs in the server, not in a hook

iwe watches a directory. If it reconciles on start and on query, **no hook is
needed at all** — a `git pull`, a branch switch, a human editing in their editor
and the agent's own writes are all just files changing under a server that is
already watching.

Hooks were the wrong layer for this. The code graph needs one only because
`codebase-memory-mcp` indexes on command rather than on change.

### The index rides in the staged `AGENTS.md`

The agent has to know *what exists* before it can query anything, and
`SessionStart` was how the design delivered that. Hookless harnesses need
another carrier, and there already is one: **omh regenerates the staged rules
file on every launch.** Putting the note index there makes it fresh at session
start, in every harness, with no hook.

`AGENTS.md` already carries exactly this kind of content — its `## Code graph`
section is routing, not instruction: *here is a tool, here is when to prefer it*.
A `## What is in your notes` section is the same shape.

It does mean the index is unconditionally in context rather than fetched. That
is acceptable because the index is bounded by design — titles and one-liners —
while the thing it replaces is unbounded. A generated index that grows with the
graph is still a fixed cost per note, and a far smaller one than the note.

### Hooks stay, as an enhancement

Where a harness has them, they sharpen the timing: a `Stop` hook can prompt for
what was learned at the end of a turn rather than relying on the agent to
remember mid-task. That is better on Claude Code and absent on opencode, which
is the correct shape for an [optional capability](adapters.md) — the feature
works everywhere and is *better* where the harness allows.

What must never depend on a hook is whether memory works at all.

## Making the agent actually write

Retrieval can be engineered. Writing is a **behaviour**, and behaviour is not
guaranteed by anything omh can build. This is the weakest link in the feature
and worth saying so before designing around it.

Two levers work in every harness:

**The rule.** *"When something did not work the way you expected, record why."*
This is an imperative — nothing prompts an agent to wonder whether it has
learned something — so by the [boundary rule](#the-agentsmd-boundary) it belongs
in `AGENTS.md`.

**The tool description.** Every MCP tool carries a description the model reads,
in every harness, with no hook involved. A tool called `remember` whose
description says *when* to reach for it is a prompt that arrives at exactly the
moment of decision. This is the most underused lever available and the only one
that is both universal and contextual.

### Record on surprise, not on schedule

The obvious trigger — write a note at the end of every turn — produces noise.
Most turns teach nothing, and an agent asked to summarise one anyway will
produce *"fixed a typo in the parser"*, which is worth less than nothing once it
is in the graph forever.

The notes actually worth having all share a shape. Every one of the valuable
facts in this repo came from an expectation being violated:

- mounting a token file returns `EBUSY`, so the login never persists
- the published installer ignores the `CBM_VARIANT` it documents
- `--aspects` with a comma-separated list returns empty, silently
- `info/exclude` is read from the *common* git dir, not the per-worktree one

None of those came from routine work. Each came from something not behaving as
assumed — which is also precisely when a fact is worth recording, because the
assumption it corrects is one the next agent will make too.

**Surprise is a better trigger than time**, and it is a trigger the model can
recognise without a hook: it knows when it expected one thing and got another.

### The floor, when the agent writes nothing

Because the graph [ingests what the repo already documents](#decided), it is
useful on day one whether or not the agent ever writes a note. That matters:
compliance is not guaranteed, so the feature must not be worthless without it.

Agent writing is the *growth* path. Ingestion is the *floor*.

### This is observable, not testable

Whether an agent reliably records what it learns cannot be asserted in a unit
test — it is a claim about a model's behaviour, in the same class as *"the
harness reads this path"*, which is why [`omh doctor`](../troubleshooting.md)
exists. The honest check is to run it for a week and look at whether the graph
grew, and whether what it grew is worth keeping.

Until that has been done, the correct status for this feature is **unverified**,
however good the design reads.

## Identity is solved by the tool, not by an id

iwe provides **LSP refactoring semantics over notes** — go to definition, find
references, rename symbol — plus link-title synchronisation that keeps link text
in step with the target's title. Renaming a note updates what points at it.

So the risk this question was really about — *the agent tidies up and every link
breaks* — is handled upstream, and by a better mechanism than a frontmatter
`id`: referential integrity maintained by the thing that owns the graph, rather
than a convention every writer has to honour.

Two caveats worth carrying:

- **This holds for renames made through iwe.** A `mv`, or a human renaming a
  file in an editor with no LSP running, updates nothing. That makes *"rename
  notes through the tool, never with `mv`"* an **imperative** — it is a rule the
  agent needs before it acts, so by the boundary rule above it belongs in
  `AGENTS.md` rather than in the graph.
- Frontmatter is supported and, better, **validated** — see
  [document schemas](#document-schemas-make-the-invariant-enforceable). An
  earlier version of this page recorded it as unconfirmed; the schema
  documentation settles it.

## Conflict: never let retrieval pick a winner

iwe does not resolve contradictions, and no markdown indexer will — detecting
that two notes disagree is a semantic judgement, not an indexing one.

But the framing was wrong, and inverting it dissolves most of the problem.
**Two contradicting notes are only dangerous when retrieval returns one and
hides the other.** If a query surfaces both, the agent holds both in context,
with their layers and dates, and reconciles with full information — which is
exactly what an LLM is good at and an indexer is not.

That is already the decided behaviour, for an unrelated reason: layer is part of
a note's identity, so `team/deploy` and `local/deploy` both retrieve rather than
one shadowing the other. It was chosen so a teammate's note could not be hidden
behind yours. It turns out to be the conflict answer too.

### When the agent must still choose

**Layer outranks recency.** A promoted note has been through human review; a
local note was written unattended by something that, in a single session,
produced seven fabricated dates with complete confidence. Recency is not
correctness, and "newest wins" would let a fresh hallucination overwrite a
reviewed fact.

**Within one layer, recency and context decide** — the newer note, read against
what the task actually needs. This is where dates earn their place.

**Ask only when it matters.** Two *promoted* notes disagreeing, or a conflict
that blocks the task. Not on every disagreement: `omh init` asks nothing, and a
memory that interrogates you would be the same failure in a new place.

### Prevention beats resolution

**Search before writing a note.** If one exists on the topic, update it rather
than adding a contradicting sibling. That removes most conflicts at the source,
and it is another `AGENTS.md` imperative — nothing prompts an agent to wonder
whether it is about to duplicate something.

### What this still does not catch

Two notes on topics that never co-occur in a query can contradict indefinitely.
Nothing here finds them, and a periodic semantic sweep over the whole graph
would cost an LLM pass per run. Recorded rather than solved.

## A cost worth naming

iwe exposes **13 MCP tools**. Every one of them carries a schema into every
session's context, alongside the code graph's own set. That is a real per-session
cost against a feature whose entire argument is context efficiency.

Worth measuring before adopting, and worth checking whether the surface can be
narrowed to the four operations this design actually needs: find, retrieve,
create, link.

## Skills are the precedent, not the alternative

**Skills already do progressive disclosure in Markdown.** A harness loads a
skill's name and description up front and its body only when relevant. That is
the retrieval half of this feature, already implemented, already layered by omh,
already portable across every harness that supports skills.

**They cannot be the implementation, because they only go one way.** Skills are
a *distribution* mechanism — an author writes, the agent reads. Memory is a
*feedback* mechanism — the agent writes what it learns and reads it back. An
agent can drop a `SKILL.md` on disk, but there is no tool for it, no validation,
no index to update, and the harness has already loaded its list: the write is
blind and invisible until restart.

That is categorical, and it settles the question more cleanly than the softer
objections do — that skills hold procedures rather than facts, and have no link
graph. Both are conventions. The write direction is not.

What skills are good for here is **evidence that the mechanism works**. The
index-at-session-start design above is not speculative: it is what a harness
already does with skills, at scale, in production. What is missing from skills
is only the write path — which is exactly what an MCP server supplies.
