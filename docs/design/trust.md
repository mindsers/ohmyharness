# Trust

The standard complaint about oh-my-zsh is opacity: a slow shell nobody can
diagnose. *"Without the hassle of understanding"* curdles into *"unable to
understand."*

That is the failure mode omh is most exposed to, because it makes more decisions
on your behalf than oh-my-zsh ever did — and some of them are security
decisions. Four commands exist to prevent it. Two are built.

## `omh config` — provenance ✅

Every value says where it came from and what it beat.

```console
$ omh config
carry_in   [".env.local"]   ← local (overrides shared)
```

No competitor does this. It is what makes three layers debuggable instead of
mysterious, and it is cheap — the resolver already knows the answer; the only
work was refusing to throw it away.

See [Configuration](../configuration.md#provenance).

## `omh doctor` — verification ✅

Proves an adapter's claims against a real container. Covered in
[Troubleshooting](../troubleshooting.md).

Belongs in this list because it is the answer to *"how do I know omh actually
did what it said?"* — and because the answer is not "the tests pass."

## `omh why <thing>` — justification ⬜

Provenance extended from *where* to *why*.

```console
$ omh why codegraph
  in the base set since 2026.06
  cut tokens-to-first-correct-edit 41% across 12 tasks
  alternatives considered: CodeGraph, custom tree-sitter
  remove with: omh config mcp rm codegraph
```

Note the second line: it is a measurement, which means `why` cannot ship before
`bench`.

## `omh bench` — evidence ⬜

A fixed task suite measuring tokens-to-first-correct-edit with each component on
and off.

**This is the load-bearing one.** It is what makes "opinionated" mean something
other than "arbitrary"; it is how base-set entries are earned and retired; and
it is a claim no app store can make about its own catalog, because an app store
never decided anything to measure.

Until it exists, the base set is justified by argument. That is the
[honest weak spot](distribution.md#the-honest-weak-spot) in the whole thesis,
and it is why `bench` sits ahead of new features on the [roadmap](roadmap.md).

## `omh eject` — the exit ⬜

Write out the raw per-harness config and step aside.

For an opinionated tool, **a credible exit is what makes adoption safe.** You are
choosing to hand a tool your rules, credentials and sandbox policy; being able
to leave with all of it is the difference between a default and a cage.

Nearly free to build, since omh already generates exactly these files.

---

Together these make the opinion a **default, not a cage**. An app store cannot
be overridden, because it never decided anything in the first place — the
freedom it offers is the freedom to do the work yourself.
