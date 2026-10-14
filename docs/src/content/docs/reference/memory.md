---
title: Memory
description: The note store — where it lives, what a note file holds, how its key is derived, and what recall matches.
---

The note store on disk: where it is, what a note file holds, how its key is
derived, what `omh memory lint` checks, and what recall matches.

You usually reach a note through [`omh memory`](/reference/commands/#omh-memory-)
and the two tools a session gets — [Commands](/reference/commands/#omh-memory-)
covers each. This page is what they read and write, for when you open the store
yourself.

| I want to… | Read |
|---|---|
| edit a note by hand | [A note file](#a-note-file) and [Three types](#three-types) |
| read what `omh memory lint` is refusing | [What lint refuses, and what it only reports](#what-lint-refuses-and-what-it-only-reports) |
| change how keys are derived | [Keys](#keys) |
| find out why recall did or did not return a note | [What recall matches](#what-recall-matches) |

## Where it is

One store per repository, on your machine, never committed.

```
~/.omh/memory/<repo>/                host — one store per repo, never committed
/omh/memory                          the same directory, inside a sandbox
```

`<repo>` is the checkout's directory name and a digest of its path, so two
checkouts called `api` are two stores. A key is a path under it:
`surprise/mounting-a-credential-file-returns-ebusy` is the file
`surprise/mounting-a-credential-file-returns-ebusy.md`.

## A note file

A note is a Markdown file: frontmatter, a title, and sections.

| field | required | |
|---|---|---|
| `key` | yes | the note's identity, and its path under the store without `.md`. `omh memory lint` refuses a note whose `key` and filename disagree, so rename one by changing both |
| `type` | yes | `surprise`, `topic` or `stub` — see [Three types](#three-types). Anything else is refused by name |
| `source` | yes | who observed it. A session's tool writes `session s03, claude`, from the session it was started for and the harness that connected; the command line writes `cli`, or what `--source` names; a stub's is the document it points at |
| `recorded` | yes | the day it happened, as `YYYY-MM-DD` — a date that exists, so `2026-13-45` is refused |
| `invalidated_by` | no | what makes the note stale, from [the closed set](/reference/commands/#omh-memory-stale) `omh memory stale` can evaluate |

This is what `omh memory remember` writes, byte for byte:

```markdown
---
key: surprise/mounting-a-credential-file-returns-ebusy
type: surprise
source: cli
recorded: 2026-10-11
---

# Mounting a credential file returns EBUSY.

## Expected
A bind mount of the token file would persist the login.

## Observed
Mounting a credential file returns EBUSY.

## Evidence
EBUSY from the mount syscall

## Answers

- why does mounting the token file fail?

## Related

- [[credentials-are-a-named-volume]]
```

**The frontmatter is `name: value` lines between two `---`**, and nothing more
elaborate. omh reads it with a parser of its own, so that it never accepts a
file it could not write back.

**A required field has to say something.** One that is present and empty is
refused like one that is missing.

**A field omh does not know is ignored** rather than refused — a note is your
document as much as the agent's.

**A link is `[[a-key]]`.** It takes a key, never a title, because a key can be
worked out before the note it names exists.

## Three types

A note's `type` decides which sections it must have. A section is a `## Name`
heading; the word appearing somewhere in the body does not count.

| type | what it is | sections it must have | sections that hold a list and nothing else |
|---|---|---|---|
| `surprise` | something that went differently from what was expected | `Expected`, `Observed`, `Evidence`, `Answers` | `Answers`, `Related` |
| `topic` | one subject, filled in as far as it goes | none | `Related` |
| `stub` | a pointer at a document the repo already has | `Answers` | `Answers` |

**`Answers` is the questions the note answers**, in the words somebody would
later ask them in. It is what makes a note findable, and only its writer knows
it — which is why `remember` refuses a note without one:

```console
$ omh memory remember --expected … --observed … --evidence …
omh: `answers` is empty; a note nobody can find is a note nobody wrote
```

<details>
<summary>Why some sections hold a list and nothing else</summary>

*A list and nothing else* is a rule about shape standing where a length limit
would be. A limit needs a number, and a number needs a store to calibrate it
against; bullets-only needs neither.

</details>

## What lint refuses, and what it only reports

`omh memory lint` names a rule beside each count. Six describe one note's shape
and refuse it; three describe the store and warn.

| rule | | |
|---|---|---|
| `Unreadable` | refused | a file in the store that does not read back as a note |
| `UnclosedFence` | refused | a code fence that never closes, which quotes everything after it |
| `MissingSection` | refused | a section the note's type must have |
| `ProseInListSection` | refused | a paragraph in a section that holds a list and nothing else |
| `KeyDisagreesWithPath` | refused | `key` says one thing and the filename another |
| `UnevaluatableTrigger` | refused | an `invalidated_by` omh has no way to check |
| `DuplicateKey` | warning | one key claimed by two files |
| `DanglingLink` | warning | a `[[link]]` to a key that is not in the store |
| `Orphan` | warning | a note nothing links to |

**Only a refusal makes `omh memory lint` fail.** A warning is reported and does
not: `Orphan` fires on every note nothing links to, so failing on warnings
would fail on the store's shape.

## Keys

**A key is derived, never chosen**, from a template per type in
`<repo>/.omh/memory.toml`. This is what ships:

```toml
# How a note's key is derived. Identity, not a title — the agent never picks
# one, so the same observation cannot be recorded twice under two spellings.
[keys]
surprise = "surprise/{{slug}}"
topic    = "{{slug}}"
stub     = "{{path}}"
```

| placeholder | bound to |
|---|---|
| `{{slug}}` | for a `surprise`, the first sentence of what was observed — up to the first `.`, `!` or `?` — lowercased, with every run of anything that is not a letter or a digit collapsed to one `-`. For the `topic` that `omh init` writes, the repo's name under the same rule |
| `{{path}}` | for a `stub`, the document's path in the repo without `.md`, each segment put through the same rule: `docs/Deploy Notes.md` is `docs/deploy-notes` |

**`omh init` writes the file once and never refreshes it**, because a template
that changed under an existing store would re-key every note written from then
on.

**A placeholder nothing binds is an error** that names it, not a literal
`{{slugg}}` inside a key.

**The result has to be slash-separated slugs**: no empty segment, none that
starts with a `.`, no backslash. The file is committed, so a clone supplies it,
and a key is also a path — that rule is what stops a template writing outside
the store.

**With no `memory.toml`, the shipped templates apply.** A `keys.toml` beside
it — the file's name before it held more than keys — is
[refused by name](/guides/troubleshooting/#keystoml-is-where-key-templates-used-to-live)
rather than ignored.

## What recall matches

`recall` is one call: it ranks the notes a question matches, follows each
one's links one hop out, and returns the neighbourhood with every note's date.

```
surprise/mounting-a-credential-file-returns-ebusy  2026-08-07
└─ credentials-are-a-named-volume                  2026-08-05
```

- **Whole words, stemmed.** A question is split into words and each loses its
  plural and its tense, so `mounted` finds `mounting`. Nothing matches on part
  of a word.
- **Ranked by rarity.** A word in every note cannot tell two notes apart and
  barely counts; a word in one note counts for a lot. There is no stopword
  list.
- **A word in the key counts double.** Ties break on the more recent note,
  then on the one more notes link to.
- **`Answers` is indexed like the rest of the note**, not ranked above it.
- **Eight matched notes at most**, each with its neighbours. When more
  matched, the answer ends by saying how many were left out, so that a short
  answer is never mistaken for a complete one.
- **`also_phrased_as`** takes the same question in other words. A note is found
  by the wording it was written in, which is rarely the wording it is asked
  for in.

<details>
<summary>What was measured, and where the measurements live</summary>

Ranking `Answers` above the rest of the note was tried and cost ten points of
first-place precision on a forty-note store.

How those were measured, and on what, is beside the script that measures them:
[`scripts/bench-recall.md`](https://github.com/mindsers/ohmyharness/blob/main/scripts/bench-recall.md).

</details>

## What `omh init` puts there

`init` fills a new store with what the repo already documents — as pointers,
not summaries — so it is of use on the first day.

**One stub for every Markdown file git tracks**, outside `.omh/`. Its
`Answers` are that file's `##` headings, word for word:

```markdown
---
key: docs/deploy
type: stub
source: docs/deploy.md
recorded: 2026-08-07
---

# Deploying

Repo document. Read `docs/deploy.md`; this note points at it and does not restate it.

## Answers

- Staging
- Rolling back
```

**A file with no `##` heading gets one answer**, ``the whole of `NOTES.md` ``,
because an `Answers` section with nothing in it would be refused.

**And one `topic` for the repo**, keyed by its name: what `init` derived, each
line with the file it came from, and a link to every stub — which is what
keeps a new store from being all orphans.

<details>
<summary>Why pointers rather than summaries</summary>

A store that starts empty is no use on the first day. A summary drops the flag,
the path and the error string, and then drifts from the page it was made from;
a pointer sends the agent to the page itself.

</details>

## When a session is removed

Notes outlive the session that wrote them. `omh sNN rm` says so when there are
any, after its report:

```
3 notes recorded during this session — `omh memory` to review
```

It says nothing when the session recorded none, and nothing under `--json`.

## Next

<div class="cards">

- [`omh memory`](/reference/commands/#omh-memory-)\
  List, remember, lint and remove notes from the command line.
- [`omh memory stale`](/reference/commands/#omh-memory-stale)\
  The closed set of triggers `invalidated_by` can name.
- [Sessions](/concepts/sessions/)\
  What a session is: a container, a git worktree and a branch.

</div>
