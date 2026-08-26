//! What a command has to say, separated from the act of saying it.
//!
//! Every command used to `println!` at the point it did the work, which cost
//! three things at once: the text could not be tested without running the
//! binary, the same fact was worded differently in two places, and there was
//! nowhere to put a second audience. `--json` is that second audience, and it
//! is the reason this module is shaped around a **value** rather than a string.
//!
//! ## The rule this file exists to keep
//!
//! A command produces a [`Report`]; [`emit`] turns it into bytes. Human text
//! may be styled, JSON never is — not "is not currently", *cannot be*, because
//! [`emit`] does not hand the [`Palette`] to [`Report::json`] at all. A colour
//! escape in a machine format is not a cosmetic bug: it is a parse failure in
//! whatever reads it, and it would be found by the person piping omh into `jq`
//! rather than by us.
//!
//! ## Width is measured on what the eye sees
//!
//! Columns size to their content instead of to a number somebody guessed.
//! `{:<8}` was in this codebase eleven times, and every one of them sheared the
//! row the first time an id or a harness name outgrew its budget.
//!
//! The subtlety that makes this a module rather than a `format!` argument:
//! **a styled string is longer than it looks.** `\x1b[2mabc\x1b[0m` is 3
//! columns wide and 12 bytes long, so padding it with `str::len` indents the
//! next column by 9 too few — and only when colour is on, which is exactly the
//! case nobody tests in CI. [`display_width`] answers the question the terminal
//! answers.

use anstyle::{AnsiColor, Style};

// ── Styling ─────────────────────────────────────────────────────────────────

/// Section headings and anything naming the subject of a command.
pub const HEAD: Style = Style::new().bold();

/// A thing went well, is running, is current.
pub const OK: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)));

/// A thing needs attention but the command still worked.
pub const WARN: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)));

/// A thing failed.
pub const BAD: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)));

/// Provenance, hints, units — present for the reader who wants it, out of the
/// way of the reader who does not.
pub const DIM: Style = Style::new().dimmed();

/// An identifier the user will type back at us: a session id, a note key.
pub const NAME: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan)));

/// Whether to style, as asked for on the command line.
///
/// `Auto` is the only value that consults the environment; the other two exist
/// because the environment gets it wrong in both directions. `Never` is for a
/// terminal that renders escapes literally, and `Always` is for the far more
/// common `omh info | less -R`, where stdout is a pipe and the human on the far
/// end of it still wants colour.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

/// Whether this run paints, decided once so it cannot drift mid-command.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    styled: bool,
}

impl Palette {
    /// Resolve the three inputs into one answer.
    ///
    /// Taken as arguments rather than read from the process, so the decision is
    /// testable without a terminal — the case that matters most here is "not a
    /// tty", which is how every CI run and every test sees us, and which no
    /// developer's machine reproduces by accident.
    ///
    /// `NO_COLOR` beats `Auto` but not `Always`: the convention is that it
    /// speaks for a user who has not said otherwise, and `--color always` is
    /// that user saying otherwise, on this one run, with their own hands.
    /// An **empty** `NO_COLOR` is not set — <https://no-color.org> is explicit
    /// that presence alone is not the trigger, and the empty string is what a
    /// shell leaves behind when a wrapper script unsets a variable badly.
    pub fn resolve(pref: Color, no_color: Option<&str>, is_tty: bool) -> Self {
        let forbidden = no_color.is_some_and(|v| !v.is_empty());
        let styled = match pref {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => is_tty && !forbidden,
        };
        Self { styled }
    }

    /// A palette that never paints — for JSON, and for tests that assert on
    /// text rather than on escapes.
    pub fn plain() -> Self {
        Self { styled: false }
    }

    /// Whether anything omh prints through this will carry colour.
    ///
    /// Asked by the two commands that hand the terminal to git: git honours
    /// neither `NO_COLOR` nor omh's `--color`, so the answer resolved here has
    /// to be passed to it as `--color=<when>` or the user's flag does nothing.
    pub fn is_plain(&self) -> bool {
        !self.styled
    }

