//! Reading a harness transcript for what the agent did, and what it cost.
//!
//! A `.jsonl` transcript — one JSON object per line — is the only place that
//! records what happened turn by turn: which tools ran, which files they
//! touched, and how many tokens each model burned. `omh sNN` reviews a diff;
//! this is what lets it say what produced the diff and what it cost, so a
//! review is not blind.
//!
//! Pure: a string in, a `Summary` out. Nothing here reads a file or a clock —
//! the caller hands over the bytes — so every shape a transcript can take is
//! reachable from a test. The one judgement that matters is kept honest by the
//! type: a transcript omh could not parse is **not** an empty session. A line
//! that will not parse is counted, and a summary that read nothing but holds
//! unreadable lines says *could not read*, never *0 turns*.

use std::collections::{BTreeMap, BTreeSet};

/// What a transcript records, summed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// Assistant turns — one per model response.
    pub turns: usize,
    /// Tool calls by name, e.g. `Edit` → 12.
    pub tools: BTreeMap<String, usize>,
    /// Every file a tool named, deduplicated.
    pub files: BTreeSet<String>,
    /// Token use per model, and its cost when the model has a known price.
    pub usage: BTreeMap<String, Usage>,
    /// Lines that did not parse as a transcript record.
    pub unreadable: usize,
}

/// Tokens for one model, and what they cost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /// Tokens written to and read from the prompt cache. Separate because they
    /// are priced differently — a cache read is a fraction of an input token,
    /// a cache write a little more — and on a Claude Code session cache reads
    /// dominate the volume, so leaving them out understates the real spend.
    pub cache_read: u64,
    pub cache_write: u64,
    /// `None` for a model with no price in `PRICES` — tokens are still summed,
    /// but a cost omh does not know is not reported as `0`.
    pub cost: Option<f64>,
}

impl Summary {
    /// Whether this summary is a real reading or a failure wearing zeroes.
    ///
    /// `unreadable > 0 && turns == 0` is the one case that must not render as
    /// "0 turns": omh opened the file and could make nothing of it, which is a
    /// different thing to tell a reviewer than "the agent did nothing".
    pub fn is_unreadable(&self) -> bool {
        self.turns == 0 && self.unreadable > 0
    }

    /// Total cost across models with a known price. `None` when no model priced.
    pub fn cost(&self) -> Option<f64> {
        let costs: Vec<f64> = self.usage.values().filter_map(|u| u.cost).collect();
        match costs.is_empty() {
            true => None,
            false => Some(costs.iter().sum()),
        }
    }
}

/// Per-model price in dollars per million tokens, `(input, output)`.
///
/// **Dated: read 2026-09 from Anthropic's pricing.** A model omh does not have
/// a row for is not free — it reports tokens and no cost, so a new model reads
/// as "cost unknown" rather than "cost zero". Update the table, do not guess.
pub const PRICES: &[(&str, f64, f64)] = &[
    ("claude-opus-4", 15.0, 75.0),
    ("claude-sonnet-4", 3.0, 15.0),
    ("claude-3-5-haiku", 0.80, 4.0),
    ("claude-3-5-sonnet", 3.0, 15.0),
];

/// The price row whose key is a prefix of `model`, if any.
///
/// A prefix match, because a transcript writes the dated id
/// (`claude-sonnet-4-20260514`) and the table keys the family. The longest
/// matching key wins, so `claude-3-5-sonnet` is not shadowed by a shorter row.
fn price_of(model: &str) -> Option<(f64, f64)> {
    PRICES
        .iter()
        .filter(|(key, _, _)| model.starts_with(key))
        .max_by_key(|(key, _, _)| key.len())
        .map(|(_, input, output)| (*input, *output))
}

