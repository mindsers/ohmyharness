---
title: Retrospectives
description: Four bodies of work that shipped outside a milestone, what each one changed, and what it cost.
---

What shipped outside a [roadmap](/project/roadmap/) milestone, and what it cost
to get right — the part worth reading before proposing the next change.

Each retrospective was written when its work landed. Each says what was wrong
and what changed; the later ones also say what it cost.

## At a glance

| Retrospective | Shipped | What changed | What it cost |
|---|---|---|---|
| [The git repairs](#the-git-repairs--done-4652) | 0.6.0, #46–#52 | four measured defects in getting work out of a session, one of which deleted unreviewed commits | not recorded |
| [The command surface](#the-command-surface--done-070-6990) | 0.7.0, #69–#90 | a command name can no longer collide with a harness name, and scope is visible in the verb | a release with no backward compatibility — and three wrong behaviours a green, reviewed release still had, found only by running the commands |
| [The silent failures](#the-silent-failures--done-080-9192) | 0.8.0, #91–#92 | two named risks closed, a lint that never ran, `omh eject`, and `main.rs` split up | the fix for 8d reopened 8d one line down, and three guards passed against broken code — found by review, not the suite |
| [Say it before it bites](#say-it-before-it-bites--done-090-9497) | 0.9.0, #94–#97 | `omh doctor` reports the host, a TLS-inspecting proxy is named, and `rm` reports only what it removed | not recorded |

## The git repairs — done (#46–#52)

Not a milestone; shipped in 0.6.0. Four measured defects in the path that gets
work out of a session, one of which deleted unreviewed commits —
[risks](/concepts/risks/) 4b and 8b. They landed first, each with its failing
test before the fix, because the
[work loop](/project/roadmap/#v15--the-work-loop--060) rested on them.

**Both cheap hardenings rode along, as planned:** the sandbox's exclude list is
rewritten on every launch (4c), and its `config` is mounted read-only (2b),
which mattered once omh started reading that gitdir from the host.

## The command surface — done (0.7.0, #69–#90)

Not a milestone either. The CLI had grown one command at a time and the shape
showed it, in four structural ways rather than cosmetic ones.

**A bare word was a harness.** A harness name typed on its own worked through
an external-subcommand catch-all, so any adapter could shadow any command — and
19 hardcoded `RESERVED` words existed only to stop that, while `session_prefix`
parsed argv twice to arbitrate which reading won. It is `omh new <harness>`
now, the catch-all and its two guards are gone, and a leading `sNN` scopes a
top-level command as soundly as it scopes a session verb.

**Two scopes, four nouns.** `config` and `repo` each carried `set`/`unset`
plus their own verbs, and which file a write landed in was a rule
you held in your head. One rule serves four commands: the write reaches the
layer that already declares the key, else the key's own classification decides,
and `--local` or `--save` names a file on purpose. What keeps a credential out
of git moved from the command's default to `src/key.rs`, per key — fail-safe by
enumeration rather than by destination, which is a real trade and is recorded
in [decisions](/project/decisions/#write-default-the-key-decides).

**`omh init` reported what existed, not what it did.** It opened with the
machine's harnesses and editors — true before the command ran. That is
`omh info` now, and `init` answers what this repo took, what it left in your
catalogue, and what to run next.

**A flag said one thing and did another.** `--dry-run` was global, ten commands
read it, and the rest accepted and discarded it — so `omh --dry-run use --all`
wrote the file and printed `wrote →`. It runs everything and withholds the
writes now, and a command that cannot yet answer it refuses the flag rather
than running: no command silently ignores it. `-a`/`--account` went the same
way for the same reason — it overrode the account for one invocation and
recorded nothing, so a session started with it could not be resumed without
repeating it. The account is `omh set account <name>`, checked against the
logins you have actually captured.

**Success criterion:** one migration, after which a command name cannot collide
with a harness name and scope is visible in the verb. **Met**, at the cost of
one release with no backward compatibility: retired spellings are refused by
name with their replacement, and nothing retired parses.

**What it cost** is worth reading before the next surface change, in two parts.

**The guards did more of the work than the sweeps did**, and where a guard
could not see, the sweep shipped falsehoods. A rename applied by spelling
inverted every historical sentence it touched; needles that could not span a
line break hid four sites; and a synopsis block in the commands reference was the
one artefact neither compiled nor parsed, which is where the four wrong lines
landed. Each is now scanned.

**And running the commands found what the suite could not.** The release was
ready, green, and reviewed when every command was exercised by hand against a
real repo: `omh why` printed a way out that removed a third of what it claimed,
`.mcp.json` — omh's own mount placeholder — was being committed onto session
branches, and the session prefix worked only as the literal first word. None of
those had a test that could fail, because each was a claim about *what a
command does*, and the suite asserted what the code returns. The guards added
since assert effects: what `remove` leaves behind, what a launch leaves in the
worktree, what a dry run leaves on disk.

## The silent failures — done (0.8.0, #91–#92)

Not a milestone either. An audit of 0.7.0 found the engineering sound and the
weaknesses concentrated in things that **did not fail** — which is why they had
survived. Two were already written down on [risks](/concepts/risks/) and unfixed.

**Two checkouts of the same name shared everything** (8d). Worktrees, run
state, keys, sandbox repositories, the note store, the cache volume, the
network and container names were all keyed by the checkout's directory name,
so `~/work/api` and `~/oss/api` were one repo and the second one's `omh new`
resumed into the first one's session. Keyed by name *and* a digest of the path
now, with a one-time migration that reads ownership off each worktree's `.git`
pointer rather than assuming it.

**The carried-file scan said nothing when it could not look** (4d). Three
shapes yield no needles — not text, not there, nothing long enough — and all
three returned the same value as *searched and found nothing clean*. They
still fail open, deliberately; they are reported now, at launch and at
harvest. A fourth was worse than silent: `PermissionDenied` on a `chmod 600`
`.env` aborted the harvest entirely, losing the agent's commits over a file
mode that is the ordinary state of a secret.

**A lint that had not run for an unknown length of time.** `cargo clippy`
resolved to a stale shim and refused on the crate's `rust-version`, printing a
dependency wall instead of a lint. Nothing failed; the lint simply never ran.
`scripts/check.sh` compares the rustc clippy was built against with the one
you have, and the crate's MSRV with both.

**`omh eject`** ships — the exit [trust](/concepts/trust/) has argued for since it was
written. It also names the files it wrote that still reference `/omh`, `/work`
or `$OMH_*`, because handing somebody a directory and saying *these are yours
now* while several of the files silently do not work is the same defect in a
new place.

**And `main.rs` stopped being where everything lived**: 8,434 lines of
production code down to 628, across `cli.rs` and nine modules under `cmd/`,
with a budget test that keeps it that way. The tests have not moved with it,
which is recorded as a debt rather than described as finished.

**Success criterion:** the two risks the page had named for weeks are closed,
and nothing omh cannot check reads the same as something it checked. **Met.**

**What it cost** is the part worth keeping. The fix for 8d **reopened 8d one
line down** — a `!entry.is_dir()` filter added while closing it, and `is_dir()`
answers `false` for every error, so a dangling symlink took the accounting
with it. Three of the guards written alongside these fixes passed against a
broken implementation, one of them the whole `profile` suite green against a
digest that re-collides the two paths it exists to separate. Both were found
by review rather than by the suite, which is the same lesson 0.7.0 recorded
and did not transfer.

## Say it before it bites — done (0.9.0, #94–#97)

Not a milestone. Four changes with one theme: omh knew things it was not
saying, and each silence cost somebody an hour.

**The host was never inspected** (#95, #96). `omh doctor` only ever spoke from
*inside* a sandbox, so on a machine where the sandbox could not be built it
answered with a single error about the thing that failed last. It reports the
host first now — the runtime and whether it is actually *answering* rather than
merely installed, the stacks, the settings omh reads and the keys it does not,
free disk, what omh has left behind, which omh version set this checkout up,
and whether the repo has a commit to fork a session from. Every row is a thing
that changes how omh behaves with nothing else saying so.

**A TLS-inspecting proxy looked like a broken build** (#94). Behind a corporate
root, image builds failed deep inside docker with no line naming the cause.
[`ca_cert`](/reference/configuration/#ca_cert) puts the root in the sandbox's trust
store — all three layers, not just the base — and doctor says whether traffic is
being inspected and whether omh has been told about it.

**`rm` reported removals it had not performed** (#97). `git worktree remove` can
exit 0 having unlinked nothing, and that was believed: `removed session sNN`,
exit 0, worktree still there. The disk is asked now, a removal that did not
finish is a non-zero exit that names what survived, and the report lists only
the parts omh watched go. `rm` also asks before destroying commits no branch
has, instead of refusing and telling you to retype the command with `--force`.

## Next

<div class="cards">

- [Roadmap](/project/roadmap/)\
  The milestones this work happened between.
- [Risks](/concepts/risks/)\
  The numbered weaknesses these retrospectives close.
- [Decisions](/project/decisions/)\
  The choices behind them, and three that were reversed.

</div>