    /// Wrap `text` in `style`, or hand it back untouched.
    ///
    /// The reset is `style`'s own, written as `{style:#}`, rather than a
    /// hardcoded `\x1b[0m`: a reset that does not match what was set leaves the
    /// terminal holding an attribute the next line never asked for, and the
    /// symptom is a bold prompt after the command exits.
    pub fn paint(&self, style: Style, text: &str) -> String {
        if self.styled {
            format!("{style}{text}{style:#}")
        } else {
            text.to_string()
        }
    }
}

/// How many columns `s` occupies once the terminal has eaten its escapes.
///
/// Everything that aligns goes through here. See the module docs: a styled
/// string measured with `str::len` is counted at its byte length, and the
/// column after it lands wherever the escape happened to be long.
///
/// **What this counts, and what it does not.** One `char`, one column, after
/// dropping escape sequences and the combining marks that attach to the
/// character before them. That is exact for everything omh prints today —
/// ASCII, `✓`, `✗`, box drawing, accented Latin — and wrong for CJK and
/// emoji, which the terminal draws two columns wide. Getting *those* right
/// needs a Unicode width table, which is a dependency and a data file; the
/// honest position is that this handles what omh emits, and a session id or a
/// harness name in Han script would shear a column. If one ever does, the fix
/// is `unicode-width`, not a wider guess here.
pub fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            ESC => skip_escape_sequence(&mut chars),
            c if is_combining(c) => {}
            _ => width += 1,
        }
    }
    width
}

const ESC: char = '\x1b';
const BEL: char = '\x07';

/// Consume the rest of an escape sequence whose `ESC` has already been taken.
///
/// The terminal's grammar, which no amount of naming can make evident from the
/// byte values: **CSI** (`ESC [ … final`) runs until a byte in `@`..=`~`;
/// **OSC** (`ESC ] … terminator`) runs until BEL, or until the `\` that
/// follows an ESC. Anything else is a two-byte sequence, and taking the second
/// byte to look at it is what consumes it.
fn skip_escape_sequence(chars: &mut std::str::Chars) {
    match chars.next() {
        Some('[') => {
            chars.find(|c| ('\x40'..='\x7e').contains(c));
        }
        Some(']') => {
            let mut prev = '\0';
            for c in chars.by_ref() {
                if c == BEL || (c == '\\' && prev == ESC) {
                    break;
                }
                prev = c;
            }
        }
        _ => {}
    }
}

/// Whether `c` is drawn on top of the character before it rather than beside
/// it, and so costs no column of its own.
///
/// The three combining blocks a Latin-script locale can actually produce: a
/// decomposed `é` — `e` followed by U+0301, which is one column and two chars
/// — is what macOS's filesystem normalisation puts in a path, and a path is
/// what reaches a table cell.
fn is_combining(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036f}' | '\u{1ab0}'..='\u{1aff}' | '\u{20d0}'..='\u{20ff}')
}

// ── Tables ──────────────────────────────────────────────────────────────────

pub struct Cell {
    text: String,
    style: Option<Style>,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
        }
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style: Some(style),
        }
    }

    fn render(&self, p: &Palette) -> String {
        match self.style {
            Some(s) => p.paint(s, &self.text),
            None => self.text.clone(),
        }
    }

    /// What the cell says, without the styling.
    ///
    /// For asserting on one cell rather than on a rendered row. A test that
    /// scans a whole line for the thing it cares about reads every other
    /// column too, and passes or fails on the fixture's other fields — which
    /// is how a guard against *a count where there should be a question* came
    /// to depend on nobody putting a digit in a session id.
    ///
    /// `cfg(test)` because that is the whole of its purpose: nothing that
    /// ships needs a cell's text back out, and in a binary crate `pub` does
    /// not stop it being dead code.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Rows of cells that will line up, however long their contents turn out to be.