/// Read a `.jsonl` transcript into a `Summary`.
pub fn summarise(jsonl: &str) -> Summary {
    let mut s = Summary::default();
    // A Codex rollout names its model once per turn, in `turn_context`, and
    // its usage lines after — so the model is carried from one to the next.
    let mut codex_model = None;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            s.unreadable += 1;
            continue;
        };
        // Codex wraps every record in `payload`; Claude Code never does. Read
        // line by line rather than per file, because a session resumed with
        // the other harness keeps both in one transcripts directory.
        match value.get("payload") {
            Some(payload) => read_codex_record(&value, payload, &mut codex_model, &mut s),
            None => read_record(&value, &mut s),
        }
    }
    // Prices, applied once the tokens are summed.
    for (model, usage) in s.usage.iter_mut() {
        usage.cost = price_of(model).map(|(input, output)| {
            // Cache pricing relative to the base input rate, per Anthropic's
            // published multipliers (read 2026-09): a cache read bills at 0.1x,
            // a cache write at 1.25x.
            (usage.input as f64 / 1_000_000.0) * input
                + (usage.output as f64 / 1_000_000.0) * output
                + (usage.cache_read as f64 / 1_000_000.0) * input * 0.1
                + (usage.cache_write as f64 / 1_000_000.0) * input * 1.25
        });
    }
    s
}

/// Fold one transcript line into the summary.
///
/// Tolerant of shape: a record that is not an assistant turn contributes
/// nothing rather than counting as unreadable, because a transcript carries
/// user turns, tool results and metadata lines too, and none of those is a
/// parse failure.
fn read_record(value: &serde_json::Value, s: &mut Summary) {
    if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return;
    }
    s.turns += 1;
    let message = value.get("message");

    if let Some(usage) = message.and_then(|m| m.get("usage")) {
        let model = message
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();
        let entry = s.usage.entry(model).or_default();
        let tok = |name: &str| usage.get(name).and_then(|t| t.as_u64()).unwrap_or(0);
        entry.input += tok("input_tokens");
        entry.output += tok("output_tokens");
        entry.cache_read += tok("cache_read_input_tokens");
        entry.cache_write += tok("cache_creation_input_tokens");
    }

    let content = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());
    for block in content.into_iter().flatten() {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
            *s.tools.entry(name.to_string()).or_insert(0) += 1;
        }
        // A tool that names a file records it, whatever the tool is. `file_path`
        // is Claude Code's key; a tool without one contributes no file rather
        // than an empty string.
        if let Some(path) = block
            .get("input")
            .and_then(|i| i.get("file_path"))
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty())
        {
            s.files.insert(path.to_string());
        }
    }
}

