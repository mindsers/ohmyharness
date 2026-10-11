# omh documentation

Published at <https://docs.ohmyharness.sh>, from this folder.

| | |
|---|---|
| `src/content/docs/` | the pages of the site, one folder per section of its sidebar |
| [`design/`](design/) | design pages, read here rather than on the site: what was tried, what it cost, what was decided |
| everything else | the site itself, built with [Astro Starlight](https://starlight.astro.build) |

## Working on a page

A page is a markdown file under `src/content/docs/`. It opens with a `title`
and a `description`, and has no `# heading` of its own — the site prints the
title. The sidebar lists it from the folder it is in.

A link to another page of the site is its address, `/reference/commands/`. A
link to anything else in this repository is its URL on GitHub. `tests/docs.rs`
holds both, down to the heading a link names, so `cargo test --test docs` is the
check to run after moving or renaming anything.

## Building the site

```console
$ cd docs
$ mise install      # node and pnpm, at the versions mise.toml pins
$ pnpm install
$ pnpm dev          # the site on your machine, reloading as you edit
$ pnpm build        # the site, built into dist/
```
