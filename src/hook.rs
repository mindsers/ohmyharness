//! The canonical hook format.
//!
//! `{ "event": "Stop", "matcher": "Edit|Write", "command": … }` is Claude
//! Code's vocabulary wearing a neutral-looking hat. Three separate things leak,
//! and they get harder in order: **event names** are Claude's words for moments
//! every harness has; **matchers** are Claude's *tool* names; and the **payload
//! protocol** — `jq -r '.tool_input.command'` in, `hookSpecificOutput` out — is
//! the one that matters, because translating every name in a hook file leaves a
//! body that still only works on Claude Code.
//!
//! All three are the same problem, so they get one answer: **omh has a closed
//! vocabulary, the adapter says how this harness spells each word, and the
//! translation happens when the hook is staged.** No part of it needs to happen
//! at runtime.
//!
//! Rejected: a runtime shim, with the harness calling `omh hook run <name>` and
//! omh normalizing the payload live. `omh` is already at `/usr/local/bin/omh`
//! in every sandbox so it would have worked, but it puts a process spawn in
//! front of every matching tool call — and `graph-read` matches `Read`, the most
//! frequent tool there is. Paying that per call, forever, to do work that can be
//! done once per launch is the wrong trade, and it would have moved a harness
//! difference out of adapter data and into omh's code.

use crate::adapter::Binding;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A moment every harness has, in omh's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    SessionStart,
    TurnEnd,
    BeforeTool,
    AfterTool,
}

/// A class of thing an agent does, in omh's words. A harness spells each one
/// however its own tools are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    Edit,
    Read,
    Shell,
    Search,
}

/// A piece of the tool call a hook can read.
///
/// `.tool_input.file_path` is Claude's spelling of a canonical field exactly as
/// `PreToolUse` is Claude's spelling of a canonical moment, which is what makes
/// the payload one mechanism with the other two rather than a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Field {
    ToolFile,
    ToolCommand,
}

impl Field {
    pub const ALL: [Field; 2] = [Self::ToolFile, Self::ToolCommand];

    /// The shell variable a hook body reads this field through. Binding the
    /// fields to `OMH_*` names is what lets a `when` predicate be written once
    /// and mean the same thing on every harness.
    pub fn var(&self) -> &'static str {
        match self {
            Self::ToolFile => "OMH_TOOL_FILE",
            Self::ToolCommand => "OMH_TOOL_COMMAND",
        }
    }
}

/// What `capture` binds its command's output to.
pub const CAPTURE_VAR: &str = "OMH_CAPTURE";

/// Variables the sandbox sets for every hook, on top of the payload fields.
///
/// Named here rather than where they are set, because this is the module that
/// has to decide whether a `$` in prose refers to something real.
/// `container::plan` puts exactly these on the container, and
/// `the_sandbox_sets_what_a_hook_may_name` holds the two lists together.
pub const SANDBOX_VARS: [&str; 2] = ["OMH_SESSION", "OMH_GRAPH_PROJECT"];

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SessionStart => "session-start",
            Self::TurnEnd => "turn-end",
            Self::BeforeTool => "before-tool",
            Self::AfterTool => "after-tool",
        })
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Edit => "edit",
            Self::Read => "read",
            Self::Shell => "shell",
            Self::Search => "search",
        })
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ToolFile => "tool-file",
            Self::ToolCommand => "tool-command",
        })
    }
}

/// What a hook does when its moment arrives. Exactly one of two things, which
/// is why it is an enum rather than two optional fields.
///
/// As four `Option`s the format could express three states it has no meaning
/// for — run *and* inject, neither, and capturing output that nothing reads —
/// so every one of them was a `bail!` in a validator, and `does()` needed a
/// fallback for a case the validator had already ruled out. Here they are not
/// states to reject; they cannot be written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Executes; output ignored.
    Run(String),
    /// Block the call and tell the model why.
    ///
    /// A separate variant from `Inject` because the difference is not in the
    /// text but in what happens to the tool call, and no renderer can guess it.
    /// On Claude Code an advisory `additionalContext` never blocks while a
    /// `permissionDecision: "deny"` does; on opencode the only text channel
    /// `tool.execute.before` has is a `throw`, which blocks whether you wanted
    /// it to or not. A format that could not say which it meant would turn
    /// `graph-first`'s nudge into a wall on the second harness — and a hook that
    /// blocks correct work gets disabled, which is the nudge's own argument for
    /// existing.
    ///
    /// No `capture`: a refusal is a fixed reason. `capture` exists because
    /// injected text may not be known until something has been asked, and
    /// nothing wants a computed refusal yet. Adding one later is additive.
    Refuse { text: String },
    /// Text into the agent's context, through whatever protocol this harness
    /// uses to accept one. **Advisory** — it never blocks the call.
    Inject {
        /// A command whose stdout binds to `$OMH_CAPTURE`, for the case `text`
        /// alone cannot express: something not known until it has been asked.
        /// Evaluated **before** `when`, so a predicate can test it.
        ///
        /// Inside this variant rather than beside it: capturing output for a
        /// `run` that discards it is the third meaningless state, and this is
        /// where it stops being expressible.
        capture: Option<String>,
        text: String,
    },
}

/// A hook declares what it wants, never how a harness spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "Raw")]
pub struct Hook {
    pub on: Event,
    pub stack: Option<String>,
    /// Empty means every tool this moment has — which is the only sensible
    /// reading for `turn-end`, where there is no tool to narrow to.
    pub tools: Vec<Tool>,
    /// A predicate. Non-zero means this hook stays silent, which is how a hook
    /// degrades to a no-op rather than to an error.
    pub when: Option<String>,
    pub action: Action,
}

/// The shape of a hook *file*, and the only shape that deserializes.
///
/// A separate struct rather than `#[serde(flatten)]` on `Hook`: flatten
/// **silently disables `deny_unknown_fields`**, and refusing `{"event":"Stop"}`
/// by naming `on` is the first thing this format has to do. A hook file
/// half-translated by hand, quietly accepted and never applied, is precisely
/// the failure the canonical format exists to prevent.
///
/// It is also what keeps `Hook` unconstructible without validation. There is no
/// `Deserialize` on `Hook` at all, so `from_raw` is the only door in, and
/// `whence` reaches the message — which `#[serde(try_from)]` could not have
/// done, since serde hands a conversion no idea which file it is reading.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    on: Event,
    /// Which ecosystem this hook belongs to, if any — a **reference** to a
    /// stack definition, never a copy of one. The marker that decides whether
    /// a repo is a rust project stays in `stacks/rust.toml`, so the two cannot
    /// disagree; a `marker` key here, or a `hooks = [...]` list there, would be
    /// the same fact in two places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stack: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refuse: Option<String>,
}

