//! omh's words for what a hook binds to: the moments, tools and payload
//! fields every harness has, and what a hook does when its moment comes.
//!
//! A leaf, so an adapter can say how its harness spells each word without
//! depending on the hook format that uses them.

use serde::{Deserialize, Serialize};

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
