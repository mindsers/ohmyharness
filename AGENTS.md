# Project rules

## Test-driven development, always

Write the failing test first. Watch it fail. Then implement.

For a bug fix, the regression test comes first and **must fail before the fix
lands** — a regression test that never went red proves nothing.

A green suite is not evidence on its own. Reintroduce the original bug and
confirm the guarding test turns red, or the test is decoration.

**Do not fix the defect before the guard is red against it.** Reintroducing it
afterwards is weaker than it looks: you choose the mutation, and you will pick
one your test already catches. Three guards here were written after their bug
was fixed and all three were too weak — one compared dates by month and passed
the exact fabricated date that shipped. The original defect is the best mutation
you will ever get; fix it first and you have thrown it away.

Prefer asserting invariants over asserting output shape. `exactly one writable
mount, and it is the worktree` survives refactoring; `the 4th mount string equals
"..."` does not.

This rule exists because ~950 lines shipped untested and carried four bugs, all
in pure, cheaply-testable code, all caught by a human reading tool output.

## Honesty about coverage

Adapter paths assert facts about *external software*. A passing suite proves omh
mounts a path faithfully, never that a harness reads it. Do not cite green tests
as evidence an adapter is correct — that needs `omh doctor`.