impl From<Hook> for Raw {
    fn from(h: Hook) -> Self {
        let (capture, run, inject, refuse) = match h.action {
            Action::Run(cmd) => (None, Some(cmd), None, None),
            Action::Inject { capture, text } => (capture, None, Some(text), None),
            Action::Refuse { text } => (None, None, None, Some(text)),
        };
        Self {
            on: h.on,
            stack: h.stack,
            tools: h.tools,
            when: h.when,
            capture,
            run,
            inject,
            refuse,
        }
    }
}

impl Hook {
    /// Parse and validate together, so no caller can hold an unchecked hook.
    /// Validating where the value is minted is the rule `memory::expand_key`
    /// and `carry::validate_pattern` already follow.
    pub fn parse(raw: &str, whence: &str) -> Result<Self> {
        let raw: Raw =
            serde_json::from_str(raw).with_context(|| format!("parsing hook {whence}"))?;
        Self::from_raw(raw, whence)
    }

    /// The wire shape, triaged into the canonical one.
    ///
    /// Two of the three checks this used to make are gone — not moved,
    /// *deleted*: `Action` cannot hold a run and an inject, and cannot hold a
    /// capture without one. What is left is the check on a **value** rather
    /// than a shape, and no type can take that one away.
    fn from_raw(raw: Raw, whence: &str) -> Result<Self> {
        let action = match (raw.run, raw.inject, raw.refuse) {
            (None, None, None) => {
                anyhow::bail!("{whence}: a hook does nothing without `run`, `inject` or `refuse`")
            }
            // Three fields, one of which. Spelled as a catch-all rather than
            // three pairs, because the message is the same whichever two were
            // written and enumerating them would only invite a fourth arm.
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
                anyhow::bail!(
                    "{whence}: a hook does one thing — `run` a command, `inject` advisory \
                     text, or `refuse` the call. A command whose output should reach the \
                     agent is `capture` plus `inject`."
                )
            }
            (None, None, Some(text)) => {
                // A refusal only means something before the call it refuses.
                // After the tool has run there is nothing left to block, and at
                // a moment with no call there was never anything — but both
                // rendered a plausible-looking payload that the harness then
                // ignored. Checked where the value is minted, the rule
                // `capture`-needs-`inject` below already follows.
                if raw.on != Event::BeforeTool {
                    anyhow::bail!(
                        "{whence}: `refuse` blocks a call, so it belongs to `before-tool` — \
                         `{}` is too late or has no call to block. To say something without \
                         stopping the call, use `inject`.",
                        raw.on
                    );
                }
                if raw.capture.is_some() {
                    anyhow::bail!(
                        "{whence}: `capture` collects output for `inject` to carry. \
                         A refusal is a fixed reason, so capturing output says nothing."
                    );
                }
                non_empty(&text, "refuse", whence)?;
                // A refusal reaches a shell exactly as an injection does, so a
                // stray `$` arrives as a hole in the sentence either way.
                check_interpolation(&text, whence, false)?;
                Action::Refuse { text }
            }
            (Some(run), None, None) => {
                if raw.capture.is_some() {
                    anyhow::bail!(
                        "{whence}: `capture` collects output for `inject` to carry. \
                         With `run` the output is ignored, so capturing it says nothing."
                    );
                }
                non_empty(&run, "run", whence)?;
                Action::Run(run)
            }
            (None, Some(text), None) => {
                non_empty(&text, "inject", whence)?;
                check_interpolation(&text, whence, raw.capture.is_some())?;
                if let Some(capture) = &raw.capture {
                    non_empty(capture, "capture", whence)?;
                    // A capture nothing reads is a subprocess per session for
                    // nothing. `when` counts as a reader as well as `text`,
                    // because `capture` is evaluated *before* the predicate on
                    // purpose so a predicate can test it — which is exactly what
                    // `graph-orient` does.
                    let read = mentions(&text, CAPTURE_VAR)
                        || raw
                            .when
                            .as_deref()
                            .is_some_and(|w| mentions(w, CAPTURE_VAR));
                    if !read {
                        anyhow::bail!(
                            "{whence}: `capture` runs a command and binds its output to \
                             ${CAPTURE_VAR}, and nothing here reads it. Name it in \
                             `inject` or in `when`, or drop the `capture`."
                        );
                    }
                }
                Action::Inject {
                    capture: raw.capture,
                    text,
                }
            }
        };
        if let Some(when) = &raw.when {
            // An empty predicate renders `|| exit 0; …`, which `sh` refuses to
            // parse — so the hook never runs while every assertion about its
            // text still passes.
            non_empty(when, "when", whence)?;
        }
        Ok(Self {
            on: raw.on,
            stack: raw.stack,
            tools: raw.tools,
            when: raw.when,
            action,
        })
    }

    /// What this hook does, in one string, for the commands that report a hook
    /// rather than run one.
    pub fn does(&self) -> &str {
        match &self.action {
            Action::Run(cmd) => cmd,
            Action::Inject { text, .. } | Action::Refuse { text } => text,
        }
    }

    /// Every shell command this hook would execute.
    ///
    /// Not [`Self::does`], which answers "what is this hook *for*" and returns
    /// injected prose for the variants that inject. This answers "what will be
    /// handed to a shell", which is a different question and the only one worth
    /// asking about a missing program: a `refuse` runs nothing and can never be
    /// blocked by a toolchain, while an `inject`'s `capture` shells out exactly
    /// as a `run` does and is just as unable to.
    pub fn runs(&self) -> Vec<&str> {
        match &self.action {
            Action::Run(cmd) => vec![cmd.as_str()],
            Action::Inject { capture, .. } => capture.as_deref().into_iter().collect(),
            Action::Refuse { .. } => Vec::new(),
        }
    }

    /// The payload fields this hook actually reads, derived from the `$OMH_*`
    /// names its bodies mention.
    ///
    /// Derived rather than declared so there is nothing to keep in sync: a
    /// `when` that stops testing the filename stops paying for it in the same
    /// edit. `graph-first` reads no payload at all and must not be charged a
    /// `jq` on every search.
    pub fn fields(&self) -> BTreeSet<Field> {
        let (capture, body) = match &self.action {
            Action::Run(cmd) | Action::Refuse { text: cmd } => (&None, cmd),
            Action::Inject { capture, text } => (capture, text),
        };
        let bodies = [
            self.when.as_deref(),
            capture.as_deref(),
            Some(body.as_str()),
        ];
        Field::ALL
            .into_iter()
            .filter(|f| bodies.iter().flatten().any(|b| mentions(b, f.var())))
            .collect()
    }
}

/// A body has to say something. An empty one is a field somebody meant to fill
/// in, and every one of them fails in the way this format exists to prevent: an
/// empty `inject` renders a hook that hands the agent nothing, an empty `run`
/// renders a hook that runs nothing, and an empty `when` renders a command `sh`
/// will not parse — each satisfying every assertion about the hook's text.
fn non_empty(body: &str, field: &str, whence: &str) -> Result<()> {
    if body.trim().is_empty() {
        anyhow::bail!("{whence}: `{field}` is empty, so this hook does nothing");
    }
    Ok(())
}

