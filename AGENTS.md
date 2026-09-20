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

Prefer asserting invariants over asserting output shape. `every writable mount
is one of these, by name` survives refactoring; `the 4th mount string equals
"..."` does not.

This rule exists because ~950 lines shipped untested and carried four bugs, all
in pure, cheaply-testable code, all caught by a human reading tool output.

## A new command or flag needs a run

`every_command_and_flag_is_exercised_by_a_real_run` in `tests/cli.rs` asks the
binary what omh accepts and fails when anything in that answer has no run
behind it. Declare a flag and it fails by name:

```
1 of omh's N commands and flags have no run behind them:
  omh graph --loudly
```

Add a line to `EXERCISES` — a real argv, not a claim about one, since what it
covers is derived from the argv itself. The container-backed half is
`#[ignore]`d, which means the linux job runs it on every push and the macOS
job does not. A command that cannot
run for real gets a stand-in, named as one where it is built: on the sandbox's
PATH when it is a program omh runs (`gh`, an editor), on disk when it is a file
omh reads (a captured login). An editor especially — without a stand-in
`attach` finds the real one, and the suite opens windows on your desktop.

The container-backed tests each hold a Docker network while they run, and
Docker's address pool is finite: a few of them in parallel on a machine with
leftovers is `all predefined address pools have been fully subnetted`. Clear
stale ones with `docker network ls | grep omh-` before blaming your change.

## Honesty about coverage

Adapter paths assert facts about *external software*. A passing suite proves omh
mounts a path faithfully, never that a harness reads it. Do not cite green tests
as evidence an adapter is correct — that needs `omh doctor`.
