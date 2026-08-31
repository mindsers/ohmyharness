<!--
The checklist is from CONTRIBUTING.md. It is short because every line of
it was earned by a bug that shipped.
-->

## What this changes

<!-- One or two sentences. What is different afterwards, and why. -->

## Checklist

- [ ] `./scripts/check.sh` — format, lints and the suite. Run this rather than
      `cargo clippy`: it first checks that the clippy answering you was built
      against the rustc you have, because a stale shim refuses on the crate's
      `rust-version` and prints a dependency wall instead of a lint.
- [ ] If you added a base-set entry: all four fields, the cost **measured**
      with its method and date, and anything you rejected recorded as
      `[[rejected]]` so nobody re-litigates it. See
      [proposing an entry](../docs/design/base-set.md#proposing-an-entry).
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