/// Does `body` reference `$var`? Checked with the following character so
/// `$OMH_TOOL_FILE_BACKUP` does not count as `$OMH_TOOL_FILE` — and `${VAR}`
/// counts, because that is the same reference written defensively.
fn mentions(body: &str, var: &str) -> bool {
    let boundary = |rest: &str| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    body.match_indices(var).any(|(i, _)| {
        let before = &body[..i];
        let rest = &body[i + var.len()..];
        // The braced form takes the same boundary test rather than requiring
        // `}` immediately: `${OMH_TOOL_FILE:-none}` is how anybody writes a
        // defensive reference, and reading it as *not* a reference skips the
        // preamble, leaves the variable unset, and makes the hook take its
        // default on every call — silently, with its text still correct.
        (before.ends_with('$') || before.ends_with("${")) && boundary(rest)
    })
}

/// `inject` is prose that reaches a shell, which is a combination that fails
/// quietly: a stray `$` expands to nothing and the sentence arrives with a hole
/// in it. So every `$` has to look like a reference, `$$` is how you write a
/// literal one, and command substitution is refused by name — that is what
/// `capture` is for, and running a command from inside a sentence is how a hook
/// body learns to be shell again.
fn check_interpolation(text: &str, whence: &str, capture: bool) -> Result<()> {
    let known: Vec<&str> = Field::ALL
        .iter()
        .map(|f| f.var())
        .chain(SANDBOX_VARS)
        .chain(capture.then_some(CAPTURE_VAR))
        .collect();

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            i += 1;
            continue;
        }
        // `$$` is the literal dollar, resolved by `interpolating`.
        if chars.get(i + 1) == Some(&'$') {
            i += 2;
            continue;
        }
        if chars.get(i + 1) == Some(&'(') {
            anyhow::bail!(
                "{whence}: `$(` in `inject` runs a command from inside a sentence. \
                 Use `capture` and interpolate ${CAPTURE_VAR}."
            );
        }

        // `${NAME}` and `${NAME:-default}` alike: the name runs to the first
        // character that cannot be part of one, and what follows has to be a
        // brace or an expansion operator. Asking only whether a `}` appears
        // *somewhere later* accepted `${ high } today`, which is a bad
        // substitution — a runtime failure, so `sh -n` sees nothing wrong and
        // the hook silently emits nothing.
        let braced = chars.get(i + 1) == Some(&'{');
        let start = i + if braced { 2 } else { 1 };
        let end = chars[start..]
            .iter()
            .position(|c| !c.is_ascii_alphanumeric() && *c != '_')
            .map_or(chars.len(), |n| start + n);
        let name: String = chars[start..end].iter().collect();

        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            anyhow::bail!(
                "{whence}: `{}` is not a variable name. A shell reads that as a bad \
                 substitution and the hook emits nothing at all.",
                chars[i..(end + 1).min(chars.len())]
                    .iter()
                    .collect::<String>()
            );
        }
        if braced
            && !matches!(
                chars.get(end),
                Some('}' | ':' | '-' | '+' | '?' | '#' | '%')
            )
        {
            anyhow::bail!(
                "{whence}: `${{{name}` is never closed with `}}`. A shell reads that as \
                 a bad substitution and the hook emits nothing at all."
            );
        }
        if !known.contains(&name.as_str()) {
            anyhow::bail!(
                "{whence}: `${name}` is not something omh sets, so it expands to \
                 whatever the sandbox happens to hold — or to nothing. Available: {}.",
                known.join(", ")
            );
        }
        i = end;
    }
    Ok(())
}

/// A hook translated into one harness's words, or the reason it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rendered(Rendered),
    /// Not an error: a harness that cannot express a moment is the same
    /// "absent key is graceful degradation" rule the capability map already
    /// uses, one level down. But the whole capability no longer goes with it,
    /// so the report has to name the hook and what it asked for.
    Dropped(Dropped),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub event: String,
    pub matcher: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub name: String,
    /// What this harness could not spell, in omh's words.
    pub wanted: String,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (no {})", self.name, self.wanted)
    }
}

/// What a harness's own vocabulary makes of one hook — or what it could not say.
///
/// The lookups are the same three every renderer needs (a moment, the tools, the
/// payload fields a body mentions) and the drop *reasons* have to be identical
/// whichever renderer asked, or two harnesses would report the same missing map
/// two different ways. `render` and the plugin emitter both go through here.
pub struct Wired<'a> {
    pub event: &'a str,
    /// This harness's names for the tools the hook narrows to. Empty means
    /// every tool, which is the only sensible reading for a moment with none.
    pub tools: Vec<&'a str>,
    /// Where this harness keeps each field the hook actually reads.
    pub fields: Vec<(Field, &'a str)>,
}

/// Resolve a hook against one harness's vocabulary.
///
/// The three lookups every renderer needs, in one place so the drop *reasons*
/// are identical whichever renderer asked. Two harnesses reporting the same
/// missing map two different ways is the drift this exists to prevent, and it
/// is the reason this is a function rather than three inline `get`s.
pub fn wire<'a>(
    name: &str,
    hook: &Hook,
    binding: &'a Binding,
    tools: &'a BTreeMap<Tool, String>,
) -> std::result::Result<Wired<'a>, Dropped> {
    let drop = |wanted: String| Dropped {
        name: name.to_string(),
        wanted,
    };
    let event = binding
        .events
        .get(&hook.on)
        .ok_or_else(|| drop(format!("`{}` moment", hook.on)))?;
    let mut named = Vec::new();
    for tool in &hook.tools {
        named.push(
            tools
                .get(tool)
                .map(String::as_str)
                .ok_or_else(|| drop(format!("`{tool}` tool")))?,
        );
    }
    let mut fields = Vec::new();
    for field in hook.fields() {
        let at = binding
            .fields
            .get(&field)
            .map(String::as_str)
            .ok_or_else(|| drop(format!("`{field}` field")))?;
        fields.push((field, at));
    }
    Ok(Wired {
        event,
        tools: named,
        fields,
    })
}

/// The same vocabulary, read the other way: this harness's words back into
/// omh's.
///
/// Directly beneath [`wire`] on purpose — they are one mechanism pointed in two
/// directions, and a translation that only went one way is how a format ends up
/// exportable and un-importable. `render::parse` already sets the precedent for
/// MCP: *every format that renders must also parse, and the pair must round
/// trip*.
///
/// **Ambiguity is refused, never resolved by map order.** An adapter mapping
/// two omh moments onto one harness word — say both `turn-end` and
/// `session-start` to `Stop` — can be rendered from (each hook goes to `Stop`)
/// and cannot be read back: a `Stop` entry is either, and a `BTreeMap` would
/// silently answer whichever sorted first. That would import somebody's hooks
/// with half of them at the wrong moment, which looks exactly like working.
#[derive(Debug)]
pub struct Vocabulary {
    events: BTreeMap<String, Event>,
    tools: BTreeMap<String, Tool>,
}

