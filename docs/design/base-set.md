# The base set

This is the product. Everything else in omh is a place to put it.

It lives in a versioned TOML file — `~/.omh/base/2026.08.toml`, shipped with the
binary and installed by `init` the same way [adapters](adapters.md) and editors
are. **`init` seeds from it and [`omh why`](../commands.md#omh-why-thing)
explains from it**, so the two cannot disagree about what is installed or why.

## An entry

```toml
version = "2026.08"

[[entry]]
name    = "codegraph"
kind    = "mcp"                 # mcp | hook
since   = "2026.06"
because = "structural queries instead of re-grepping the repo every task"
remove  = "omh config mcp rm codegraph"
command = "codebase-memory-mcp" # what init seeds; also the baseline `why` compares against

  [[entry.measured]]
  what  = "to index this repo, cold"
  value = "0.46s"
  how   = "index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox"
  on    = "2026-08-06"

  [[entry.instead_of]]
  name = "gitnexus"
  why  = "PolyForm-Noncommercial licence"
```

`deny_unknown_fields`, matching `Adapter`, and for the same reason: a misspelled
key must fail loudly rather than parse into a base set that quietly contains
less than you think.

## The curation standard is a test, not a convention

`docs/design/distribution.md` says every entry states what it costs, what it
buys, what was considered instead, and how to remove it — and that anything
unable to fill in all four is taste pretending to be curation.

That was aspiration in a document nothing enforced. It is now
`every_base_set_entry_states_its_case`, so an entry cannot be added without its
reasoning. The cheapest moment to demand a justification is before it ships, and
the only moment anyone reliably does is when something turns red.

The test rejects a blank `because` or `remove`, an empty `instead_of` (*"an
entry with no alternatives was not chosen, it was defaulted to"*), an empty
`measured`, and — added after the failure below — a blank, unparseable, or
impossible date.

### Cost is measured; benefit is argued

The two halves are different kinds of claim and the manifest keeps them apart.

`because` is a judgement. `measured` is a recording, and carries the date it was
taken and the method — because a number printed bare reads as a fact about right
now, which is exactly the fabricated authority this file exists to avoid.

**This is not theoretical.** Every `on` in the first version of this manifest
read `2026-08-04`, one day before the repository's first commit. Three of those
entries claimed to measure this repo; one measured a file that did not exist
yet. They were typed, not taken. The guard walking every measurement checked a
date was *present* and never that it was true.

Two values were wrong independently: `~40 B` for the grep nudge was really 243
(the tilde is the tell — it made the number unfalsifiable), and a byte count
naming a specific file was stale on the commit that introduced it.

So where a cost **can** be computed, it is: `grep_nudge()` builds the string
from the same literals the hook ships, and a test recomputes it and fails if the
manifest disagrees. That is the shape every in-process cost should take. The
rest need a container, which is [`doctor`](../troubleshooting.md) territory —
the same boundary that makes adapter paths unverifiable in-process.

## Rejections are artifacts

```toml
[[rejected]]
name       = "gitnexus"
considered = "2026.06"
because    = "PolyForm-Noncommercial. Every omh user writing code at work would be in violation of a dependency they never chose."
```

`omh why gitnexus` answers from this. Without it the same candidate gets
re-litigated every time somebody rediscovers it — and the reasoning that
disqualified it, which is often a licence or an operational dependency rather
than a quality judgement, has to be reconstructed from memory.

## Versioning

The base set **expires**. A distribution's real work is re-choosing as the
catalogue churns, so the file is versioned and older ones are kept rather than
deleted: re-cutting it marks every carried-over measurement stale, which is the
prompt to re-take or re-affirm each one.

Selection is by **parsed version**, not filename sort. Sorting filenames was
three silent wrong answers at once:

- any stray `.toml` sorting after the real manifest became the base set
- `2027.2` beat `2027.10`, so zero-padding was load-bearing and unenforced
- nothing compared a file's declared `version` to anything

The first of those was the worst: a `notes.toml` in `~/.omh/base` made `init`
seed `{"mcpServers": {}}` and report every decision cleanly, while the hooks —
which come from code, not the manifest — still pointed the agent at a server
that was not installed. A manifest declaring no entries is now an error.

Staleness is measured against the **manifest version**, not the entry's `since`.
Against `since` it could never fire: a measurement is taken at or after the
entry was added and `since` never moves, so no shipped number was flaggable in
2027 or 2035. Against the version it fires whenever the base set is re-cut,
which is exactly when carried-over numbers should be re-taken or re-affirmed.

## Seeded, not live

Worth understanding before changing anything here: `init` **copies** the base
set into your profile — `mcp.json` and `hooks/*.json` in the shared layer — and
then never revisits it. It uses `write_if_absent`, so your edits survive.

Two consequences follow, and both caused bugs:

- An omh-seeded entry and one you added are **byte-identical in the same file**.
  Nothing marks which is which, which is why `omh why` recovers authorship by
  comparing against the manifest rather than by reading a marker.
- The manifest is refreshed on upgrade while your profile is not, so the two
  drift by design. `omh why` therefore reports *"not what omh ships now"* and
  explicitly declines to say who caused the difference — it cannot know, and an
  earlier version that guessed told every user they had edited files they never
  opened.

## Data versus code

MCP servers are pure data and live entirely in the manifest. Hook **commands**
stay in `src/base.rs`: they are intricate shell that interpolates `GRAPH_BIN`
and `$OMH_GRAPH_PROJECT`, and flattening them into TOML would break that
compile-time coupling.

So the manifest carries hook *curation metadata* while the code carries the
hook itself — two sources describing one base set, which can drift. A test
asserts the name sets match exactly **in both directions**, because the drift is
silent in the worst way: `omh why` confidently explaining an entry that is no
longer installed, or an entry shipping with no explanation at all.

## Adding an entry

1. Add it to the newest manifest, filling in all four fields.
2. Run `cargo test`. If the curation test fails, the entry has not earned its
   place yet — that is the test working, not an obstacle.
3. Measure the cost rather than estimating it, and record how. If it can be
   computed in-process, compute it in a test instead of typing it.
4. If it is a hook, add the command to `base::hooks()` too; the drift guard will
   tell you if you forget.

If an eighth entry needs a paragraph to justify itself, it belongs in a profile
rather than the base set.
