# Contributing

```console
$ cargo test            # the suite
$ ./scripts/smoke.sh    # end-to-end walkthrough in a throwaway repo
$ omh doctor            # the only thing that verifies an adapter
```

## Read this first

**Adapters assert facts about external software**, and those facts break
silently. A wrong path does not crash anything — the harness starts, and simply
never sees your profile.

Almost every bug this project has shipped lived at that boundary, and **not one
was catchable by the test suite.** If you change an adapter, an image, or a
mount, `cargo test` passing tells you nothing. Run `omh doctor`.

This is the single thing most likely to mislead you here, which is why it is at
the top rather than in a footnote.

## TDD, always

Write the failing test. Watch it fail. Then implement.

For a bug fix, **the regression test must go red before the fix lands.** A
regression test that never went red proves nothing — it may be asserting
something that was already true.

**A green suite is not evidence on its own.** Reintroduce the original bug and
confirm the guarding test turns red, or the test is decoration.

### Do not fix the defect before the guard is red against it

The order is not a formality, and reintroducing the bug afterwards is a weaker
substitute than it looks — **you choose the mutation, and you will reliably
choose one your test already catches.**

Three guards written in this repo were too weak for exactly this reason, all
three written after their bug was already fixed:

| Guard | Written against | Missed |
|---|---|---|
| measurement dates | a manifest already corrected | compared by month, so it passed `2026-08-04`, the fabricated date that shipped |
| staleness | `is_stale` already switched to the manifest version | asserted "stale" appeared *anywhere*; two call sites satisfy that, so mutating one stayed green |
| cost carries its date | the render code being written beside it | checked a date was *present*, never that it was true — seven invented dates walked through it |

Each was then "verified" by a mutation chosen from memory, and twice the
remembered bug was easier than the real one: blanking a date rather than making
it plausible-but-false, mutating the call site the assertion did not read.

**The original defect is the best mutation you will ever have.** Fix it before
the guard is red and you have thrown away the only one you did not invent.

This whole rule exists for a specific reason: roughly 950 lines shipped untested
in this repo and carried four bugs, all in pure, cheaply-testable code, all
eventually caught by a human reading tool output.

### Assert invariants, not output shape

> *exactly one writable mount, and it is the worktree*

survives refactoring.

> *the 4th mount string equals "..."*

does not, and worse, it fails for reasons that have nothing to do with the thing
it was protecting — which trains people to update tests reflexively instead of
reading them.

The architecture favours this. `document()` is pure; `plan()` and the backends'
argument construction are pure given a temp filesystem. **The places where a
mistake is invisible are the places that test cheaply**, which is not an
accident and is worth preserving.

## The invariants

Each has a test that fails without it. If you break one, you are changing the
product, not fixing a test.

| Invariant | Why |
|---|---|
| nothing beyond the worktree and credentials is writable | a stray `rw` is the difference between a sandbox and a suggestion |
| credentials mount where the harness reads, writable | anywhere else and the session is logged out; read-only and refreshed tokens vanish |
| a named-but-missing account stops the launch | otherwise the session runs logged out and says nothing |
| staged links resolve under `/omh/layers/…` | host paths do not exist in the sandbox; skills silently vanish |
| staging is keyed by session **and** harness | else a second harness overwrites the first's mounted config |
| unknown adapter fields are rejected | else a stale adapter degrades everything, silently |
| `rm` keeps a branch that has commits; `ensure` reattaches | unreviewed agent work must be unloseable |
| `rm` drops a branch with no commits | it preserves nothing, and dead refs hide the live ones |
| each MCP format emits its harness's real schema | a wrong shape means zero servers and no complaint |
| every renderer round-trips through its parser | else `import` silently drops data |
| a probe with no output is never a pass | silence means the sandbox never ran it |
| the declared token is atomically writable | a bind-mounted file returns `EBUSY` and the login never persists |
| an account name is a single path component | otherwise credentials mount the user's real `~` writable |
| a runtime failure is never a successful login | stale credentials would read as a fresh capture |
| sshd publishes on 127.0.0.1 only | `0.0.0.0` exposes a shell in the sandbox to the LAN |
| a session runs detached, never `--rm` | it must outlive the terminal, or nothing can attach |
| a plan is rejected when the backend lacks a capability | else `sbx` fails mysteriously instead of loudly |
| no command name can be shadowed by an adapter | `omh <anything>` is a harness, so `RESERVED` is load-bearing |
| every relative link in the docs resolves | a doc tree rots silently; nothing else notices |

Notice how many of them are about **failing loudly**. That is the recurring
lesson of this codebase: the expensive bugs were not crashes, they were things
that worked, said so, and were wrong.

## Documentation

Docs live in [`docs/`](README.md) and are tested — `tests/docs.rs` checks that
every relative link resolves and that no page is orphaned. Add a page, link it
from [`docs/README.md`](README.md).

Where a number appears, it should be a measurement, and it should say what it
measured. Where something is unverified, say so. The project's credibility rests
on the difference between those two, and the moment a doc claims more than
`doctor` can prove, the whole set becomes marketing.

## Before opening a PR

1. `cargo test`
2. `cargo fmt`
3. `omh doctor` if you touched anything a harness reads
4. Reintroduce the bug your new test guards; confirm it goes red

Step 4 is the one people skip. It is also the one that catches tests that assert
nothing.
