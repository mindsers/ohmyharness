# The sandbox's repository becomes a remote

**Status: proposed, not built.** Written after #153. Nothing here has landed, and three things in it are [not decided](#not-decided). What ships is in [Sessions](https://docs.ohmyharness.sh/concepts/sessions/#the-agent-has-git-and-it-is-not-yours) and under [risk 2b](https://docs.ohmyharness.sh/concepts/risks/#security).

## Why the patches are not the answer

#153 closed four ways a host-side git ran something the sandbox had written: a
`commondir` file that sent git to a `config` of the agent's, a repository nested
in the worktree that `status`, `diff` and `add` each ran git inside, the same
through one of the user's own submodules, and a `checkout` omh itself printed.
Each was measured and each fix is held by a test that was red first.

The shape of that list is the problem. They are four doors found and closed,
in a room git says not to stand in. From git(1), under SECURITY, in the 2.56.0
that all of this was measured against:

> it is not safe to run Git commands in a .git directory (or the working tree
> that surrounds it) when that .git directory itself comes from an untrusted
> source. […] If you have an untrusted .git directory, you should first clone
> it with git clone --no-local to obtain a clean copy.

omh runs `status`, `add`, `commit`, `log`, `show`, `rev-list`, `merge-base`,
`symbolic-ref` and `config` in exactly such a directory, and what stands between
those and the agent is a denylist: eight `-c` keys, two environment variables,
flags added by verb, and a poisoned config value on `add`. A denylist is right
until the next indirection, and #153 found this one wrong in four places.

## The shape

One rule, which is git's own: **the only git the host runs against the
sandbox's repository is a fetch.** Everything else runs in a repository the
host owns.

| today, in the sandbox's gitdir | proposed |
|---|---|
| `log`, `show`, `rev-list`, `merge-base` — `omh sNN log`, `diff <n>`, the count `rm` refuses over, the range `--keep` replays | fetch into a **mirror**: a bare repository beside the seed record, outside the mount. Read the mirror |
| `symbolic-ref HEAD` — is it detached | `ls-remote --symref`, which is the same conversation a fetch starts |
| `status` — what is uncommitted | stage the worktree into an index the host owns, and compare with the fetched tip |
| `add` and `commit` around a sync — the checkpoint before it, `base moved to …` after | **run inside the sandbox**, by the sandbox's own git, where its config and hooks are at home |
| `add` and `commit` for the `Work in progress` sweep before a harvest | built in the mirror, from the host's index. It has to reach the branch, not the sandbox |
| `config`, rewritten on every launch | a pristine copy kept beside the seed, copied over the mounted one. No git |
| the harvest's fetch | unchanged. It is already this shape |

The session's worktree is the half this does not touch. It is already the
right shape — the repository and the index are the user's, and only the files
are the agent's — and what #153 added there stays: pointers left out of what
`add` may touch, `--ignore-submodules=dirty`, the refusal.

## Measured

2026-10-11, git 2.56.0. The sandbox's gitdir held everything #153 found and
more: a `commondir` pointing at a `config` naming a monitor, a pager, clean and
smudge filters, a textconv driver and `uploadpack.packObjectsHook`; seven hooks;
a replace ref; a tag called `main`; a `.gitattributes` in the tree applying the
filter and the driver to every `.txt`.

| the host does | ran on the host | cost |
|---|---|---|
| fetches `refs/heads/*` and the turn ref into the mirror | nothing | 35ms when nothing is new |
| `ls-remote --symref` for `HEAD` | nothing | names the branch, or only an id when detached |
| `log -p` and `show` in the mirror | nothing | — |
| stages a 400-file worktree into its own index, diffs against the fetched tip | nothing | the staging: 15ms cold, 8ms warm |
| *today's* `status`, in place, with none of #153 | the agent's monitor | — |

Three things stop needing a guard because they stop arriving. The fetch names
its refs, so no replace ref and no tag is in the mirror — measured, zero of
each — and a graft file has nothing to rewrite. The `config` git reads is the
mirror's. The index is the host's.