impl Vocabulary {
    /// Invert one harness's tables, refusing any word that means two things.
    pub fn of(binding: &Binding, tools: &BTreeMap<Tool, String>) -> Result<Self> {
        let mut events = BTreeMap::new();
        for (ours, theirs) in &binding.events {
            if let Some(clash) = events.insert(theirs.clone(), *ours) {
                anyhow::bail!(
                    "this harness spells both `{clash}` and `{ours}` as `{theirs}`, \
                     so a `{theirs}` hook cannot be read back as either"
                );
            }
        }
        let mut inverted = BTreeMap::new();
        for (ours, theirs) in tools {
            if let Some(clash) = inverted.insert(theirs.clone(), *ours) {
                anyhow::bail!(
                    "this harness spells both `{clash}` and `{ours}` as `{theirs}`, \
                     so a `{theirs}` matcher cannot be read back as either"
                );
            }
        }
        Ok(Self {
            events,
            tools: inverted,
        })
    }

    /// Which omh moment this harness's word names, if any.
    pub fn event(&self, theirs: &str) -> Option<Event> {
        self.events.get(theirs).copied()
    }

    /// Every tool spelling this harness declares, with what it means.
    ///
    /// Exposed because a spelling may itself be an alternation — Claude writes
    /// `edit` as `Edit|Write|MultiEdit` — so reading a matcher back means
    /// matching whole spellings against it, not splitting it on `|` and looking
    /// each piece up.
    pub fn spellings(&self) -> impl Iterator<Item = (&str, Tool)> {
        self.tools.iter().map(|(word, tool)| (word.as_str(), *tool))
    }
}

/// Translate one hook into the shape this harness parses.
pub fn render(
    name: &str,
    hook: &Hook,
    binding: &Binding,
    tools: &BTreeMap<Tool, String>,
) -> Result<Outcome> {
    let dropped = |wanted: String| {
        Ok(Outcome::Dropped(Dropped {
            name: name.to_string(),
            wanted,
        }))
    };

    let Wired {
        event,
        tools: matchers,
        fields,
    } = match wire(name, hook, binding, tools) {
        Ok(wired) => wired,
        Err(d) => return Ok(Outcome::Dropped(d)),
    };

    let mut command = String::new();

    // One `cat`, however many fields. stdin is consumable once, so a second
    // bare `jq` would read an empty payload and the field would silently be
    // blank — the shape of bug that satisfies every assertion about a hook's
    // text while the hook does nothing.
    if !fields.is_empty() {
        command.push_str("p=$(cat); ");
        for (field, expr) in &fields {
            command.push_str(&format!(
                "{}=$(printf '%s' \"$p\" | jq -r '{expr} // empty'); ",
                field.var()
            ));
        }
    }

    // Before `when`, so a predicate can test what was captured.
    if let Action::Inject {
        capture: Some(capture),
        ..
    } = &hook.action
    {
        command.push_str(&format!("{CAPTURE_VAR}=$({capture}); "));
    }
    if let Some(when) = &hook.when {
        command.push_str(&format!("{when} || exit 0; "));
    }

    match &hook.action {
        Action::Run(run) => command.push_str(run),
        // Advisory and blocking are two protocols, so they are two templates and
        // either may be absent. A harness that can advise but not block drops a
        // refusal by name rather than downgrading it to a notice the agent is
        // free to ignore — silently turning a wall into a nudge is the mirror of
        // the mistake this whole distinction exists to stop.
        // Which protocol a hook's text travels by is decided once, on the
        // binding, rather than matched again here — see `Binding::protocol`.
        Action::Inject { text, .. } | Action::Refuse { text } => {
            let template = match binding.protocol(&hook.action) {
                Ok(Some(t)) => t,
                Ok(None) => unreachable!("a run does not reach this arm"),
                Err(wanted) => return dropped(wanted.into()),
            };
            command.push_str(&fill(&template.template, text, event));
        }
    }

    Ok(Outcome::Rendered(Rendered {
        event: event.to_string(),
        matcher: matchers.join("|"),
        command,
    }))
}

/// Put a hook's text and this harness's word for the moment into a template.
fn fill(template: &str, text: &str, event: &str) -> String {
    template
        .replace("{{text}}", &interpolating(text))
        .replace("{{event}}", event)
}

