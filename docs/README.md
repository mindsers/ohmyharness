# omh documentation

Published at <https://docs.ohmyharness.sh>, from this folder.

| | |
|---|---|
| `src/content/docs/` | the pages of the site, one folder per section of its sidebar |
| [`design/`](design/) | design pages, read here rather than on the site: what is proposed and not yet built |
| everything else | the site itself, built with [Astro Starlight](https://starlight.astro.build) |

## Working on a page

A page is a markdown file under `src/content/docs/`. It opens with a `title`
and a `description`, and has no `# heading` of its own — the site prints the
title. The sidebar lists it from the folder it is in.

A link to another page of the site is its address, `/reference/commands/`. A
link to anything else in this repository is its URL on GitHub. `tests/docs.rs`
holds both, down to the heading a link names, so `cargo test --test docs` is the
check to run after moving or renaming anything.

## Writing a page

People arrive with a task. A page should let them find it, do it, and leave —
with the detail there for whoever wants it, not in everyone's way.

### The shape of a page

1. **One sentence that says what the page lets you do.** The site sets the first
   paragraph larger; make it the promise, not the history.
2. **What you need first**, if anything: `**You need** omh installed, Docker
   running…`.
3. **The task or the reference** — the thing the reader came for — before any
   explanation of it.
4. **Next:** cards to the two to four pages a reader goes to from here.

### Sentences

- Lead with the action or the fact. The reason comes after it, in a sentence,
  or in a `<details>` below.
- Three or four sentences to a paragraph, one idea each. When a reader will scan
  for that idea, bold its opening words.
- Talk to the reader, in the present tense: "omh builds the image", not "the
  image is built". No "simply", "just" or "easy".
- A rule the reader must follow and a rule omh enforces read differently; say
  which one it is.

### Keep the depth, move it out of the way

The history, the measurement, the alternative that was tried — omh's docs are
good at these, and they stay. They go in a `<details>` whose summary says what
is inside, or on a Concepts page the task links to. **Never delete a fact to
make a page shorter; move it to where it belongs.**

### Components

Pages are plain markdown: `tests/docs.rs` reads `.md` files only, so there is no
MDX. These cover what MDX components would, and `src/styles/omh.css` styles
them.

**Steps** — an ordered list in a wrapper. The first sentence of each item is
its title; the rest, indented three spaces, stays inside the step:

````md
<div class="steps">

1. Set the repository up.

   ```console
   $ omh init
   ```

2. Log in to Claude.

</div>
````

**Cards** — links, each with one line under it. The whole card is clickable:

```md
<div class="cards">

- [Install omh](/getting-started/installation/)\
  Homebrew, the install script, or from source.

</div>
```

**Details** — the *why*, folded:

```md
<details>
<summary>Why init never asks a question</summary>

Markdown, with a blank line after the summary.

</details>
```

**Asides** — `:::caution[Title]` for anything that can lose work or break a
setup, `:::note[Title]` for context a reader would want before going on,
`:::tip[Title]` for a shortcut. One per screen at most.

**Code** — `console` for what a person types, with a `$` prompt; `text` for what
omh prints, with `title="omh init"` naming the command. Keep a trailing comment
short enough to fit without scrolling.

**Tables** — for options, flags and settings: name, what it does, default.

**Openers** — a page longer than a few screens opens with a map of itself,
under the purpose sentence:

- a reference or a ledger (commands, decisions, retrospectives) opens with an
  `## At a glance` table, one row per entry, each linking to its section;
- an essay (the Concepts pages) opens with an **In short** list of three to
  five claims, each linking to the section that argues it;
- a troubleshooting page opens with what you see → the section that fixes it.

**Status** — one legend for the whole site: ✅ shipped, 🔶 partly built, ⬜ not
started. A page's own status line goes in a `:::note`, after the purpose
sentence, never before it.

**Alternatives** — when a fix is "do A, or B", use a bullet list with bold
openings; numbered steps are for things done in order.

**Commands stay in fences** — `src/main_tests.rs` reads every fenced `omh …`
line on the site, checks omh accepts it, and pins the count per page. A command
moved into prose is a command nothing checks; keep it fenced, and when a page
gains or loses one, update its count in that test's map, with a word on what
changed.

### Reference entries

Every command and every setting reads the same way, so a reader learns the
shape once:

1. The heading is its syntax: `` ## `omh sNN diff [n] [-p]` ``.
2. One sentence: what it does.
3. A usage block, then a table of its flags or values.
4. One example, with what it prints.
5. Then the edge cases, as short paragraphs with bold openings — and the
   reasoning behind them in a `<details>`.

### Headings are addresses

A heading's text is the anchor every link to it uses, and some of those links
are outside this folder: the README, ohmyharness.sh, and the omh binary itself,
which prints
`/guides/troubleshooting/#an-unknown-issuer-error-when-omh-builds-an-image`.
Before renaming a heading, search for its slug — `git grep '#its-slug'`, and in
the ohmyharness-website repository — and keep it if anything outside the docs
uses it. `cargo test --test docs` catches the links inside this repository; it
cannot see the others.

### Checking a page

`pnpm dev`, then look at it at desktop and phone width, in the light and the
dark theme. `pnpm build`, then `cargo test --test docs` from the repository
root.

## Building the site

```console
$ cd docs
$ mise install      # node and pnpm, at the versions mise.toml pins
$ pnpm install
$ pnpm dev          # the site on your machine, reloading as you edit
$ pnpm build        # what CI runs, and what a release publishes
```

It is published to GitHub Pages when a release is tagged — see
[`.github/workflows/docs.yml`](../.github/workflows/docs.yml).
