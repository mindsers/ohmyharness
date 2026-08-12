# The base set

This is the product. Everything else in omh is a place to put it.

It lives in a versioned TOML file — `~/.omh/base/2026.08.toml`, shipped with the
binary and installed by `init` the same way [adapters](adapters.md) and editors
are. **Every session is built from it and [`omh why`](../commands.md#omh-why-thing)
explains from it**, so the two cannot disagree about what is installed or why.

## An entry

```toml
version = "2026.08"

[[entry]]
name    = "codegraph"
kind    = "mcp"                 # mcp | hook | rules
feature = "codegraph"           # what it is part of; `[omh]` switches features, not entries
since   = "2026.06"
because = "structural queries instead of re-grepping the repo every task"
remove  = "omh config mcp rm codegraph — the feature, server and hooks together"
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

## A feature, not a flat list

`feature` is what an entry belongs to. `codegraph` is one thing: the MCP
server, the four hooks that make it *used* rather than merely installed, and
the section of the rules that tells the agent it exists. Half of that is not a
smaller version of it — a graph with no `graph-refresh` keeps answering
confidently about code the session has since rewritten.

So the feature is the unit that matters:

| | |
|---|---|
| **removal** | `omh config mcp rm codegraph` takes the server and its four hooks together. Before this it left them behind, nudging the agent toward something that was gone |
| **disabling** | `[omh] codegraph = false` in `<repo>/.omh/settings.toml`. Per repo, and nothing is uninstalled — your `mcp.json` is untouched and the next repo gets it back |
| **explaining** | `omh why graph-first` answers "part of codegraph"; `omh why codegraph` lists what it brought |

`[omh]` takes feature names only. `graph-first = false` is refused, naming the
feature it belongs to — which is also how the grouping is discoverable without
reading this file. That keeps "graph on, refresher off" unrepresentable rather
than warned about, and an earlier draft that allowed it needed a warning at
every launch, a manifest field declaring whether disabling cost correctness,
and a rule about which kind may switch off silently. Taking the granularity
away took all three with it.

The grouping lived as a comment header in the manifest — the one claim in that
file no test could check, while every other claim an entry makes is a field
with a guard demanding it be filled. `every_base_set_entry_names_its_feature`
is that guard, and it is load-bearing rather than documentary: an entry
belonging to nothing is an entry nobody can switch off.

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

## Seeded, or generated

`mcp.json` is **seeded**: `init` copies the servers into your profile with
`write_if_absent` and never revisits, so your edits survive. Two consequences
follow, and both caused bugs:

- An omh-seeded server and one you added are **byte-identical in the same
  file**. Nothing marks which is which, which is why `omh why` recovers
  authorship by comparing against the manifest rather than by reading a marker.
- The manifest is refreshed on upgrade while your profile is not, so the two
  drift by design. `omh why` reports *"not what omh ships now"* and declines to
  say who caused the difference — it cannot know, and an earlier version that
  guessed told every user they had edited files they never opened.

Hooks and rules sections are **generated**. They are written into the session
at launch, from the manifest, and exist as a file nowhere. That is the only
arrangement in which omh can ship a fix to its own machinery: seeding never
revisits, so `git-unavailable` — rewritten once, after the old pattern was
found to miss the multi-line scripts agents most often emit — would have gone
on running broken in every repo initialised before the fix.

A manifest **hook** name is omh's, on or off. A file in a layer answering to one is
never read: with the feature on the generated hook would win anyway, and with
it off there would be nothing to override the file with, so switching a feature
off would leave the disabled thing running.

`omh why` says so rather than treating the file as yours — a repo initialised
before generation still has the five sitting in `.omh/profile/hooks/`, and
editing one changes nothing.

## Data versus code

MCP servers are pure data and live entirely in the manifest. Hook **commands**
stay in `src/base.rs`: they are intricate shell that interpolates `GRAPH_BIN`
and `$OMH_GRAPH_PROJECT`, and flattening them into TOML would break that
compile-time coupling. Rules **bodies** stay there for the same reason —
`memory-rules` interpolates the guest note path and `git-rules` reads the
string the `git-unavailable` hook also emits, and two copies of a safety notice
drift, with the one that drifts never being the one you are reading.

So the manifest carries *curation metadata* while the code carries the thing
itself — two sources describing one base set, which can drift. A test
asserts the name sets match exactly **in both directions**, because the drift is
silent in the worst way: `omh why` confidently explaining an entry that is no
longer installed, or an entry shipping with no explanation at all.

## Adding an entry

1. Add it to the newest manifest, filling in all four fields.
2. Run `cargo test`. If the curation test fails, the entry has not earned its
   place yet — that is the test working, not an obstacle.
3. Measure the cost rather than estimating it, and record how. If it can be
   computed in-process, compute it in a test instead of typing it.
4. If it is a hook or a rules section, add it to `base::hooks()` or
   `base::sections()` too; the drift guard will tell you if you forget. A
   section's size is computed from the string it ships, so the manifest has to
   agree with the prose rather than with your estimate of it.
5. Name the feature it serves. If none of the existing three fits, the entry is
   introducing a feature, and that is a bigger claim than adding a package.

If an eighth entry needs a paragraph to justify itself, it belongs in a profile
rather than the base set.