fn str_at<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// Fold one line of a Codex rollout into the summary.
///
/// Shapes measured from Codex 0.154.0's `$CODEX_HOME/sessions/**/rollout-*.jsonl`:
///
/// - `turn_context` carries the model for the turn.
/// - `event_msg` / `token_count` carries usage once per model response, as a
///   running `total_token_usage` and this response's `last_token_usage` — the
///   latter is summed, the former would count every earlier response again.
///   `token_usage_record` repeats the same numbers and is not read.
/// - `response_item` / `function_call` and `custom_tool_call` are the calls the
///   model made, by the names it used (`exec_command`, `apply_patch`).
/// - `event_msg` / `item_completed` carries the files: the paths a
///   `FileChange` changed and the paths a `CommandExecution` read.
fn read_codex_record(
    value: &serde_json::Value,
    payload: &serde_json::Value,
    model: &mut Option<String>,
    s: &mut Summary,
) {
    match (str_at(value, "type"), str_at(payload, "type")) {
        (Some("turn_context"), _) => {
            if let Some(m) = str_at(payload, "model") {
                *model = Some(m.to_string());
            }
        }
        (Some("event_msg"), Some("token_count")) => {
            let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) else {
                return;
            };
            s.turns += 1;
            let tok = |name: &str| last.get(name).and_then(|t| t.as_u64()).unwrap_or(0);
            let entry = s
                .usage
                .entry(model.clone().unwrap_or_else(|| "unknown".into()))
                .or_default();
            let cached = tok("cached_input_tokens");
            // Codex's input count includes the cached part; Claude Code's does
            // not. Split here so a cached token is counted once, as a read.
            entry.input += tok("input_tokens").saturating_sub(cached);
            entry.cache_read += cached;
            entry.cache_write += tok("cache_write_input_tokens");
            entry.output += tok("output_tokens");
        }
        (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
            if let Some(name) = str_at(payload, "name") {
                *s.tools.entry(name.to_string()).or_insert(0) += 1;
            }
        }
        (Some("event_msg"), Some("item_completed")) => {
            let Some(item) = payload.get("item") else {
                return;
            };
            match str_at(item, "type") {
                Some("FileChange") => {
                    for path in item
                        .get("changes")
                        .and_then(|c| c.as_object())
                        .into_iter()
                        .flatten()
                    {
                        s.files.insert(path.0.clone());
                    }
                }
                Some("CommandExecution") => {
                    for parsed in item
                        .get("parsed_cmd")
                        .and_then(|p| p.as_array())
                        .into_iter()
                        .flatten()
                    {
                        if let Some(path) = str_at(parsed, "path").filter(|p| !p.is_empty()) {
                            s.files.insert(path.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(model: &str, input: u64, output: u64, tools: &[(&str, &str)]) -> String {
        let content: Vec<serde_json::Value> = tools
            .iter()
            .map(|(name, file)| {
                serde_json::json!({
                    "type": "tool_use",
                    "name": name,
                    "input": { "file_path": file },
                })
            })
            .collect();
        serde_json::json!({
            "type": "assistant",
            "message": {
                "model": model,
                "usage": { "input_tokens": input, "output_tokens": output },
                "content": content,
            }
        })
        .to_string()
    }

    /// A transcript records turns, tools, the files they touched, and tokens.
    #[test]
    fn a_transcript_is_read_into_what_the_agent_did() {
        let jsonl = [
            r#"{"type":"user","message":{"content":"go"}}"#.to_string(),
            assistant(
                "claude-sonnet-4-20260514",
                100,
                50,
                &[("Edit", "src/a.rs"), ("Read", "src/b.rs")],
            ),
            assistant("claude-sonnet-4-20260514", 200, 80, &[("Edit", "src/a.rs")]),
        ]
        .join("\n");
        let s = summarise(&jsonl);
        assert_eq!(s.turns, 2, "two assistant turns, the user line is not one");
        assert_eq!(s.tools.get("Edit"), Some(&2));
        assert_eq!(s.tools.get("Read"), Some(&1));
        assert_eq!(
            s.files,
            ["src/a.rs".to_string(), "src/b.rs".to_string()]
                .into_iter()
                .collect(),
            "each file once, though a.rs was edited twice"
        );
        assert!(!s.is_unreadable());
    }

    /// A Codex 0.154.0 rollout, written by `codex exec` against a stub model
    /// that read a file, patched another and answered — three requests of
    /// 1,001/2,001/3,001 input tokens (100/200/300 cached) and 12/22/32 output.
    /// Trimmed of long prompt text; every record kept is as Codex wrote it.
    const CODEX_ROLLOUT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex-rollout.jsonl"
    ));

    /// A Codex rollout is read into the same summary a Claude transcript is.
    #[test]
    fn a_codex_rollout_is_read_into_what_the_agent_did() {
        let s = summarise(CODEX_ROLLOUT);
        assert_eq!(s.unreadable, 0);
        assert_eq!(s.turns, 3, "one per model response");
        assert_eq!(s.tools.get("exec_command"), Some(&1));
        assert_eq!(s.tools.get("apply_patch"), Some(&1));
        assert_eq!(
            s.files,
            ["/work/a.txt".to_string(), "/work/b.txt".to_string()]
                .into_iter()
                .collect(),
            "the file the shell read and the file the patch changed"
        );
        let u = &s.usage["gpt-5.5"];
        assert_eq!(u.output, 12 + 22 + 32);
        assert_eq!(u.cache_read, 100 + 200 + 300);
        assert_eq!(
            u.input,
            (1001 - 100) + (2001 - 200) + (3001 - 300),
            "Codex's input count includes the cached part; counted once, as a cache read"
        );
        assert_eq!(u.cost, None, "no price omh has, so no cost — not zero");
    }

    /// Codex records usage twice per response — `token_count` and
    /// `token_usage_record` — and `total_token_usage` is a running sum. Only
    /// `last_token_usage`, once per response, adds up to what was spent.
    #[test]
    fn a_codex_rollout_counts_each_request_once() {
        let s = summarise(CODEX_ROLLOUT);
        let u = &s.usage["gpt-5.5"];
        assert_eq!(
            u.input + u.cache_read + u.output,
            6069,
            "the rollout's own final total"
        );
    }

    /// One session directory can hold both formats — a session resumed with the
    /// other harness — and each line is read as what it is.
    #[test]
    fn claude_and_codex_lines_are_read_side_by_side() {
        let jsonl = format!(
            "{}\n{}",
            assistant("claude-sonnet-4-20260514", 100, 50, &[("Edit", "src/a.rs")]),
            CODEX_ROLLOUT
        );
        let s = summarise(&jsonl);
        assert_eq!(s.turns, 4);
        assert_eq!(s.usage["claude-sonnet-4-20260514"].input, 100);
        assert_eq!(s.usage["gpt-5.5"].output, 66);
    }

    /// A transcript omh cannot parse is never reported as an empty session.
    #[test]
    fn a_transcript_omh_cannot_parse_is_never_reported_as_an_empty_session() {
        let s = summarise("this is not json\n{also not\n");
        assert_eq!(s.turns, 0);
        assert_eq!(s.unreadable, 2);
        assert!(s.is_unreadable(), "unreadable, not an empty session");

        // An empty file is genuinely empty, not unreadable.
        assert!(!summarise("").is_unreadable());
    }

    /// Tokens are summed per model, not across them.
    #[test]
    fn tokens_are_summed_per_model_not_across_them() {
        let jsonl = [
            assistant("claude-sonnet-4-20260514", 100, 50, &[]),
            assistant("claude-opus-4-20260514", 10, 5, &[]),
            assistant("claude-sonnet-4-20260514", 100, 50, &[]),
        ]
        .join("\n");
        let s = summarise(&jsonl);
        assert_eq!(s.usage["claude-sonnet-4-20260514"].input, 200);
        assert_eq!(s.usage["claude-sonnet-4-20260514"].output, 100);
        assert_eq!(s.usage["claude-opus-4-20260514"].input, 10);
    }

    /// A model with no recorded price reports tokens and no cost — never zero.
    #[test]
    fn a_model_with_no_recorded_price_reports_tokens_and_no_cost() {
        let s = summarise(&assistant("some-future-model-x", 1000, 1000, &[]));
        let usage = &s.usage["some-future-model-x"];
        assert_eq!(usage.input, 1000);
        assert_eq!(usage.cost, None, "a price omh does not have is not zero");
        assert_eq!(s.cost(), None);

        // A known model does carry a cost.
        let known = summarise(&assistant(
            "claude-sonnet-4-20260514",
            1_000_000,
            1_000_000,
            &[],
        ));
        assert_eq!(
            known.usage["claude-sonnet-4-20260514"].cost,
            Some(3.0 + 15.0)
        );
        assert_eq!(known.cost(), Some(18.0));
    }

    /// Cache tokens are priced and counted, not dropped — on a Claude Code
    /// session they dominate the volume, so leaving them out understates cost.
    #[test]
    fn cache_tokens_are_counted_in_the_cost() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "model": "claude-sonnet-4-20260514",
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cache_read_input_tokens": 1_000_000,
                    "cache_creation_input_tokens": 1_000_000,
                },
                "content": [],
            }
        })
        .to_string();
        let s = summarise(&line);
        let u = &s.usage["claude-sonnet-4-20260514"];
        assert_eq!((u.cache_read, u.cache_write), (1_000_000, 1_000_000));
        // sonnet input is $3/Mtok: read at 0.1x = $0.30, write at 1.25x = $3.75.
        assert_eq!(
            u.cost,
            Some(0.30 + 3.75),
            "cache reads and writes are priced, not free"
        );
    }

    /// Every tool call names the file it touched, when the record carries one.
    #[test]
    fn every_tool_call_names_the_file_it_touched_when_the_record_carries_one() {
        let jsonl = assistant("claude-sonnet-4-20260514", 1, 1, &[("Bash", "")]);
        // A Bash call with an empty file_path names no file rather than "".
        let s = summarise(&jsonl);
        assert_eq!(s.tools.get("Bash"), Some(&1));
        assert!(
            s.files.is_empty() || !s.files.contains(""),
            "no empty file name"
        );
    }
}