/// A double-quoted shell word: `$` stays live so a hook can name what it is
/// talking about, everything that could end the word or start a command does
/// not. `$$` is the literal dollar, resolved here rather than left for the
/// shell, which reads `$$` as its own process id.
pub fn interpolating(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                out.push_str("\\$");
            }
            '\\' | '"' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{Adapter, Capability};
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    fn claude() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    /// A binding built from TOML rather than a struct literal: an adapter always
    /// arrives as a file, and a hand-built one can be right in exactly the way
    /// the code is wrong.
    fn binding(toml: &str) -> Binding {
        toml::from_str(toml).unwrap()
    }

    fn rendered(name: &str, hook: &Hook, b: &Binding) -> Rendered {
        match render(name, hook, b, &shipped().tools).unwrap() {
            Outcome::Rendered(r) => r,
            Outcome::Dropped(d) => panic!("unexpectedly dropped: {d}"),
        }
    }

    fn dropped(name: &str, hook: &Hook, b: &Binding) -> Dropped {
        match render(name, hook, b, &shipped().tools).unwrap() {
            Outcome::Rendered(r) => panic!("unexpectedly rendered: {r:?}"),
            Outcome::Dropped(d) => d,
        }
    }

    // ── reading a harness's words back ──────────────────────────────────────

    /// Every word `wire` can emit, `Vocabulary` can read back — which is what
    /// makes the pair a translation rather than an export.
    ///
    /// Asserted over the **shipped** adapters and over omh's whole vocabulary,
    /// not a fixture: an adapter that gained a moment and forgot to be
    /// invertible would otherwise be found by somebody trying to import.
    #[test]
    fn every_word_a_harness_renders_can_be_read_back() {
        for name in ["claude", "opencode"] {
            let adapter = crate::adapter::Adapter::find(Path::new(ADAPTERS), name).unwrap();
            let Some(binding) = adapter.supports(Capability::Hooks) else {
                continue;
            };
            let vocab =
                Vocabulary::of(binding, &adapter.tools).unwrap_or_else(|e| panic!("{name}: {e:#}"));

            for (ours, theirs) in &binding.events {
                assert_eq!(
                    vocab.event(theirs),
                    Some(*ours),
                    "{name}: renders `{ours}` as `{theirs}` and cannot read it back"
                );
            }
            let spellings: BTreeMap<&str, Tool> = vocab.spellings().collect();
            for (ours, theirs) in &adapter.tools {
                assert_eq!(
                    spellings.get(theirs.as_str()),
                    Some(ours),
                    "{name}: renders `{ours}` as `{theirs}` and cannot read it back"
                );
            }
        }
    }

    /// **A word that means two things is refused**, rather than resolved by
    /// whichever sorted first.
    ///
    /// An adapter mapping two omh moments onto one harness word renders fine —
    /// every hook goes to the same place — and cannot be read back at all. A
    /// `BTreeMap` would answer with whichever key sorted first, so half of
    /// somebody's imported hooks would land at the wrong moment, and the import
    /// would report success. Refusing costs an error message; guessing costs
    /// trust in every hook omh imported.
    #[test]
    fn a_harness_word_that_means_two_things_is_refused() {
        let ambiguous = binding(
            "path = \"x\"\nrender = \"claude-settings\"\n\n\
             [events]\nturn-end = \"Stop\"\nsession-start = \"Stop\"\n",
        );
        let err = format!(
            "{:#}",
            Vocabulary::of(&ambiguous, &BTreeMap::new())
                .expect_err("`Stop` cannot be read back as either moment")
        );
        assert!(err.contains("Stop"), "must name the word: {err}");
        assert!(
            err.contains("turn-end") && err.contains("session-start"),
            "and both things it means: {err}"
        );

        // The same for tools, which travel in the matcher rather than the key.
        let tools = BTreeMap::from([
            (Tool::Edit, "Write".to_string()),
            (Tool::Read, "Write".into()),
        ]);
        let err = format!(
            "{:#}",
            Vocabulary::of(
                &binding("path = \"x\"\nrender = \"claude-settings\"\n"),
                &tools
            )
            .expect_err("`Write` cannot be read back as either tool")
        );
        assert!(err.contains("Write"), "got: {err}");
    }

    // ── naming a stack ──────────────────────────────────────────────────────

    /// A hook may say which ecosystem it belongs to, and that is all it may say
    /// about it.
    ///
    /// A **reference**, not a copy. The marker that decides whether a repo is a
    /// rust project lives in `stacks/rust.toml` and nowhere else, so a hook
    /// naming `rust` cannot disagree with it — where a `marker` key here, or a
    /// `hooks = [...]` list in the stack file, would be the same fact in two
    /// places, free to drift. That coupling is the thing this step exists to
    /// break, so it must not be reintroduced in the other direction.
    ///
    /// Optional, and the absence is the common case: a hook that works
    /// anywhere — `graph-refresh`, somebody's `shellcheck` — belongs to no
    /// ecosystem and must not be filtered by one.
    #[test]
    fn a_hook_may_name_the_stack_it_belongs_to() {
        let h = Hook::parse(
            r#"{"on":"turn-end","stack":"rust","run":"cargo test"}"#,
            "rust-test.json",
        )
        .expect("a hook may name its stack");
        assert_eq!(h.stack.as_deref(), Some("rust"));

        let anywhere = Hook::parse(r#"{"on":"turn-end","run":"echo hi"}"#, "greet.json").unwrap();
        assert_eq!(
            anywhere.stack, None,
            "a hook that works anywhere belongs to no ecosystem"
        );
    }

    /// It survives the round trip, because `init` writes hook files and
    /// `omh import` will.
    ///
    /// `Hook` serialises through `Raw`, so a field the writer drops is a field
    /// that silently un-names a hook's stack the first time omh rewrites it —
    /// and the drift report would then stop recognising a hook it wrote itself.
    #[test]
    fn the_stack_a_hook_names_survives_being_written_back() {
        let h = Hook::parse(
            r#"{"on":"turn-end","stack":"go","run":"go test ./..."}"#,
            "go-test.json",
        )
        .unwrap();
        let written = serde_json::to_string(&h).unwrap();
        assert!(
            written.contains("\"stack\":\"go\""),
            "the stack has to reach the file: {written}"
        );
        assert_eq!(Hook::parse(&written, "again").unwrap(), h);

        // And a hook that names none writes no key, rather than a null that
        // every reader would then have to know means the same thing.
        let anywhere = Hook::parse(r#"{"on":"turn-end","run":"echo hi"}"#, "greet.json").unwrap();
        assert!(
            !serde_json::to_string(&anywhere).unwrap().contains("stack"),
            "an absent stack is an absent key"
        );
    }

    // ── the format ──────────────────────────────────────────────────────────

    /// The whole reason the format exists. `event` and `matcher` are one
    /// harness's words, and a file omh presents as its own cannot be written in
    /// them — otherwise every hook anybody writes is a hook that works on
    /// exactly one harness, and nothing says so.
    #[test]
    fn a_hook_declares_a_moment_not_a_harness_event() {
        let err = Hook::parse(r#"{"event":"Stop","command":"cargo test"}"#, "h.json")
            .expect_err("Claude's vocabulary is not omh's");
        let err = format!("{err:#}");
        assert!(err.contains("h.json"), "must name the file: {err}");
        assert!(err.contains("`on`"), "and the field to use: {err}");
    }

    /// A hook that is otherwise perfectly good, plus one word from somewhere
    /// else. The case above cannot prove this: it has no `on` at all, so it
    /// fails on the missing field whether unknown ones are refused or not.
    ///
    /// The guard exists for the shape of the type rather than for the user.
    /// `#[serde(flatten)]` — the obvious way to spell a hook whose action is an
    /// enum — **silently disables `deny_unknown_fields`**, and the failure it
    /// buys is exactly the one this format was written to stop: a `matcher`
    /// beside a canonical hook is a hook half-translated by hand, accepted, and
    /// never applied.
    #[test]
    fn a_word_from_another_harness_is_refused_beside_good_ones() {
        let err = Hook::parse(
            r#"{"on":"before-tool","matcher":"Read","run":"cargo test"}"#,
            "h.json",
        )
        .expect_err("`matcher` is Claude's word and omh does not read it");
        assert!(format!("{err:#}").contains("matcher"), "by name: {err:#}");
    }

    /// The file is the contract, so a hook has to serialise back into one.
    ///
    /// Load-bearing beyond tidiness: `omhs_own_hooks_obey_the_format_they_impose`
    /// round-trips the five omh ships through the real parser, and it can only
    /// do that if the canonical type writes the wire shape rather than its own
    /// internal one.
    #[test]
    fn a_hook_serialises_back_into_the_file_it_came_from() {
        for body in [
            r#"{"on":"turn-end","run":"cargo test"}"#,
            r#"{"on":"before-tool","tools":["read"],"when":"[ -f \"$OMH_TOOL_FILE\" ]","inject":"about $OMH_TOOL_FILE"}"#,
            r#"{"on":"session-start","capture":"date","inject":"it is $OMH_CAPTURE"}"#,
        ] {
            let hook = Hook::parse(body, "h.json").unwrap();
            let written = serde_json::to_string(&hook).unwrap();
            assert_eq!(
                Hook::parse(&written, "h.json").unwrap(),
                hook,
                "{body} did not survive: {written}"
            );
        }
    }

    #[test]
    fn a_hook_that_neither_runs_nor_injects_is_refused() {
        let err = Hook::parse(r#"{"on":"turn-end"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("run"), "got: {err}");
    }

    /// `run` ignores output and `inject` is text, so a hook asking for both is
    /// asking for something with no meaning — most likely it wanted a command
    /// whose output reaches the agent, which is `capture` plus `inject`.
    #[test]
    fn a_hook_that_both_runs_and_injects_is_refused_with_what_it_meant() {
        let err = Hook::parse(r#"{"on":"turn-end","run":"x","inject":"y"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    /// `inject` advises; `refuse` blocks. A hook has to say which it means,
    /// and it cannot mean both.
    ///
    /// The distinction is not decoration: on Claude Code `before-tool` + inject
    /// is advisory and never blocks, while the only text channel opencode's
    /// `tool.execute.before` has is a `throw`, which does. Translating a nudge
    /// into a wall breaks `graph-first`'s own rule — "a nudge, not a wall: a
    /// hook that blocks correct work gets disabled" — so the *format* has to
    /// carry the difference rather than each renderer guessing.
    #[test]
    fn a_hook_refuses_or_injects_but_not_both() {
        for body in [
            r#"{"on":"before-tool","refuse":"no","inject":"maybe"}"#,
            r#"{"on":"before-tool","refuse":"no","run":"x"}"#,
        ] {
            let err = Hook::parse(body, "h.json").expect_err("a hook does one thing");
            assert!(err.to_string().contains("h.json"), "name the file: {err}");
        }

        let refusal = Hook::parse(
            r#"{"on":"before-tool","tools":["shell"],"refuse":"git does not work here"}"#,
            "h.json",
        )
        .expect("a refusal on its own is a hook");
        assert_eq!(refusal.does(), "git does not work here");
    }

    /// A refusal only means something before the call it refuses.
    ///
    /// Nothing checked the moment, so `{"on":"turn-end","refuse":"stop"}` parsed
    /// and rendered on both harnesses — as `permissionDecision: "deny"` under
    /// `Stop`, a key Claude Code honours only for `PreToolUse`, and as a `throw`
    /// in a handler with no call in it. Installed, reported, doctor-green, and
    /// blocking nothing.
    ///
    /// Refused where the value is minted, the rule `capture`-needs-`inject`
    /// already follows: `after-tool` is the interesting one, because the tool
    /// has already run by then and "block it" has no meaning left.
    #[test]
    fn a_refusal_belongs_to_the_moment_before_the_call() {
        for moment in ["turn-end", "session-start", "after-tool"] {
            let err = Hook::parse(&format!(r#"{{"on":"{moment}","refuse":"no"}}"#), "h.json")
                .expect_err("a refusal after the fact refuses nothing");
            let err = format!("{err:#}");
            assert!(err.contains("before-tool"), "name the moment: {err}");
        }
        // And the one moment where it does mean something still parses.
        Hook::parse(r#"{"on":"before-tool","refuse":"no"}"#, "h.json").unwrap();
    }

    /// A refusal reaches a shell exactly as an injection does, so the `$` rule
    /// is the same one — prose with a stray dollar arrives with a hole in it.
    #[test]
    fn a_dollar_in_a_refusal_is_checked_like_one_in_an_inject() {
        let err = Hook::parse(
            r#"{"on":"before-tool","refuse":"costs $5 a month"}"#,
            "h.json",
        )
        .expect_err("prose that expands to nothing");
        assert!(
            err.to_string().contains("not a variable name"),
            "got: {err}"
        );
    }

    /// A capture whose output nothing reads is a subprocess per session, for
    /// nothing — and the module doc named it as one of the three meaningless
    /// states and claimed it "cannot be written down". Moving `capture` inside
    /// `Inject` killed only capture-beside-`run`; this half is a check on a
    /// *value*, which no shape can make unconstructible.
    ///
    /// Both bodies count, because `capture` is evaluated before `when` on
    /// purpose so a predicate can test it — `graph-orient` reads it in both.
    #[test]
    fn a_capture_nothing_reads_is_refused() {
        let err = Hook::parse(
            r#"{"on":"session-start","capture":"date","inject":"hello"}"#,
            "h.json",
        )
        .expect_err("nothing reads $OMH_CAPTURE here");
        assert!(
            err.to_string().contains(CAPTURE_VAR),
            "say which variable went unread: {err}"
        );

        // Read by the text, and read by the predicate: both are uses.
        for body in [
            r#"{"on":"session-start","capture":"date","inject":"it is $OMH_CAPTURE"}"#,
            r#"{"on":"session-start","capture":"date","when":"[ -n \"$OMH_CAPTURE\" ]","inject":"hi"}"#,
        ] {
            Hook::parse(body, "h.json").unwrap_or_else(|e| panic!("{body} should validate: {e:#}"));
        }
    }

    /// An empty predicate renders `|| exit 0; …`, which `sh` refuses to parse —
    /// so the hook never runs, and every assertion about its text still passes.
    /// CONTRIBUTING states the invariant it breaks: every hook command parses
    /// **and runs** under `sh`.
    #[test]
    fn an_empty_body_is_not_a_body() {
        for body in [
            r#"{"on":"turn-end","when":"","run":"cargo test"}"#,
            r#"{"on":"turn-end","run":""}"#,
            r#"{"on":"turn-end","inject":""}"#,
        ] {
            let err = Hook::parse(body, "h.json")
                .map(|_| ())
                .expect_err("an empty body says nothing and can break the shell");
            assert!(err.to_string().contains("h.json"), "name the file: {err}");
        }
    }

    #[test]
    fn capture_without_inject_is_refused() {
        let err =
            Hook::parse(r#"{"on":"turn-end","capture":"date","run":"x"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    /// `inject` is prose that reaches a shell, and that combination fails
    /// quietly: the sentence arrives with a hole where the `$` was, and every
    /// assertion about the hook's text still passes.
    #[test]
    fn a_dollar_that_names_no_variable_is_refused() {
        let err =
            Hook::parse(r#"{"on":"turn-end","inject":"costs $5 a month"}"#, "h.json").unwrap_err();
        assert!(
            err.to_string().contains("not a variable name"),
            "must say what is wrong with it: {err}"
        );
    }

    #[test]
    fn a_doubled_dollar_is_a_literal_one() {
        let h = Hook::parse(r#"{"on":"turn-end","inject":"costs $$5"}"#, "h.json").unwrap();
        assert_eq!(interpolating(h.does()), r#""costs \$5""#);
    }

    /// Running a command from inside a sentence is how a hook body learns to be
    /// shell again, which is the thing the format exists to undo.
    #[test]
    fn command_substitution_in_prose_points_at_capture() {
        let err = Hook::parse(r#"{"on":"turn-end","inject":"now $(date)"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    /// Anything that validates has to *run*, not merely parse.
    ///
    /// This used `sh -n`, which is a parse check — and the failure that matters
    /// here, a bad substitution, is a runtime diagnostic `sh -n` returns 0 for.
    /// So it asserted half the invariant CONTRIBUTING states ("every hook
    /// command parses **and runs** under `sh`") while reading as all of it.
    #[test]
    fn every_accepted_inject_reaches_the_agent_intact() {
        for prose in [
            "plain words",
            "a $OMH_GRAPH_PROJECT reference",
            "a ${OMH_GRAPH_PROJECT} braced one",
            "a ${OMH_GRAPH_PROJECT:-none} defaulted one",
            "a $$5 literal dollar",
            "quotes \" and \\ and ` backticks",
            "a trailing brace } on its own",
        ] {
            let h = Hook::parse(
                &serde_json::json!({ "on": "turn-end", "inject": prose }).to_string(),
                "h.json",
            )
            .unwrap_or_else(|e| panic!("{prose:?} should validate: {e}"));

            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(&rendered("p", &h, hooks_binding()).command)
                .env("OMH_GRAPH_PROJECT", "repo-s01")
                .stdin(std::process::Stdio::null())
                .output()
                .expect("sh must run");
            assert!(
                out.status.success() && out.stderr.is_empty(),
                "{prose:?} did not run: {} {:?}",
                String::from_utf8_lossy(&out.stderr),
                out.status.code()
            );
            let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
                .unwrap_or_else(|e| panic!("{prose:?} emitted no JSON: {e}"));
            assert!(
                !doc["hookSpecificOutput"]["additionalContext"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "{prose:?} injected nothing"
            );
        }
    }

    /// `${VAR:-default}` is how anybody writes a defensive reference, and it
    /// has to count as reading the field — otherwise the preamble is skipped,
    /// the variable is unset, and the hook silently takes its default on every
    /// call while every assertion about its text still passes.
    #[test]
    fn a_braced_reference_with_a_default_still_reads_the_field() {
        let h = Hook::parse(
            r#"{"on":"before-tool","run":"echo ${OMH_TOOL_FILE:-none}"}"#,
            "h.json",
        )
        .unwrap();
        assert_eq!(h.fields(), BTreeSet::from([Field::ToolFile]));
    }

    /// A `$` in prose has to name something the sandbox will actually set.
    ///
    /// The rule was "followed by a letter", which accepts `$PATH`, `$USER` and
    /// `$HOME` — each of which expands to something real and wrong inside the
    /// container, so the sentence arrives mangled rather than merely empty.
    /// And `$OMH_CAPTURE` without a `capture` expands to nothing at all.
    #[test]
    fn a_dollar_naming_something_omh_never_sets_is_refused() {
        for prose in [
            "your $PATH is long",
            "run as $USER",
            "captured $OMH_CAPTURE",
        ] {
            let err = Hook::parse(
                &serde_json::json!({ "on": "turn-end", "inject": prose }).to_string(),
                "h.json",
            )
            .expect_err(&format!("{prose:?} names nothing omh binds"));
            assert!(
                err.to_string().contains("OMH_"),
                "must name what is available: {err}"
            );
        }
    }

    /// A malformed `${…}` is a **bad substitution** — a runtime failure, not a
    /// parse failure.
    ///
    /// The first guard asked whether a `}` appeared anywhere later in the
    /// string, so `${ high } today` passed; and its companion used `sh -n`,
    /// which returns 0 for a bad substitution because the script parses fine.
    /// The hook then exits non-zero before `jq` runs, nothing is injected, and
    /// every assertion about the hook's text still holds.
    #[test]
    fn a_malformed_expansion_is_refused_even_when_a_brace_appears_later() {
        for prose in [
            "cost is ${ high } today",
            "a ${} placeholder",
            "cost is ${ high",
            "${1bad} name",
            // A `}` *before* an unclosed `${`. The first version of this guard
            // asked whether a `}` appeared anywhere in the string, so one
            // closer satisfied an opener that came after it.
            "a } brace, then cost is ${ high",
        ] {
            assert!(
                Hook::parse(
                    &serde_json::json!({ "on": "turn-end", "inject": prose }).to_string(),
                    "h.json",
                )
                .is_err(),
                "{prose:?} renders a bad substitution and must not validate"
            );
        }
    }

    /// `graph-first` fires on every search and reads no payload at all. A
    /// preamble it does not need is a `jq` process per search, forever.
    #[test]
    fn only_the_fields_a_hook_mentions_are_bound() {
        let quiet = Hook::parse(r#"{"on":"before-tool","inject":"a nudge"}"#, "h.json").unwrap();
        assert!(quiet.fields().is_empty());
        assert!(!rendered("q", &quiet, hooks_binding())
            .command
            .contains("jq -r"));

        let reads = Hook::parse(
            r#"{"on":"before-tool","when":"[ -f \"$OMH_TOOL_FILE\" ]","run":"x"}"#,
            "h.json",
        )
        .unwrap();
        assert_eq!(reads.fields(), BTreeSet::from([Field::ToolFile]));
    }

    /// A name that merely starts with a binding's is not that binding.
    #[test]
    fn a_longer_name_is_not_a_field_reference() {
        let h = Hook::parse(
            r#"{"on":"before-tool","run":"echo $OMH_TOOL_FILENAME"}"#,
            "h.json",
        )
        .unwrap();
        assert!(
            h.fields().is_empty(),
            "OMH_TOOL_FILENAME is its own variable"
        );
    }

    /// stdin is consumable once. A second bare `jq` would read an empty payload
    /// and the field would silently be blank — a hook that satisfies every
    /// assertion about its text while doing nothing.
    #[test]
    fn the_payload_is_read_once_however_many_fields() {
        let h = Hook::parse(
            r#"{"on":"before-tool","when":"[ -n \"$OMH_TOOL_COMMAND\" ]",
                "inject":"about $OMH_TOOL_FILE"}"#,
            "h.json",
        )
        .unwrap();
        let cmd = rendered("two", &h, hooks_binding()).command;
        assert_eq!(cmd.matches("$(cat)").count(), 1, "got: {cmd}");
        assert_eq!(cmd.matches("jq -r").count(), 2, "one per field: {cmd}");
    }

    // ── the translation ─────────────────────────────────────────────────────

    /// The shipped adapter, read once.
    ///
    /// Leaked deliberately: every test below wants the same maps a launch
    /// uses, and reconstructing a `Binding` from its parts was a fixture that
    /// could be wrong in the same direction as the code.
    fn shipped() -> &'static Adapter {
        static CELL: std::sync::OnceLock<Adapter> = std::sync::OnceLock::new();
        CELL.get_or_init(claude)
    }

    fn hooks_binding() -> &'static Binding {
        shipped()
            .supports(Capability::Hooks)
            .expect("claude has hooks")
    }

    /// An absent map entry means this harness has no such moment — the same
    /// "absent key is graceful degradation" rule the capability map uses, one
    /// level down. What changes is the granularity: the capability survives, so
    /// the report has to name the hook and what it wanted, or a hook that is
    /// simply not there looks exactly like a hook that is working.
    #[test]
    fn an_event_this_harness_cannot_express_drops_the_hook_by_name() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nturn-end = \"Stop\"\n[inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(r#"{"on":"after-tool","run":"cargo fmt"}"#, "h.json").unwrap();
        let d = dropped("rust-format", &h, &b);
        assert_eq!(d.name, "rust-format");
        assert!(d.wanted.contains("after-tool"), "got: {}", d.wanted);

        // And the sibling still ships, which is the half a capability-level
        // count could never say.
        let sibling = Hook::parse(r#"{"on":"turn-end","run":"cargo test"}"#, "h.json").unwrap();
        assert_eq!(rendered("rust-test", &sibling, &b).command, "cargo test");
    }

    #[test]
    fn an_unmapped_tool_drops_the_hook_saying_which_tool() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n\
             [inject]\ntemplate = \"echo {{text}}\"\n",
        );
        // A harness that has a shell and no reader. The map is the adapter's
        // now, so an absent tool is an absent adapter-level entry.
        let tools = BTreeMap::from([(Tool::Shell, "Bash".to_string())]);
        let h = Hook::parse(
            r#"{"on":"before-tool","tools":["read"],"run":"x"}"#,
            "h.json",
        )
        .unwrap();

        match render("peek", &h, &b, &tools).unwrap() {
            Outcome::Dropped(d) => assert!(d.wanted.contains("read"), "got: {}", d.wanted),
            Outcome::Rendered(r) => panic!("unexpectedly rendered: {r:?}"),
        }
    }

    /// The payload is names too, so it degrades the same way. A harness whose
    /// stdin schema has no filename cannot answer `graph-read`'s question, and
    /// rendering it anyway would produce a hook comparing an empty string.
    #[test]
    fn an_unmapped_field_drops_the_hook_saying_which_field() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n\
             [fields]\ntool-command = \".cmd\"\n[inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(
            r#"{"on":"before-tool","when":"[ -f \"$OMH_TOOL_FILE\" ]","run":"x"}"#,
            "h.json",
        )
        .unwrap();
        assert!(dropped("graph-read", &h, &b).wanted.contains("tool-file"));
    }

    /// And the mirror: a harness that can block but not advise drops the
    /// *injectors*, and does not quietly refuse the call instead.
    ///
    /// The guarded direction was only ever one way round. Six tests catch a
    /// harness handing a refusal to the advisory protocol; nothing caught the
    /// reverse, which is the worse half — a nudge promoted to a wall stops work
    /// the user never asked to stop.
    #[test]
    fn a_harness_that_cannot_advise_drops_the_nudge_by_name() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n\
             [refuse]\ntemplate = \"deny {{text}}\"\n",
        );
        let h = Hook::parse(r#"{"on":"before-tool","inject":"a nudge"}"#, "h.json").unwrap();
        let d = dropped("graph-first", &h, &b);
        assert!(
            d.wanted.contains("inject"),
            "say what it wanted: {}",
            d.wanted
        );

        // A `run` still lands: losing one protocol is not losing the capability.
        let r = Hook::parse(r#"{"on":"before-tool","run":"x"}"#, "h.json").unwrap();
        assert_eq!(rendered("r", &r, &b).command, "x");
    }

    /// A harness that can advise but not block drops the refusal by name.
    ///
    /// Never downgraded to a notice: a wall quietly becoming a nudge is the
    /// mirror of the mistake `refuse` exists to stop, and it would be invisible
    /// — the hook would appear to work while the call it was meant to prevent
    /// went ahead.
    #[test]
    fn a_harness_that_cannot_refuse_drops_the_hook_by_name() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n\
             [inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(r#"{"on":"before-tool","refuse":"no git here"}"#, "h.json").unwrap();
        let d = dropped("git-unavailable", &h, &b);
        assert_eq!(d.name, "git-unavailable");
        assert!(
            d.wanted.contains("refuse"),
            "say what it wanted: {}",
            d.wanted
        );
    }

    /// The refusal reaches the model through this harness's own protocol, which
    /// for Claude Code is a permission decision rather than context.
    ///
    /// Verified against code.claude.com/docs/en/hooks (read 2026-08-13):
    /// `additionalContext` advises, `permissionDecision: "deny"` with a
    /// `permissionDecisionReason` blocks.
    #[test]
    fn a_refusal_reaches_the_model_through_the_harnesss_own_protocol() {
        let h = Hook::parse(
            r#"{"on":"before-tool","tools":["shell"],"refuse":"git does not work here"}"#,
            "h.json",
        )
        .unwrap();
        let cmd = rendered("git-unavailable", &h, hooks_binding()).command;
        assert!(cmd.contains("permissionDecision"), "got: {cmd}");
        assert!(cmd.contains("deny"), "got: {cmd}");
        assert!(
            !cmd.contains("additionalContext"),
            "a refusal is not a notice: {cmd}"
        );
        assert!(cmd.contains("git does not work here"), "got: {cmd}");
    }

    /// A harness with the moment but no way to accept text drops the injectors
    /// and keeps the runners.
    #[test]
    fn a_harness_that_cannot_take_text_still_runs_commands() {
        let b =
            binding("path = \"/x\"\nrender = \"claude-settings\"\n[events]\nturn-end = \"Stop\"\n");
        let say = Hook::parse(r#"{"on":"turn-end","inject":"hello"}"#, "h.json").unwrap();
        assert!(dropped("say", &say, &b).wanted.contains("inject"));

        let run = Hook::parse(r#"{"on":"turn-end","run":"cargo test"}"#, "h.json").unwrap();
        assert_eq!(rendered("run", &run, &b).command, "cargo test");
    }

    /// Prose reaches the agent through a shell, and the characters that end a
    /// shell word are exactly the ones prose is full of. Asserting on the
    /// command string proves the sentence is embedded, never that a shell will
    /// emit it — so this runs the thing.
    #[test]
    fn injected_prose_survives_the_shell() {
        let prose = "a \"quoted\" word, a \\ backslash, a `backtick`, and $$5 — it's fine";
        let h = Hook::parse(
            &serde_json::json!({ "on": "turn-end", "inject": prose }).to_string(),
            "h.json",
        )
        .unwrap();

        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&rendered("prose", &h, hooks_binding()).command)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("sh must run");
        assert!(
            out.stderr.is_empty(),
            "the harness shows the user stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON: {} ({e})", String::from_utf8_lossy(&out.stdout)));
        assert_eq!(
            doc["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap(),
            prose.replace("$$", "$"),
            "the prose has to survive shell quoting intact"
        );
    }

    /// `capture` runs before `when`, which is the only order that lets a
    /// predicate test what was captured — `graph-orient` stays silent when the
    /// graph answered nothing, and that is the whole of its degradation.
    #[test]
    fn capture_is_evaluated_before_the_predicate_that_tests_it() {
        let h = Hook::parse(
            r#"{"on":"session-start","capture":"echo hi",
                "when":"[ -n \"$OMH_CAPTURE\" ]","inject":"got $OMH_CAPTURE"}"#,
            "h.json",
        )
        .unwrap();
        let cmd = rendered("orient", &h, hooks_binding()).command;
        assert!(
            cmd.find("OMH_CAPTURE=$(").unwrap() < cmd.find("|| exit 0").unwrap(),
            "got: {cmd}"
        );
    }
}