## What gets deleted

`NEUTRALISED`, the `GIT_COMMON_DIR` pin and the constructor that carries it,
`GUEST_ENV`, the backstop on `add`, and `stage`'s dance with the agent's index.
Not maintained — gone, with the comments that explain what each was measured
against. `write_config`'s read-modify-write goes with them.

What stays is what was never about the gitdir: `--no-textconv` and
`--no-ext-diff`, because a driver in the *user's* config is still a program
run over the agent's bytes; `out::untrusted` at every render; and the pointer
handling, because a worktree's files are the agent's wherever the repository
lives.

## What it costs

- **A commit omh makes for the agent needs a sandbox to make it in.** A sync
  requires the sandbox stopped, so its two commits would run in a short-lived
  container over the same mounts. That asks nothing new of the machine — a sync
  already refuses without a runtime it can question — and it costs a container
  start, twice. Not measured: how long that is.
- **The sweep before a harvest stops appearing in the sandbox's history.** It
  lands on the branch as it does now, but the agent's repository still shows
  those changes as uncommitted, and a later commit of them by the agent is
  replayed onto a branch that already has them. Not measured: whether that
  replays as empty every time.
- **A commit the agent rewound is no longer counted.** `unkept` reads the
  sandbox's reflog today, and a reflog does not travel — measured, the rewound
  commit is not in the mirror. The mirror's own reflog records every tip the
  host has seen instead. That is coarser, and it is also the first version of
  this record the agent cannot erase.
- **A fetch before every read.** 35ms a session, on every `omh s`.
- **The objects exist twice**, until a session is removed.
- **`upload-pack` still reads the agent's `config`.** Measured both ways: the
  `packObjectsHook` did not run, and `uploadpack.allowAnySHA1InWant` *was*
  honoured — a fetch asking for the rewound commit by id got it. git restricts
  what `upload-pack` will run, not what it will read, and its own page says the
  surface is large. So the fetch carries `fetch.fsckObjects`, and see the third
  open question.

## Order

Each step lands alone, red test first. The tests #153 added assert outcomes
— nothing the sandbox wrote runs on the host, no tag arrives — so they are the
regression suite for every step. The one that counts `--git-dir` is about the
mechanism instead, and is replaced by its successor: no git but a fetch names
the sandbox's gitdir.

1. **The mirror, and the reads.** The largest win and nothing a user can see:
   `log`, `diff <n>`, the `rm` count and the stranded-commit refusal read a
   repository the host owns.
2. **The host's index.** `uncommitted` stops being a `status` in the sandbox's
   gitdir.
3. **The config copy.** A launch runs no git in the gitdir at all.
4. **The agent's commits, made by the agent's git.** Blocked on the first open
   question.
5. **The deletion.** Only once nothing is left to protect.

## Not decided

- **The commits omh makes in the sandbox's repository.** Three answers. *Run
  them in the sandbox*: principled, and two container starts added to every
  sync. *Stop making them*: a sync delivers files and the note, and loses
  `git show HEAD` as the account of what moved — the reason the agent finds a
  commit there at all. *Write the objects and the ref by hand*: no runtime, and
  omh reimplementing the part of git it trusts least to get right.
- **Where the mirror lives.** A bare repository per session, or scratch refs in
  the user's own — which is where a harvest already fetches to. The second
  costs no disk and puts the agent's objects in the user's repository on every
  `omh s`, not only when they ask to land something.
- **Who runs `upload-pack`.** git's own advice for an untrusted repository is
  to serve it as an unprivileged user. omh has one: `--upload-pack` can name a
  command, and one that runs `git-upload-pack` *inside* the container leaves
  the host parsing a pack and reading nothing in the gitdir at all. Measured as
  far as the mechanism: a fetch served by a named command, for a path the host
  does not have. Not measured through a container. Stopped, it is the
  short-lived one again.
