# Contributing

```console
$ cargo test                    # the suite
$ cargo clippy --all-targets -- -D warnings
$ ./scripts/test-install.sh     # install.sh, and every refusal it should make
$ omh doctor                    # the only thing that verifies an adapter
```

A few tests are `#[ignore]`d because they need `node` on `PATH`: they run the
JavaScript omh generates through `node --check`, which is the only thing that
proves a rendered module will load. `cargo test -- --include-ignored` runs them,
and so does CI.

Four more used to be ignored because they shell out to git, which could not run
inside an omh sandbox. That stopped being true in 2026.08 — see
[adoption](../docs/design/adoption.md) — and they run with everything else now.

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

> *the writable mounts are exactly this named set*

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
| every writable mount is one of a named set | a stray `rw` is the difference between a sandbox and a suggestion |
| credentials mount where the harness reads, writable | anywhere else and the session is logged out; read-only and refreshed tokens vanish |
| a named-but-missing account stops the launch | otherwise the session runs logged out and says nothing |
| staged links resolve under `/omh/layers/…` | host paths do not exist in the sandbox; skills silently vanish |
| staging is keyed by session **and** harness | else a second harness overwrites the first's mounted config |
| unknown adapter fields are rejected | else a stale adapter degrades everything, silently |
| `rm` keeps a branch that has commits; `ensure` reattaches | unreviewed agent work must be unloseable |
| the rules omh stages are mounted, never written into the worktree | written there they are indistinguishable from the agent's work, and `info/exclude` cannot hide a file git tracks |
| `s commit` never commits a file omh put in the worktree | omh's own `CLAUDE.md` in the user's PR is omh corrupting the work it exists to isolate |
| a key that can name a credential never lands in a committed file unasked | `omh set` defaults to the committed file, so the protection is a property of the key, not of the command — `src/key.rs` is the whole of it |
| the file omh hides a secret in is a file git ignores | it is routed there *because* it is hidden, and the ignore line used to be `omh init`'s alone |
| rule 2 only moves a write away from the committed file, never toward it | the symmetric version is the obvious-looking repair and it commits `carry_in` |
| `omh unset` reaches every repo layer that holds the key | removing where `set` would have written left a committed `carry_in` standing and reported success |
| a `carry_in` entry git already tracks is reported, not copied | it overwrites the branch's copy with the checkout's, and the only path a secret takes to the agent is not one to widen |
| `s commit` refuses a carried file rather than dropping it | omh cannot tell a credential from a deliberate change, and silently discarding either is worse than stopping |
| every hook command parses and runs under `sh` | a hook that cannot parse satisfies every assertion about its text and never runs |
| `s commit` writes no message the user did not write | a synthesized message is a claim about intent omh does not hold |
| nothing to commit is never a successful commit | `-q` hides git's own complaint, so the user gets a bare error and a commit that never happened |
| `s commit` stages before asking whether anything changed | `git diff` says nothing about untracked files, so a session whose only work is new files reads as clean |
| `s push` never invents a branch name | `omh/s01` on origin outlives the session that would explain it |
| `s push` reads the branch back from origin before passing | every local step can succeed while the remote stays untouched |
| `omh s` never reports a session holding work as clean | the state that strands work is the one it exists to surface |
| the git notice names what to run instead, not just what is missing | an agent told only that git is broken offers to commit work it has no way to commit |
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
| no command name can be shadowed by an adapter | every command is named, so a bare word is not a launch and there is no shared slot |
| every relative link in the docs resolves | a doc tree rots silently; nothing else notices |
| the note store outlives the worktree that wrote it | a store under /work dies with `git worktree remove --force` |
| a note is never listed without its date and its layer | an undated, unattributed claim cannot be judged |
| `omh memory rm` never touches a neighbour | a pruned neighbourhood is invisible; a dangling link is not |
| the note template in the staged rules parses | it teaches a shape the store then refuses, silently |
| a retrieved note always carries its date and layer | an unattributed claim launders a guess into a fact |
| the memory server is pointed at the paths omh mounts | it starts, finds nothing, and "0 notes" reads as an empty store |
| a generated stub passes the same schema an agent's write does | ingestion half-populates the store while `init` reports success |
| a committed note links only to committed notes | the link dangles in a teammate's clone, where the target does not exist |
| the gitignored layer never reaches a clone | every private note is published to the whole team, silently |
| `promote` rewrites nothing but the note it was given | a renderer touching what it was not asked to is invisible to every semantic test |
| writing a note never rewrites its link text | the shape of the vendor's own remaining quality gap, guarded on omh's renderer. A byte comparison catches a rewrite that differs each pass; an idempotent one is caught only by asserting the link is still there, since both writes agree |
| `stale` never reports "cannot tell" as "still current" | it makes the command a liar rather than merely incomplete |
| an image digest a note pins is stable across toolchains | `DefaultHasher` marks every pinned note stale on a Rust upgrade |

Notice how many of them are about **failing loudly**. That is the recurring
lesson of this codebase: the expensive bugs were not crashes, they were things
that worked, said so, and were wrong.

## Documentation

Docs live in [`docs/`](../docs/README.md) and are tested — `tests/docs.rs` checks that
every relative link resolves and that no page is orphaned. Add a page, link it
from [`docs/README.md`](../docs/README.md).

Where a number appears, it should be a measurement, and it should say what it
measured. Where something is unverified, say so. The project's credibility rests
on the difference between those two, and the moment a doc claims more than
`doctor` can prove, the whole set becomes marketing.

## Before opening a PR

1. `cargo test`
2. `cargo fmt`
3. `cargo clippy --all-targets -- -D warnings`
4. `omh doctor` if you touched anything a harness reads
5. Reintroduce the bug your new test guards; confirm it goes red

Step 5 is the one people skip. It is also the one that catches tests that assert
nothing.
