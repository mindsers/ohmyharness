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
  the team shares, gitignored notes that are yours alone, merged the way the
  [profile](../configuration.md#the-three-layers) already is
- **index injection at session start**, so the agent knows what exists without
  loading it
- **the hooks that make retrieval happen** — the lesson from the code graph is
  that a server nothing reaches for is inert
- **provenance on every fact**, so a note can be judged rather than trusted
- **the `AGENTS.md` boundary** — deciding what is an imperative and what is a
  fact, and moving the second out

That list is larger than the server, and none of it comes free with any
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
  paragraph buried in a document
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

- **one idea per file**, so a wrong fact is one deletion
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

## Still open

1. **Layering.** Does the [three-layer profile](../configuration.md#the-three-layers)
   apply — personal notes, committed team notes, gitignored local notes — and can
   a link cross layers? Unattended writing makes this sharper: a wrong note in a
   committed layer reaches teammates.
2. **One graph or two.** codegraph is code-shaped (symbols, calls, imports).
   Notes are concept-shaped. Separate stores, or one?
3. **Boundary.** Which of today's `AGENTS.md` is an imperative that stays, and
   which is a fact that moves?
4. **Expiry.** How does a note stop being true, and what notices?

## An alternative worth considering

**Skills are already progressive-disclosure Markdown.** A harness loads a
skill's name and description up front and its body only when relevant — the
exact mechanism this feature wants, already implemented, already layered by
omh, already portable across every harness that supports skills.

Notes-as-skills would need no MCP server at all. Against it: skills are
procedures by convention rather than facts, there is no link graph, and the
agent cannot *write* one mid-session. But it sets the bar a new subsystem has to
clear.
