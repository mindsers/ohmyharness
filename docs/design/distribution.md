# Why a distribution

The single most useful thing to understand about omh: it is a **distribution**.
Not a framework, not an abstraction layer, not a marketplace.

Judged as an abstraction layer it looks thin — most of what it does, something
else already does. That is the wrong axis. Debian didn't write the kernel.
Homebrew didn't write the software. oh-my-zsh didn't write zsh, and its genius
was never the plugin catalog — it was that installing it gave you a good shell
*immediately*.

**"It already exists" is the precondition for a distribution, not a refutation
of one.**

| Part | Who does it | omh's job |
|---|---|---|
| Isolation | `sbx`, Docker | choose one, wire it |
| Rules & skills portability | AGENTS.md, SKILL.md standards | place them correctly |
| Code graph | `codebase-memory-mcp` et al | pick one, index on init |
| Model routing | LiteLLM, OpenRouter | one env var |
| Curation | Claude marketplace, community | **subtract from it** |

## What omh is not

Anthropic's marketplace ships 200+ curated plugins into an ecosystem of
[23,600+ skills and 12,700+ MCP servers](https://claudemarketplaces.com/).

That is an **app store**, and an app store is structurally incapable of being
opinionated. The moment it picks winners, every excluded publisher becomes a
business problem.

| | App store | Distribution |
|---|---|---|
| Optimizes for | catalog size | working defaults |
| Success metric | items listed | **decisions removed** |
| Can it pick winners? | no | yes — that *is* the product |

23,600 skills is not an opinion. It is the **problem statement**. And it grows
every month, which means the case for a distribution strengthens over time
rather than eroding.

**The product is subtraction.** A dozen MCP servers, not 12,700. The metric is
decisions-to-productive, and the target is zero.

Corollary: *"we support everything"* is an anti-feature. Mechanisms may be
broad; the UX must not expose that breadth as choice.

## Competitive position

| | curated | isolated | harness-neutral |
|---|---|---|---|
| Anthropic marketplace | ✓✓ | ✗ | ✗ |
| [Docker `sbx`](https://docs.docker.com/reference/cli/sbx/) | ✗ | ✓✓ | ✓ |
| [Sculptor](https://nimbalyst.com/compare/sculptor/) | ✗ | ✓✓ | ~ |
| [Conductor](https://nimbalyst.com/blog/best-agent-management-tools-2026/), Nimbalyst | ✗ | ✓ | ~ |
| Plexus, agent-rules-sync | ✗ | ✗ | ✓ |
| **omh** | — | ✓ | ✓✓ |

**No tool is both curated and harness-neutral.** That is the gap.

It also sharpens the pitch. omh is not "a curated setup" — Anthropic will always
beat us at that on their own harness. omh is **your curated setup, anywhere.**
Isolation we don't build; curation we inherit and port; provenance so it stays
debuggable.

Sobering note: Vibe Kanban's company shut down in April 2026. This category has
already produced a casualty.

## The honest weak spot

The em-dash in that table is not a typo. **omh's curation column is currently
unearned** — the base set is one MCP server plus four hooks, justified by
argument rather than measurement.

The name promises curation. The verified, defensible half of the product today
is isolation.

There is no benchmark coming to fix that, and pretending otherwise would be its
own kind of dishonesty — see
[measure the cost, argue the benefit](trust.md#measure-the-cost-argue-the-benefit).
What closes the gap is the ordinary work of a distribution: more entries that
earn their place, each with stated criteria, a measured cost, and a one-line
way to remove it.

Stating this here rather than discovering it in an issue thread is the point of
writing it down.

## What follows from being a distribution

- **The shortlist must be earned.** Every base-set entry states what it costs
  (measured, in bytes and seconds), what it buys (argued, in a sentence), what
  was considered instead, and how to remove it. An entry that cannot fill in all
  four is taste pretending to be curation.
- **The shortlist expires.** A distribution's real work is re-choosing quarterly
  as the catalog churns. The base set is therefore versioned (`omh 2026.08`) and
  `omh upgrade` shows a changelog of what entered, what left, and why.
- **Curation is a recurring commitment**, which is the honest reason a solo
  distribution is hard. See [risks](risks.md).
- **The opinion must be escapable.** A distribution whose opinion cannot be
  overridden is not trustworthy — hence [`omh eject`](trust.md), and hence
  runtime backends staying plural even when one is clearly better.
