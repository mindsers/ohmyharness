<!--
The checklist is from CONTRIBUTING.md. It is short because every line of
it was earned by a bug that shipped.
-->

## What this changes

<!-- One or two sentences. What is different afterwards, and why. -->

## Checklist

- [ ] `cargo test`
- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `omh doctor` — **required if you touched an adapter, an image, or a mount.**
      `cargo test` passing tells you nothing about those; they assert facts
      about external software and break silently.

## The test went red first

<!--
For a bug fix: the regression test must have failed *before* the fix landed.
A green suite is not evidence on its own.

Say which test guards this and what you saw it do when the bug was present.
If you wrote the guard after the fix, say so — three guards in this repo were
written that way and all three were too weak. Better flagged than assumed.
-->

## Invariants

<!--
CONTRIBUTING.md lists the invariants, each with a test that fails without
it. If you changed one, say which and why — that is a product change, not a
test fix. Delete this section if none are affected.
-->
