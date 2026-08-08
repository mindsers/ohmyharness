# Memory M0 — running iwe, instead of reading about it

> **Measured 2026-08-08**, iwe `v0.19.0`, on `aarch64`, in a container built
> from omh's own base recipe. Everything below is an observation with a command
> behind it, not a reading of the vendor's documentation.

[The spec](memory.md#15-open-questions) made this blocking, and named why:
everything the [survey](memory-rationale.md#survey-what-already-exists) said
about iwe came from its documentation — *"precisely the class of claim this
project treats as unverified: the graph server's own docs advertised a
`CBM_VARIANT=ui` switch its published installer ignored."*

The spike was worth running. **Four of the survey's claims did not survive it**,
and one of them blocks adoption outright.

## What was run

A store of ten notes in omh's own format, written from this repo's real history
— `EBUSY` on a bind-mounted token file, `info/exclude` in the common git dir,
the installer ignoring `CBM_VARIANT`, `--aspects` on a comma list — indexed and
queried and rewritten by iwe, unattended, as the non-root `agent` user.

## 1. It does not run on omh's base image

**This is the blocking finding.**

```console
$ iwe --version
iwe: /lib/aarch64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found
$ ldd --version | head -1          # omh's base: node:22-bookworm-slim
ldd (Debian GLIBC 2.36-9+deb12u14) 2.36
```

The published binary needs **glibc 2.39**; omh's base ships **2.36**. Isolated
against `debian:trixie-slim` (glibc 2.41), where the same binary runs fine, so
the cause is the base and nothing else.

iwe publishes `-gnu` targets only — there is no musl build to fall back to. So
adopting iwe means one of: moving omh's base off bookworm, building iwe from
source in the image, or asking upstream for a musl target. None is fatal; all
three are decisions the spec did not know it was making.

## 2. It is not "a single static binary"

The survey chose iwe partly on *"Rust, single static binary, no database — a
service to run is a decision `omh init` promised to remove."* The no-database
half holds. The rest does not:

| | claimed | measured |
|---|---|---|
| binaries | one | **three** — `iwe` (CLI), `iwes` (LSP), `iwec` (MCP) |
| linkage | static | **dynamic**, ELF PIE against glibc |
| size | — | **64 MB** installed |

The no-service claim survives, and it was the load-bearing half of that
argument.

## 3. Apache-2.0 confirms

`Apache License 2.0`, from the repository's own licence metadata. The one
survey criterion that mattered most — *"no repeat of the gitnexus problem"* —
holds.

## 4. It reads omh's notes unattended, and that part works

```console
$ iwe init
10 files here · 10 wiki links
note: frontmatter fields: key 100%, recorded 100%, source 100%, type 100%
```

It found every note, parsed omh's frontmatter, and read `[[wiki-links]]` as
edges without being told to. `iwe find credentials` returned the neighbourhood
with `type`, `source` and `recorded` attached — so the provenance omh cares
about is *available* to it.

What it cannot carry is **`layer`**, because the layer is not in the file. It is
the directory the note was read from, which is deliberate ([invariant
1](memory.md#11-invariants)): a note that can declare its own layer can claim to
have been reviewed. Any iwe-backed retrieval would have to have that stamped on
by omh afterwards — which is [§9.5](memory.md#95-why-omh-owns-the-surface)'s
argument arriving as a measurement rather than an assertion.

## 5. Its writes and omh's writes disagree about the same file

`iwe normalize` rewrote **all ten files in place**, unattended. The change was
formatting only — a blank line after each `##` heading — and the
`[[wiki-links]]` were untouched:

```diff
 ## Expected
+
 a bind mount of the host token dir
```

Harmless in itself, and omh still parses the result. But it means **two
renderers own the same bytes**: every note omh writes, iwe considers
un-normalized, and every note iwe normalizes, omh did not write. That is churn
with no owner, and it is the shape of the defect
[the rationale](memory-rationale.md) records as the vendor's own surviving
quality gap — *"model non-compliance that looks random is the signal to go and
read the write path."*

## 6. `iwe rename` breaks omh's identity model

This is the finding with teeth, and it is not a formatting quibble.

```console
$ iwe rename credentials-are-a-named-volume "Credentials are a named volume, renamed"
Renaming 'credentials-are-a-named-volume' to 'Credentials are a named volume, renamed'
Updated 2 document(s)
```

Three things happened, and each one is a problem:

1. The file became **`Credentials are a named volume, renamed.md`** — spaces and
   a comma in a name that omh derives canonically, precisely so that
   [one input yields one key](memory.md#6-identity).
2. The referencing note's link became **`[[Credentials are a named volume,
   renamed]]`**. A link target is a *key*, computable before its target exists;
   it is now a human title, and it resolves to nothing.
3. The renamed note's **frontmatter `key` was left untouched** at
   `credentials-are-a-named-volume`.

So one `rename` produces a note whose key disagrees with its filename — omh's
`KeyDisagreesWithPath`, a **refused write** — plus a dangling link, plus a
second key for a topic that already had one. `AGENTS.md`'s *rename through the
tool, never `mv`* was written on the assumption that the tool would keep
identity intact. Measured, it does the opposite.

## 7. The per-session tool tax, measured

[§9.5](memory.md#95-why-omh-owns-the-surface) argued the proxy partly on
dropping *"iwe's 13 tools to 2"*. Measured over a real MCP handshake with
`iwec`:

```
tools: 14
  iwe_attach, iwe_create, iwe_delete, iwe_extract, iwe_find, iwe_inline,
  iwe_normalize, iwe_query, iwe_rename, iwe_retrieve, iwe_squash, iwe_stats,
  iwe_tree, iwe_update
description bytes: 3787
```

**14, not 13** — a small correction, and the first number in this design that
was counted rather than recalled. `iwec` speaks protocol `2025-06-18`, the same
version omh's own server negotiates.

## What this changes

Nothing that is built. M1 and M2 are plain files in a directory omh owns, with
no indexer underneath, and none of the above touches them — which is why
[the plan](memory.md#14-build-order) put the spike outside the build rather
than in front of it.

What it changes is the **case for adopting iwe at all**, and the honest summary
is that the survey's reasons are weaker than they read:

| survey's reason | after running it |
|---|---|
| Apache-2.0 | **holds** |
| no database, no service | **holds** — and it was the load-bearing half |
| single static binary | **wrong**: three dynamic binaries, 64 MB |
| runs anywhere | **wrong**: not on omh's base image |
| it is also an LSP, so the editor and the agent read one graph | **untested here** — `iwes` was not exercised, and this remains the strongest remaining argument |

Against that, omh's own retrieval is a linear scan over a few hundred Markdown
files, it already carries the layer iwe structurally cannot, and it exists.
`recall::search` is the seam an iwe-backed retriever would replace without
touching the tool surface or invariant 1, so **this decision does not have to be
made now** — which is the useful thing the spike bought.

If it is adopted later, two things are prerequisites rather than details:

- **iwe never writes to the store.** `normalize` and `rename` both produce notes
  omh's schema refuses. Read-only, or not at all.
- **a base image that can run it**, chosen deliberately rather than discovered.

## What was not tested

Stated so nobody reads more into this than it says:

- **`iwes`, the LSP** — the editor-and-agent-share-one-graph argument, which is
  the best reason left to adopt iwe, is still untested.
- **retrieval quality.** Nothing here compares iwe's answers to omh's, or to
  grep. That is [§13](memory.md#13-measurement)'s job and it needs a question
  set written by somebody who did not write the store.
- **scale.** Ten notes. Nothing here says what happens at a thousand.