#[derive(Default)]
pub struct Table {
    rows: Vec<Vec<Cell>>,
    indent: usize,
    gap: usize,
}

/// How far a table sits from the left margin — the shape omh's output already
/// had, now in one place rather than at every call site.
const INDENT: usize = 2;

/// The gap between columns. Two spaces is the narrowest gutter that still reads
/// as a gutter rather than a typo when a cell ends in a digit.
const GUTTER: usize = 2;

impl Table {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            indent: INDENT,
            gap: GUTTER,
        }
    }

    pub fn indent(mut self, n: usize) -> Self {
        self.indent = n;
        self
    }

    pub fn row(mut self, cells: Vec<Cell>) -> Self {
        self.rows.push(cells);
        self
    }

    /// Lay the rows out, sizing every column to its widest cell.
    ///
    /// The **last** cell of a row is never padded. Trailing whitespace is
    /// invisible until someone diffs the output or copies a line out of it, and
    /// a table whose every row ends in spaces makes both of those worse for no
    /// gain — nothing is ever aligned against the right-hand edge.
    pub fn render(&self, p: &Palette) -> String {
        let columns = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|i| {
                self.rows
                    .iter()
                    .filter_map(|r| r.get(i))
                    .map(|c| display_width(&c.text))
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        let mut out = String::new();
        for row in &self.rows {
            let mut line = " ".repeat(self.indent);
            for (i, cell) in row.iter().enumerate() {
                let rendered = cell.render(p);
                if i + 1 == row.len() {
                    line.push_str(&rendered);
                } else {
                    let pad = widths[i].saturating_sub(display_width(&cell.text)) + self.gap;
                    line.push_str(&rendered);
                    line.push_str(&" ".repeat(pad));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// A section title, in the one place that decides what a section title looks
/// like.
///
/// `omh info` wrote `harnesses:`, `omh config` wrote `mcp:`, and `omh repo`
/// wrote its own third thing — all bare `println!`s, so nothing stopped them
/// drifting apart, and nothing could restyle them together.
pub fn heading(p: &Palette, text: &str) -> String {
    format!("{}\n", p.paint(HEAD, text))
}

/// What a section says when it has nothing in it.
///
/// Dimmed and parenthesised, because an empty section is a fact about the
/// user's setup rather than a problem with it — and because a section that
/// prints its title and then nothing at all reads like output that got cut off.
pub fn nothing(p: &Palette, why: &str) -> String {
    format!("  {}\n", p.paint(DIM, &format!("({why})")))
}

// ── Voice ───────────────────────────────────────────────────────────────────

/// What omh calls itself when it speaks about itself.
///
/// Diagnostics carry it, answers do not. `omh info` printing `omh: ` before every
/// session would be noise — the user typed the word — but a warning arriving in
/// the middle of a harness's own output needs to say who is talking.
const ME: &str = "omh";

/// Something is wrong but the command carried on. **stderr.**
///
/// On stderr even though the command succeeded, because the thing that makes a
/// warning useful is that `omh info > sessions.txt` still shows it to the person
/// and still keeps it out of the file.
pub fn warning(p: &Palette, msg: &str) -> String {
    format!("{}: {msg}\n", p.paint(WARN, ME))
}

/// A next step the user may want. **stderr**, and phrased as a command.
pub fn hint(p: &Palette, msg: &str) -> String {
    format!("{}\n", p.paint(DIM, msg))
}

/// The whole of why a command failed, cause chain and all. **stderr.**
///
/// anyhow's own `Error:` is `{:?}`'s doing — a debug format, chosen by nobody,
/// which leads with a word that is not the program's name. Worse, the obvious
/// replacement (`e.to_string()`) prints **only the outermost context**, so
/// `writing ~/.omh/facts.json` arrives with no hint that the reason was a full
/// disk. The context chain exists precisely so the innermost cause survives,
/// and dropping it is how an error message becomes something the user has to
/// reproduce under `strace` to act on.
/// Text omh did not write, made safe to print.
///
/// Everything the sandbox names reaches a terminal eventually: git quotes the
/// refs, paths and branch names an agent chose straight back in its stderr, and
/// `problem` prints that. A control sequence in one of them moves the cursor,
/// clears the line, or repaints what omh just said — and omh's own output is
/// the thing a user trusts to tell them whether their work is safe.
///
/// Newline survives because git's messages are several lines and reflowing them
/// into one is its own kind of lie. Everything else in C0 and C1 goes, which
/// takes ESC with it and so takes every escape sequence built on it.
pub fn untrusted(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => c,
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

pub fn problem(p: &Palette, e: &anyhow::Error) -> String {
    let mut out = format!("{}: {}\n", p.paint(BAD, ME), e);
    for cause in e.chain().skip(1) {
        out.push_str(&format!("  {} {cause}\n", p.paint(DIM, "because")));
    }
    out
}

// ── Reports ─────────────────────────────────────────────────────────────────

/// What a command decided, before anyone has chosen how to read it.
///
/// Two renderers over one value, which is the point: a command cannot report a
/// different number of sessions to a person than to a script, because there is
/// only one list and both methods walk it.
pub trait Report {
    /// For a person. May be styled; may reflow; may omit what a person can see
    /// for themselves.
    ///
    /// **The answer only.** A next step rendered here is on stdout by
    /// construction, and `omh s > sessions.txt` then captures advice the
    /// file was never meant to hold — see [`asides`](Self::asides).
    fn human(&self, p: &Palette) -> String;

    /// For a program. Stable field names, nothing elided, no styling.
    fn json(&self) -> serde_json::Value;

    /// What this report has to say that is **not** the answer.
    ///
    /// Defaulted, because most reports are all answer. The two that are not
    /// used to render these lines into [`human`](Self::human), which put them
    /// on stdout: `omh s` wrote *clear each with omh <id> rm* into any
    /// file it was redirected into, and `omh s rm` wrote its two review
    /// commands into one.
    ///
    /// Carried here rather than emitted at the call site because only the
    /// report knows which of its lines are advice, and only [`Ctx`] knows the
    /// stream and the format. Splitting it that way is what stops the next
    /// `.next(…)` quietly landing back on stdout: there is no longer a way to
    /// express one that does.
    fn asides(&self) -> Asides {
        Asides::default()
    }
}

/// Warnings and next steps a report carries, for [`Ctx::say`] to place.
///
/// Two lists rather than one because they are not the same offer: a hint is a
/// promise that the line can be selected and pasted, and a warning mixed into
/// them breaks that promise for every line — the reader can no longer tell
/// which is which without reading them all. Same reason `Action` keeps `next`
/// and `notes` apart.
#[derive(Debug, Default, Clone)]
pub struct Asides {
    /// Something is wrong but the command carried on.
    pub warnings: Vec<String>,
    /// A command the user may want next, reproduced so it can be pasted.
    pub hints: Vec<String>,
}

impl Asides {
    pub fn warn(mut self, msg: impl Into<String>) -> Self {
        self.warnings.push(msg.into());
        self
    }

    pub fn hint(mut self, msg: impl Into<String>) -> Self {
        self.hints.push(msg.into());
        self
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[default]
    Human,
    Json,
}

/// How this run reports, carried to the commands that report.
///
/// The point of passing it rather than reading it: a command cannot decide
/// halfway through that it is talking to a terminal after all. Both fields are
/// settled in `Cli::output` before any work starts.
#[derive(Debug, Default, Clone, Copy)]
pub struct Ctx {
    pub format: Format,
    pub palette: Palette,
}

impl Ctx {
    /// The answer to what the user asked. **stdout**, because it is the thing
    /// they would want to redirect into a file.
    ///
    /// Anything the report carries that is *not* the answer goes to **stderr**
    /// afterwards, through the same [`warn`](Self::warn) and
    /// [`hint`](Self::hint) every other caller uses. Under `--json` it is
    /// dropped instead of printed: [`Report::json`] already carries the same
    /// facts as fields — `next`, `leftovers` — and a second copy as prose on
    /// stderr is noise in whatever is parsing the first.
    pub fn say<R: Report + ?Sized>(&self, report: &R) {
        print!("{}", emit(report, self.format, &self.palette));

        // Flushed before the asides rather than left to the runtime. When both
        // streams are the same terminal the user reads them as one transcript,
        // and stdout is block-buffered the moment it is anything else, so
        // without this the advice can overtake the answer it is about.
        let _ = std::io::Write::flush(&mut std::io::stdout());

        if self.format == Format::Json {
            return;
        }
        let asides = report.asides();
        for warning in &asides.warnings {
            self.warn(warning);
        }
        for hint in &asides.hints {
            self.hint(hint);
        }
    }

    /// Something is wrong but the command carried on. **stderr.**
    pub fn warn(&self, msg: &str) {
        eprint!("{}", warning(&self.palette, msg));
    }

    /// omh speaking about itself, in its own voice. **stderr**, human only.
    ///
    /// Between [`warn`](Self::warn) and [`progress`](Self::progress): not a
    /// problem, so not yellow; not incidental, so not dimmed away. The launch
    /// line — *claude on omh/s01* — is the case this exists for. It is the
    /// last thing omh says before the harness owns the terminal, and it is on
    /// **stderr** because from that moment stdout belongs to the harness and
    /// anything of omh's on it lands in whatever the harness is piped into.
    pub fn announce(&self, msg: &str) {
        if self.format == Format::Human {
            eprintln!("{}: {msg}", self.palette.paint(HEAD, ME));
        }
    }

    /// What omh is doing, while it is still doing it. **stderr**, human only.
    ///
    /// Not part of the answer: `omh doctor > checks.txt` should capture the
    /// checks, not the sentence about which image they ran in. A script gets
    /// nothing, because progress is a courtesy to somebody watching a slow
    /// container start — and the same facts reach `--json` as fields.
    pub fn progress(&self, msg: &str) {
        if self.format == Format::Human {
            eprintln!("{}", self.palette.paint(DIM, msg));
        }
    }

    /// A next step. **stderr**, so it never lands in a redirected answer.
    ///
    /// Suppressed under `--json` — a hint is advice for a person, and a script
    /// reading stderr for real diagnostics does not want prose mixed into them.
    pub fn hint(&self, msg: &str) {
        if self.format == Format::Human {
            eprint!("{}", hint(&self.palette, msg));
        }
    }

    /// A plain palette and human format, for tests and for the paths that
    /// report before the flags have been resolved.
    #[cfg(test)]
    pub fn plain() -> Self {
        Self {
            format: Format::Human,
            palette: Palette::plain(),
        }
    }
}

/// Render a report in the requested format.
///
/// The [`Palette`] reaches [`Report::human`] and nothing else. That is the
/// enforcement of the module's central rule: `json` has no way to paint,
/// because it is never handed anything that could.
pub fn emit<R: Report + ?Sized>(report: &R, format: Format, p: &Palette) -> String {
    match format {
        Format::Human => report.human(p),
        Format::Json => {
            let mut s = serde_json::to_string_pretty(&report.json()).unwrap_or_default();
            s.push('\n');
            s
        }
    }
}

#[cfg(test)]
mod tests {

    /// Text the sandbox wrote cannot paint the terminal.
    ///
    /// git hands back the branch and ref names an agent chose, and omh prints
    /// them. An escape sequence in one repaints omh's own output, which is the
    /// line a user reads to decide whether their work is safe.
    #[test]
    fn text_the_sandbox_wrote_cannot_paint_the_terminal() {
        let forged = "omh/s01\u{1b}[2K\rcommitted to main";
        let safe = untrusted(forged);
        assert!(!safe.contains('\u{1b}'), "no escape survives: {safe:?}");
        assert!(!safe.contains('\r'), "nor a carriage return: {safe:?}");
        assert!(
            safe.contains("omh/s01") && safe.contains("committed to main"),
            "and the text itself is still readable: {safe:?}"
        );
        assert_eq!(
            untrusted("fatal: bad object\nhint: try again"),
            "fatal: bad object\nhint: try again",
            "a newline is how git writes, not something to strip"
        );
    }
    use super::*;

    /// A column is as wide as the terminal draws it, not as wide as its bytes.
    ///
    /// This is the whole reason [`display_width`] exists rather than a
    /// `{:<width$}` at each call site, and it is not hypothetical: `omh doctor`
    /// already prints `✓` and `✗` in the first column of a table. Each is one
    /// column on screen and **three bytes** in memory, so a byte-counting pad
    /// indents every passing row two spaces short of every failing one — in the
    /// one command whose whole job is to be read carefully.
    ///
    /// Asserted as an invariant rather than against a golden line: **the second
    /// column begins at the same offset in every row**, whatever is in the
    /// first. That survives someone changing the gutter, the indent or the
    /// marks; a hardcoded expected string survives none of them.
    #[test]
    fn a_column_is_as_wide_as_it_draws_and_not_as_wide_as_its_bytes() {
        let table = Table::new()
            .row(vec![Cell::plain("✓"), Cell::plain("second")])
            .row(vec![Cell::plain("x"), Cell::plain("second")]);

        let out = table.render(&Palette::plain());
        let offsets: Vec<usize> = out
            .lines()
            .map(|l| {
                let byte = l.find("second").expect("every row has a second column");
                l[..byte].chars().count()
            })
            .collect();

        assert_eq!(
            offsets[0], offsets[1],
            "a one-column mark and a one-column letter must leave the second \
             column in the same place — got {out:?}"
        );
    }

    /// Text that already carries escapes is measured on what survives them.
    ///
    /// [`Cell`] keeps style and text apart, so nothing omh writes today lands
    /// here. But `plain` takes any `String`, and the day somebody hands it a
    /// pre-painted one — a harness's own coloured output, quoted back — the
    /// column silently gains however many bytes the escape happened to be.
    /// Alignment must not depend on every future caller having been careful.
    #[test]
    fn an_escape_costs_no_columns() {
        assert_eq!(display_width("\x1b[32mabc\x1b[0m"), display_width("abc"));
        assert_eq!(display_width("abc"), 3);
    }

    /// Columns size to the content, so nothing shears when a value outgrows the
    /// width somebody guessed for it.
    ///
    /// The old output reserved 8 columns for a session id and 14 for a label.
    /// A 9-character id did not truncate — it pushed every later column right
    /// *on that row alone*, which is worse, because the table still looks like
    /// a table and only one line is wrong.
    #[test]
    fn one_long_value_moves_every_row_and_not_just_its_own() {
        let table = Table::new()
            .row(vec![Cell::plain("s01"), Cell::plain("up")])
            .row(vec![
                Cell::plain("a-very-long-session-id"),
                Cell::plain("stopped"),
            ]);

        let out = table.render(&Palette::plain());
        let offsets: Vec<usize> = out
            .lines()
            .map(|l| l.rfind(char::is_whitespace).unwrap() + 1)
            .collect();

        assert_eq!(
            offsets[0], offsets[1],
            "both rows' second column starts at the same offset — got {out:?}"
        );
        assert!(
            out.lines().all(|l| !l.contains("  up  ")),
            "and no row is padded past its last cell"
        );
    }

    /// **JSON is never styled**, even when the user forced colour on.
    ///
    /// `--color always --json` is not a silly combination: it is what a script
    /// inherits when somebody exports `--color always` in a wrapper, or sets
    /// `CLICOLOR_FORCE` for their interactive shell and then runs a cron job.
    /// An escape in that stream is a parse error in `jq`, reported by a user
    /// rather than by us, so the guarantee is structural — `emit` does not give
    /// `json` a palette — and this test is what stops that being refactored
    /// into a convenience later.
    #[test]
    fn forcing_colour_on_cannot_put_an_escape_in_the_machine_format() {
        struct Both;
        impl Report for Both {
            fn human(&self, p: &Palette) -> String {
                p.paint(OK, "green")
            }
            fn json(&self) -> serde_json::Value {
                serde_json::json!({ "state": "green" })
            }
        }

        let loud = Palette::resolve(Color::Always, None, true);
        assert!(
            emit(&Both, Format::Human, &loud).contains('\x1b'),
            "the human format still paints when asked to"
        );
        assert!(
            !emit(&Both, Format::Json, &loud).contains('\x1b'),
            "but the machine format cannot, whatever the palette says"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&emit(&Both, Format::Json, &loud)).is_ok(),
            "and what it emits parses"
        );
    }

    /// **Every link in the cause chain reaches the user**, not just the last
    /// one added.
    ///
    /// This is the guard on the tempting one-liner. `format!("omh: {e}")`
    /// compiles, reads fine, and prints `writing ~/.omh/facts.json` while
    /// silently discarding `No space left on device` — leaving the user with
    /// the one half of the message they could already guess and none of the
    /// half they needed. anyhow keeps the chain; the renderer has to spend it.
    ///
    /// Asserted on the invariant — every context appears somewhere — rather
    /// than on the assembled line, so re-wording `because` or re-indenting the
    /// causes does not go red for no reason.
    #[test]
    fn an_error_reports_every_cause_and_not_merely_the_outermost() {
        let e = anyhow::anyhow!("No space left on device")
            .context("writing /home/u/.omh/facts.json")
            .context("remembering what this image contains");

        let rendered = problem(&Palette::plain(), &e);
        for link in [
            "remembering what this image contains",
            "writing /home/u/.omh/facts.json",
            "No space left on device",
        ] {
            assert!(
                rendered.contains(link),
                "the chain must survive rendering, and {link:?} did not — got {rendered:?}"
            );
        }
        assert!(
            rendered.starts_with("omh:"),
            "and a diagnostic says who is speaking — got {rendered:?}"
        );
    }

    /// The three inputs to colour, and which of them beats which.
    ///
    /// Written as a table because the interesting part is the *precedence*, and
    /// precedence is only visible when the cases sit next to each other. The
    /// one that would otherwise get broken silently is the last: a pipe is not
    /// a terminal, and a user who typed `--color always` into a pipe meant it.
    #[test]
    fn no_color_speaks_for_a_user_who_has_not_said_otherwise() {
        let cases = [
            (
                Color::Auto,
                None,
                true,
                true,
                "a terminal, nothing forbidding",
            ),
            (Color::Auto, None, false, false, "a pipe stays plain"),
            (Color::Auto, Some("1"), true, false, "NO_COLOR beats auto"),
            (
                Color::Auto,
                Some(""),
                true,
                true,
                "but empty NO_COLOR is unset",
            ),
            (
                Color::Always,
                Some("1"),
                false,
                true,
                "an explicit flag beats NO_COLOR",
            ),
            (Color::Never, None, true, false, "and never means never"),
        ];

        for (pref, env, tty, want, why) in cases {
            // Asserted on what the palette *does* rather than on a flag it
            // carries: painting is the only thing a caller can observe, and a
            // predicate that agreed with the flag while `paint` ignored it
            // would pass this test and still print grey.
            let paints = Palette::resolve(pref, env, tty).paint(OK, "x") != "x";
            assert_eq!(
                paints, want,
                "{why}: {pref:?} / NO_COLOR={env:?} / tty={tty}"
            );
        }
    }
}
