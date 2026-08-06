# Memory

> **Status: designed, not built.** This page describes intent, not behaviour.
> Nothing here ships in v0 except the derived seeds `omh init` writes.

Memory is **layered like the profile**, not stored in one bucket. Global-only
and per-project-only are both wrong, for opposite reasons.

| Scope | Location | Holds | Example |
|---|---|---|---|
| personal | `~/.omh/memory` | facts about **you** | prefers TDD; dislikes defensive comments |
| project | repo-keyed volume | facts about **this codebase** | the sbx spike is unresolved; hooks live in layer 2 |
| team *(later)* | committed in the repo | facts the whole team should share | deploys need the VPN |

A query merges the layers and reports which one answered, exactly as
[`omh config`](../configuration.md#provenance) does for settings.

## Why not global-only

A single store accumulates thousands of repo-specific facts and retrieval
degrades into noise — you pay tokens loading facts about repo B while working in
repo A.

It also carries one client's context into another's session, which is a
confidentiality problem before it is a quality one.

## Why not project-only

You are one person across every repo. How you work, what you have already
learned the hard way, and what you keep correcting do not reset when you `cd`.
Re-teaching those per project is precisely the hassle omh exists to remove.

## Writes default to the narrower scope

Project unless promoted, mirroring `omh config set` defaulting to the gitignored
layer. The asymmetry is the point: a fact that should have been global is a mild
annoyance, and a client detail that should not have been global is not.

Promotion is deliberate — `omh memory promote <fact>`.

## Surviving a harness switch is orthogonal

Both layers survive a switch, because neither is keyed by harness. That property
comes from what memory is *not* keyed by, not from being repo-keyed — worth
stating because it is easy to attribute to the wrong design decision and then
"preserve" it by preserving the wrong thing.

## Seeding

`omh init` already derives facts rather than asking for them — from the README,
manifests, git log and any existing rules files. Same principle as everywhere
else: [derive, never interrogate](../getting-started.md#derive-never-interrogate).

Derived seeds also refresh when the repo changes, instead of going stale in a
config file nobody revisits.

## The unsolved part

A wrong **global** fact poisons every project, so global memory needs expiry and
an [`omh why`](trust.md) story more urgently than project memory does. Neither
exists yet.

Until they do, a memory store that accumulates confident wrong facts is worse
than no memory store — which is the honest reason this is designed and not
built.
