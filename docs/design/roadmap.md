# Roadmap

Ordered by what gates what, not by what is most fun.

**These are milestones, not crate versions.** The crate is at `0.7.0` and
milestone v1.5 is roughly what 0.6.0 contained. They are deliberately not
kept in lockstep — v0 has spanned eight releases already: a milestone moves when
a body of work lands, semver moves on every release, and calling the crate `1.0`
would imply a stability this project has not earned — one verified harness, one
verified runtime.

## v0 — the base set, one harness

✅ `omh init` that decides · ✅ images · ✅ sandbox + worktree · ✅ persistence ·
✅ `omh auth` · ✅ `omh s attach` · ✅ `omh doctor` · ✅ `carry_in` ·
✅ code graph · ✅ `omh graph` · ✅ stack hooks ·
🔶 [memory](memory.md) — the store, retrieval, the team layer and staleness
ship; hub pages do not

**Success criterion:** `omh init && omh new claude` is visibly better than raw
`claude`, with zero questions asked.

*Not in v0: further adapters, memory, breadth in the capability superset.*
[Breadth before depth is how distributions die.](adapters.md#breadth-is-capped-on-purpose)

## v0.5 — backends

The runtime trait exists and declares capabilities. What is missing is the
spike: build an opencode kit, try a single-file mount, try attaching an IDE.

It is roughly an afternoon, and it decides whether `sbx` becomes the default or
stays opt-in hardening. Until it runs, **Docker is the only verified runtime**
and the [credential weakness](risks.md#security) stays unaddressed.

## v1 — accountability

✅ [`omh why`](trust.md) · ⬜ cost accounting

`why` ships: every base-set entry answers who installed it, what it costs, what
was considered instead and how to remove it — and the four fields are enforced
by a test rather than by convention, so an entry cannot be added without its
reasoning. The base set moved out of the binary into a versioned manifest to
make that possible; `init` seeds from it and `why` explains from it.

What remains is the rollup: what the **whole** set injects, measured in bytes,
deterministically and without an LLM in the loop. Per-entry costs are recorded
in the manifest today, and one of them — the grep nudge — is computed by a test
rather than typed, which is the shape the rest should take. The ones needing a
live MCP connection are `doctor` territory rather than a pure function.

This is deliberately *not* a benchmark. The reasoning is in
[measure the cost, argue the benefit](trust.md#measure-the-cost-argue-the-benefit)
— briefly, an eval suite over a stochastic metric costs real money per decision,
measures tokens rather than correctness, and becomes the thing the base set is
tuned to pass. Distributions have never earned legitimacy that way.

What cost accounting buys instead is the one thing genuinely missing: a reason
for the base set to ever **shrink**. Arguments for adding are always available;
*"this now costs 4.1 KB before you type anything, up from 2.3"* is the fact that
forces the other conversation, and it is free to produce.

## The git repairs — done (#46–#52)

Not a milestone. Four measured defects in the path that gets work out of a
session, one of which deleted unreviewed commits — [risks](risks.md) 4b and 8b.
They landed first, each with its failing test before the fix, because the
milestone below rested on them.

Both cheap hardenings rode along as planned: the sandbox's exclude list is
rewritten on every launch (4c), and its `config` is mounted read-only (2b),
which mattered once omh started reading that gitdir from the host.

## v1.5 — the work loop ✅ (0.6.0)

[Git](git.md), built. All twelve steps in that page's ledger landed across
#53–#67, and the page names the pull request behind each one.

`omh sNN log` makes the sandbox's commits visible, `--turns` reads the
end-of-turn snapshots omh takes for an agent that commits nothing, `diff` grew
`-p` and checkpoint numbers, `--keep` takes the numbers `log` printed and is
repeatable from a recorded replay point, `sync` brings trunk in merged on the
host, `rm` refuses over work no branch has, and `omh s` became a dashboard —
state, staleness, the files two sessions are both changing, and the sandbox
repositories a removed session left behind. `omh s01` is that dashboard scoped
to one session.

**Success criterion:** a session's work can be read, curated and landed without
typing `git` and without knowing that a session is a worktree. **Met.**

What this cost is worth reading before proposing the next one: a third of those
pull requests were corrections to the ones before them, and the recurring defect
was not a wrong answer but a confident one — a question omh could not answer
rendering identically to an answer. [Git](git.md) records each, with the
measurement that settled it.

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
in [decisions](decisions.md).

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

Worth reading before the next surface change, in two parts.

**The guards did more of the work than the sweeps did**, and where a guard
could not see, the sweep shipped falsehoods. A rename applied by spelling
inverted every historical sentence it touched; needles that could not span a
line break hid four sites; and a synopsis block in `docs/commands.md` was the
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

## v2 — portability

Second adapter driven for real work, [`omh eject`](trust.md), full `omh import`
covering rules, skills, hooks and commands rather than MCP alone.

None of these were ever gated on evidence — an earlier version of this roadmap
put them behind a benchmark, which parked three cheap, useful deliverables
behind a research project.

## v3 — the unique capability

Marketplace plugin port: Claude Code plugins re-rendered for other harnesses.

This is the thing nothing else can do, and it makes curation nearly free —
inherit Anthropic's taste, port it everywhere. It sits at v3 rather than v1
because it is worth very little until the ports are verified, which needs v2's
portability work and v1's evidence.

## v4 — memory and graph, re-justified

Both are in the product on argument. The re-justification is a real review
against their measured cost and a stated criterion — not a p-value — and either
they earn their place or they go.

A distribution that cannot retire its own choices is just an opinion with a
changelog.

---

## What would change this order

- **The spike fails badly.** If `sbx` forces host-path mounts, the staging model
  needs rework rather than a tweak, and v0.5 grows.
- **The graph's cost grows faster than its usefulness.** Then v4 happens early
  and subtracts, which is the system working.
- **Someone actually uses this besides its author.** Real usage reorders roadmaps
  more reliably than reasoning does — every item on this list that has shipped
  was reordered at least once by running the tool.
