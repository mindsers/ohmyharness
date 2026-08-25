//! omh — launch any coding harness, in a sandbox, with your setup already there.
//!
//!     omh claude          omh opencode          omh codex
//!
//! Same rules, same skills, same MCP servers, same memory. The container is not
//! a fourth feature bolted on: it is what makes the other three free, because
//! the profile is *mounted* rather than copied, so there is no drift to fight.

mod adapter;
mod ask;
mod auth;
mod base;
mod bundled;
mod carry;
mod config;
mod container;
mod derive;
mod detect;
mod doctor;
mod editor;
mod facts;
mod hook;
mod idle;
mod image;
mod key;
mod mcp;
mod memory;
mod notice;
mod out;
mod persist;
mod profile;
mod render;
mod report;
mod rules;
mod runtime;
mod selection;
mod session;
mod settings;
mod shadow;
mod ssh;
mod stack;
mod why;

use adapter::Adapter;
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use profile::{Paths, Profile};
use session::Session;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "omh", version, about, long_about = None)]
struct Cli {
    /// Print the launch plan instead of running it.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Reuse an existing session instead of creating a new one.
    #[arg(long, short, global = true)]
    session: Option<String>,

    /// Start a fresh session instead of resuming the most recent one.
    ///
    /// Refused alongside `--session`, which names one: `session::pick` returns
    /// the explicit id and never looks at `new`, so the two together used to
    /// resolve by quietly dropping one of them.
    #[arg(long, global = true, conflicts_with = "session")]
    new: bool,

    /// Which captured account to log in as.
    #[arg(long, short = 'a', global = true)]
    account: Option<String>,

    // Global, which means `omh claude --json` is **refused** rather than
    // forwarded — see `passthrough`. That is the right way round: every omh
    // global is stolen from the harness's argv, and a flag that silently
    // changed which of the two it addressed would be the `--dry-run` bug again.
    // `omh claude -- --json` still reaches the harness.
    //
    // Deliberately *not* a doc comment: clap prints those, and the reader of
    // `--help` is not the reader of this paragraph.
    /// Report as JSON, for a script rather than a person.
    #[arg(long, global = true)]
    json: bool,

    /// When to colour the output.
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto")]
    color: out::Color,

    #[command(subcommand)]
    cmd: Cmd,
}

impl Cli {
    /// How this run reports, decided once.
    ///
    /// Resolved here and passed down rather than consulted where it is used: a
    /// command that asked `is_terminal` twice could paint half its output, and
    /// the half that changed would be whichever half ran after the first write
    /// to a full pipe buffer.
    ///
    /// **`--json` implies no colour** even under `--color always`. The flag
    /// says a program is reading, and `out::emit` already refuses to paint
    /// JSON; making the palette agree means anything a command prints *around*
    /// the report — a warning on stderr, say — does not paint either, which is
    /// what a log scraper on the far end needs.
    fn output(&self) -> (out::Format, out::Palette) {
        if self.json {
            return (out::Format::Json, out::Palette::plain());
        }
        let no_color = std::env::var("NO_COLOR").ok();
        let palette = out::Palette::resolve(
            self.color,
            no_color.as_deref(),
            std::io::IsTerminal::is_terminal(&std::io::stdout()),
        );
        (out::Format::Human, palette)
    }
}

/// Lift a leading `sNN` out of the command line and into the session.
///
/// One form, and three deletions. `omh s diff`, `omh s diff s01`, `omh s -s s01
/// diff` and `omh -s s01 s diff` all meant the same thing, over two mechanisms
/// applied unevenly — `rm` required the positional, `commit` and `push` had no
/// field to read one from, and `push` could not have one because that slot is
/// the branch name.
///
/// The desugaring is literal: `omh s01 diff` is `omh sessions --session s01
/// diff`. When what follows is not a session verb the prefix still names the
/// session and the command runs where it lives, which is what covers the
/// launch — `sessions` has no verb for starting a harness.
///
/// Pure, and separate from `Cli::parse` for that reason: what a command line
/// means is worth testing without a repository, a container or a clock.
///
/// **The parser decides which reading is meant**, rather than this function
/// classifying the token after the prefix. The first version did classify it,
/// and could only see the token in one position: `omh s01 --json diff` — an
/// ordinary thing to type — went to the harness dispatcher as `unknown harness
/// \`diff\``, because a flag is neither a verb nor a harness name. Teaching this
/// function which flags take values would duplicate knowledge the parser
/// already has and would rot the first time a global was added. So both
/// readings are offered to clap and the sessions one is preferred: it loses
/// only when it does not parse and the line as written does, which is the
/// launch. When neither parses, the sessions error is the one worth showing —
/// bare `omh s01` becomes `omh s`, whose error names the verbs.
///
/// The pattern matters more than the list. `s\d+` is what `next_id` generates,
/// so it always matches a real session; `validate_id` refuses an id spelled
/// like a command, so an id that reaches here is one this can safely lift.
/// `--session` stays for a name that is not `sNN` — and because both can be
/// given, `main` refuses a line that names the session twice rather than
/// choosing between them.
fn session_prefix(argv: Vec<String>) -> (Option<String>, Vec<String>) {
    let Some(first) = argv.get(1) else {
        return (None, argv);
    };
    let looks_like_a_session =
        first.len() > 1 && first.starts_with('s') && first[1..].chars().all(|c| c.is_ascii_digit());
    if !looks_like_a_session {
        return (None, argv);
    }

    let as_written: Vec<String> = std::iter::once(argv[0].clone())
        .chain(argv.iter().skip(2).cloned())
        .collect();
    let mut through_sessions = as_written.clone();
    through_sessions.insert(1, "s".to_string());

    // Which reading is meant, decided by what each one parses to rather than by
    // whether it parses at all. `omh s01 commit --keep 1,3` proved the weaker
    // rule wrong when `--keep` was still a flag with no value: the sessions
    // reading was refused by clap and the line as written parsed fine, as a
    // request to launch a harness called `commit`. #56 gave `--keep` a value,
    // so that line now parses both ways — but the rule it established stands
    // for every line that still does not, `omh s01 commit --whatever` among
    // them. A mistyped verb must not become a launch, so a fallback is a
    // launch only when what it names is not a session verb.
    let launch = match (
        Cli::try_parse_from(&through_sessions),
        Cli::try_parse_from(&as_written),
    ) {
        (Ok(_), _) => false,
        // Where the harness boundary is, clap says, not a scan here: `Cmd::Run`
        // is the arm everything omh does not answer to falls into, and its
        // first element is the harness's name.
        (Err(_), Ok(cli)) => match &cli.cmd {
            Cmd::Run(harness) => !harness.first().is_some_and(|name| is_a_session_verb(name)),
            _ => true,
        },
        // Neither reads: the sessions error is the useful one, since a
        // leading `sNN` says the sessions grammar is what was meant. (Bare
        // `omh s01` was this arm's worked example until `omh s` alone became
        // the listing; it now parses, and is decided above.)
        (Err(_), Err(_)) => false,
    };
    (
        Some(first.clone()),
        if launch { as_written } else { through_sessions },
    )
}

/// Whether a word is one of the things a session can be asked to do.
///
/// Read off the parser rather than written out here, the way `omh_globals`
/// reads its flags: a list typed out is one that stops being true the first
/// time a verb is added, and the failure would be silent — a new verb would
/// simply start being read as a harness of the same name.
fn is_a_session_verb(name: &str) -> bool {
    use clap::Subcommand;
    SessionsCmd::augment_subcommands(clap::Command::new("s"))
        .get_subcommands()
        .any(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
}

/// The session the command acts on, from a line that may name it two ways.
///
/// Refused rather than resolved by precedence. `--session` exists to name an id
/// the prefix cannot spell, so a line carrying both is one whose author meant
/// two different sessions, and picking either would act on a session they did
/// not ask for — silently, since both are valid ids.
///
/// After the parse, not before it, because clap is what knows the spellings:
/// `--session s02`, `--session=s02`, `-s s02` and `-ss02` are one flag to it
/// and four strings to a scan over argv. The scan this replaces matched the
/// first two spellings as whole tokens and let the other two through, so the
/// guard fired on the form nobody types and missed the ones they do.
fn the_one_session(prefix: Option<String>, flag: Option<String>) -> Result<Option<String>> {
    if let (Some(prefix), Some(flag)) = (&prefix, &flag) {
        anyhow::bail!(
            "this names the session twice — `{prefix}` and `{flag}`. Name it once:\n  omh {prefix} …"
        );
    }
    Ok(flag.or(prefix))
}

/// omh's own long flags, taken from the parser rather than written out.
///
/// A list would rot: the next global flag added would fall outside the guard
/// below without anyone noticing, which is exactly how the guarded mistake
/// happens in the first place.
fn omh_globals() -> Vec<String> {
    use clap::CommandFactory;
    Cli::command()
        .get_arguments()
        .filter(|a| a.is_global_set())
        .filter_map(|a| a.get_long().map(|long| format!("--{long}")))
        .collect()
}

/// The harness's arguments, refusing any of omh's own flags among them.
///
/// `omh <harness> …` takes everything after the name as the harness's argv, so
/// `omh opencode --dry-run` handed omh's flag to opencode and launched for
/// real. Silent, and worst for exactly the flag whose meaning is "change
/// nothing" — so this refuses rather than warns, and says the form that works.
///
/// Long forms only. `-s` is omh's session flag and is also a flag plenty of
/// harnesses have; refusing shorts would break launches that work today to
/// guard a mistake nobody has made. `--` ends the inspection and is consumed,
/// for the day a harness really does have `--new`.
fn passthrough(argv: &[String], globals: &[String]) -> Result<Vec<String>> {
    let mut out = vec![argv[0].clone()];
    let mut rest = argv[1..].iter();
    for arg in rest.by_ref() {
        if arg == "--" {
            break;
        }
        if globals.iter().any(|g| g == arg) {
            anyhow::bail!(
                "`{arg}` is omh's flag, not {}'s, and everything after a harness \
                 name belongs to the harness\n  \
                 try  omh {arg} {}\n  \
                 or   omh {} -- {arg}   to pass it on regardless",
                argv[0],
                argv[0],
                argv[0]
            );
        }
        out.push(arg.clone());
    }
    out.extend(rest.cloned());
    Ok(out)
}

/// Built-ins and their aliases always beat a harness name — otherwise an
/// adapter called `s` or `config` would silently shadow a command.
pub const RESERVED: [&str; 19] = [
    "init", "doctor", "d", "auth", "ls", "attach", "a", "sessions", "s", "config", "c", "graph",
    "why", "memory", "help", "use", "unuse", "repo", "import",
];

#[derive(Subcommand)]
enum Cmd {
    /// Set this repo up. Decides everything; asks nothing.
    Init,
    /// Verify a harness actually sees the profile, inside a real sandbox.
    #[command(visible_alias = "d")]
    Doctor { harness: Option<String> },
    /// Who put this here, and on what grounds.
    Why {
        /// A base-set entry, something you added, or something omh rejected.
        thing: String,
    },
    /// Open the code graph in your browser.
    ///
    /// **Not per session**, and `omh s01 graph` is refused rather than
    /// answered: every session's graph lives in one volume, so a per-session
    /// server showed every other session's graph anyway. It had its own
    /// positional until the prefix arrived, and for one commit it had both —
    /// `omh s01 graph` set `cli.session`, `graph` read the positional, and the
    /// browser opened on whichever session `pick` chose. The positional went;
    /// the comment claiming the prefix scoped this outlived it by longer.
    Graph {
        /// Stop the graph server; the session keeps running.
        #[arg(long)]
        stop: bool,
    },
    /// Log a harness in once. Repeat with different names for several accounts.
    Auth {
        harness: String,
        /// Account name, e.g. `personal` or `work`.
        #[arg(default_value = auth::DEFAULT_ACCOUNT)]
        account: String,
    },
    /// What you have here: harnesses, editors, sessions.
    Ls,
    /// Open a session in an editor, over SSH.
    #[command(visible_alias = "a")]
    Attach {
        /// Defaults to $OMH_EDITOR or $EDITOR.
        editor: Option<String>,
    },
    /// Work with sessions.
    #[command(visible_alias = "s")]
    Sessions {
        /// `None` is the listing itself — every session, or the one the prefix
        /// named. Optional so that a bare noun is a question rather than a
        /// clap error about a missing verb.
        #[command(subcommand)]
        cmd: Option<SessionsCmd>,
    },
    /// Your defaults and your catalogue, or change them.
    #[command(visible_alias = "c")]
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
    /// This checkout: what it uses, what it decided, and what decided it.
    Repo {
        #[command(subcommand)]
        cmd: Option<RepoCmd>,
    },
    /// Select a catalogue entry for this repo. Writes the committed file: what
    /// a project uses is a fact about the project, and a teammate cloning it
    /// should get the same one.
    Use {
        /// One of rules, skills, mcp, commands, subagents, hooks.
        capability: Option<String>,
        name: Option<String>,
        /// Resync every list to the whole catalogue.
        #[arg(long)]
        all: bool,
    },
    /// Stop using a catalogue entry here.
    Unuse { capability: String, name: String },
    /// The note store: what is in it, and what is wrong with it.
    Memory {
        #[command(subcommand)]
        cmd: Option<MemoryCmd>,
    },
    /// Bring a setup you already have into omh.
    Import {
        capability: String,
        harness: String,
        /// Read this instead of where the adapter says the harness keeps it —
        /// for a config somewhere else, and for seeing what omh would do
        /// without pointing it at your own.
        #[arg(long)]
        from: Option<std::path::PathBuf>,
    },
    /// Anything else is a harness: `omh claude`, `omh opencode`.
    #[command(external_subcommand)]
    Run(Vec<String>),
}

#[derive(Subcommand)]
enum McpCmd {
    /// Servers, with the layer each comes from.
    Ls,
    /// Add a server to your catalogue.
    Add {
        name: String,
        command: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
    },
    /// Remove a server from your catalogue.
    Rm { name: String },
    /// Import servers you already configured in an installed harness.
    Import {
        harness: String,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

/// The verbs. **No `ls`** — `omh s` with no verb at all is the listing, and
/// `omh s01` is that listing scoped to one session.
///
/// `ls` was a verb until 2026.08. What made the scoped row possible is
/// `sessions_ls` learning a scope; retiring the verb is a separate and
/// smaller call, so that one thing has one spelling rather than two.
///
/// It survives below as a hidden tombstone rather than being deleted
/// outright, because deleting it did not make `omh s01 ls` unspellable — only
/// unrefusable. With no `ls` under `sessions`, that line fails the sessions
/// reading and parses as the **top-level** `omh ls`, which never reads
/// `cli.session`: the scope vanishes and every session is listed, looking
/// like it had listed one. Kept here, the sessions reading parses again and
/// wins, so the line is refused by name — and the spelling everyone still has
/// in their fingers gets somewhere to point.
#[derive(Subcommand)]
enum SessionsCmd {
    /// Retired in 2026.08. Kept so that typing it says so — see above, and
    /// `the_retired_listing_verb_is_refused_by_name_rather_than_widening`.
    #[command(hide = true)]
    Ls,
    /// Remove a session — its container and its worktree. A branch holding
    /// commits is kept.
    Rm {
        /// Remove it even though the sandbox holds work no branch has — or
        /// omh could not tell whether it does.
        ///
        /// The refusal without this is the whole point: those commits and
        /// those edits exist nowhere else, and `rm` is what deletes the
        /// repository holding them. This says *I know, and I want them gone*.
        #[arg(long)]
        force: bool,
    },
    /// Stop a sandbox. The worktree and branch survive.
    Down {
        /// Stop every sandbox without being asked.
        ///
        /// With no session named this stops all of them, which is what it is
        /// for and also the one command whose blast radius grows with how much
        /// work is in flight. It asks first — and a script, a pipe or CI has
        /// nobody to answer, so this is how that answer is given in advance.
        #[arg(long)]
        all: bool,
    },
    /// Bring the session up to date with its base branch.
    ///
    /// The merge happens on the host, in your repository, and the sandbox only
    /// ever receives files — no commit of yours may enter it.
    Sync {
        /// Merge against this instead of the repo's default branch.
        #[arg(long)]
        base: Option<String>,
        /// Stop the sandbox first, rather than refusing because it is up.
        #[arg(long)]
        down: bool,
    },
    /// What the agent has committed inside the sandbox, newest first.
    ///
    /// Numbered, so nothing here asks for an object id: the numbers are what
    /// `diff` and `--keep` will take.
    Log {
        /// omh's own snapshot of the tree at the end of each turn, instead of
        /// the agent's commits.
        ///
        /// A separate list on purpose: these are never replanted by `--keep`,
        /// so their numbers are not the ones `diff` and `--keep` take.
        #[arg(long)]
        turns: bool,
    },
    /// What a session changed, against its base branch.
    Diff {
        /// One checkpoint, by its number in `omh sNN log`. Without one, the
        /// whole session.
        checkpoint: Option<usize>,
        /// Defaults to the repo's own default branch.
        ///
        /// Refused alongside a checkpoint number, which is measured against its
        /// own parent: accepting both would take the flag and silently diff
        /// against something else, the same resolve-by-dropping-one this file
        /// already refuses for `--new` and `--session`.
        #[arg(long, conflicts_with = "checkpoint")]
        base: Option<String>,
        /// The patch itself, through your pager, rather than a summary.
        #[arg(long, short = 'p')]
        patch: bool,
    },
    /// Commit a session's work onto its branch. Run on the host: the sandbox's
    /// git is its own repository and cannot reach yours, and the worktree omh
    /// keeps out of your way is not somewhere you should have to go.
    Commit {
        /// The message, verbatim. Without it, git opens your editor.
        #[arg(short = 'm', long)]
        message: Option<String>,
        /// Commit without the files omh carried in from your checkout.
        #[arg(long)]
        skip_carried: bool,
        /// Keep the agent's own commits and messages instead of squashing the
        /// work into one.
        ///
        /// On its own it takes every checkpoint since the last handover, in
        /// order, with no editor. With a selection — `--keep 1,3-4` — it takes
        /// those, in that order, by the numbers `omh sNN log` printed.
        #[arg(long, conflicts_with = "message", num_args = 0..=1, default_missing_value = "")]
        keep: Option<String>,
        /// Open the list in your editor, as `rebase -i` does, to reorder,
        /// reword and drop by hand.
        #[arg(long, requires = "keep", conflicts_with = "message")]
        edit: bool,
        /// Commit even with conflict markers still in the files.
        #[arg(long)]
        force: bool,
    },
    /// Push a session's branch to origin under a name a reviewer can read.
    Push {
        /// The branch name on origin. Required the first time, remembered after.
        name: Option<String>,
        /// Open a pull request with `gh` once it is pushed.
        #[arg(long)]
        pr: bool,
    },
}

/// Two scopes, so two commands. `omh config` narrows to mean **you** — your
/// catalogue and your defaults. `omh repo` means **this checkout**.
///
/// `--layer` used to carry both, and it strained because the two want opposite
/// defaults: what a project *uses* is a fact about the project and should be
/// committed, while what a project *overrides* holds `carry_in` paths and MCP
/// env and must not be committable by accident. One flag cannot express two
/// opposite defaults.
#[derive(Subcommand)]
enum ConfigCmd {
    /// Set one of your defaults, in `~/.omh/settings.toml`.
    Set {
        key: String,
        value: String,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// Remove one of your defaults.
    Unset {
        key: String,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// Open your settings, or one catalogue entry, in $EDITOR.
    Edit {
        /// One of rules, skills, mcp, commands, subagents, hooks. Without it,
        /// your settings file.
        capability: Option<String>,
        /// Which entry. Without it, the capability's directory.
        name: Option<String>,
        #[arg(long, value_parser = parse_layer, hide = true)]
        layer: Option<config::Layer>,
    },
    /// MCP servers — configuration, so it lives here.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Switch one of omh's features on here.
    Enable { feature: String },
    /// Switch one of omh's features off here. Nothing is uninstalled.
    Disable { feature: String },
    /// Set a value for this checkout. Gitignored by default, because these
    /// carry `carry_in` paths and MCP env and a mistyped key must not be
    /// committable by accident.
    Set {
        key: String,
        value: String,
        /// Write the committed file instead, and say so.
        #[arg(long)]
        shared: bool,
    },
    /// Remove a value, letting any lower layer resurface.
    Unset {
        key: String,
        #[arg(long)]
        shared: bool,
    },
}

/// Deliberately short. `promote` and `stale` arrive with the layers and the
/// expiry events they act on; a subcommand that prints "not implemented" is
/// worse than its absence, because `--help` advertises it.
#[derive(Subcommand)]
enum MemoryCmd {
    /// Record what surprised you. Writes to the gitignored layer, always.
    Remember {
        /// What you thought would happen.
        #[arg(long)]
        expected: String,
        /// What actually happened.
        #[arg(long)]
        observed: String,
        /// The command, the error, the file.
        #[arg(long)]
        evidence: String,
        /// A question this note answers, as somebody would later ask it.
        /// Repeat for several. A note nobody can find is a note nobody wrote.
        #[arg(long = "answers")]
        answers: Vec<String>,
        /// Keys of notes this connects to. Keys, not titles: a key is
        /// computable before its target exists.
        #[arg(long = "relates-to")]
        relates_to: Vec<String>,
        /// One of the closed set omh can evaluate itself.
        #[arg(long)]
        invalidated_by: Option<String>,
        /// Who observed it. Defaults to this session when there is one.
        #[arg(long)]
        source: Option<String>,
        /// What to do when the derived key is taken. Skipping is a mode you
        /// ask for, never a fallback — as a fallback every real conflict
        /// disappears silently.
        #[arg(long, value_parser = parse_if_exists, default_value = "error")]
        if_exists: memory::IfExists,
    },
    /// Speak MCP on stdin/stdout. Launched by the harness, not by you.
    ///
    /// Hidden because it is a wire protocol, not a command: it prints JSON-RPC
    /// frames and waits, which is indistinguishable from a hang if you run it
    /// by hand. Paths arrive as arguments because this runs inside the
    /// sandbox, where there is no repo to discover.
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        team: std::path::PathBuf,
        #[arg(long)]
        local: std::path::PathBuf,
        /// The session this server serves. Defaults to `$OMH_SESSION`, which
        /// omh already sets in the sandbox — so the base set can declare
        /// static arguments and still record real provenance.
        #[arg(long)]
        session: Option<String>,
    },
    /// Share a note with the repo: local → team. The only human gate there is.
    Promote {
        /// One or more keys. Notes that link to each other must be named
        /// together, or each would leave the other dangling for a teammate.
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Notes the world has moved on from. A join, never a judgement.
    Stale,
    /// Schema and hygiene violations, across both layers.
    Lint,
    /// Remove one note. Never a neighbour; reports what linked to it.
    Rm {
        key: String,
        #[arg(long, value_parser = parse_note_layer)]
        layer: Option<memory::Layer>,
        /// Which file, when one key somehow reached two of them. Path
        /// relative to the layer's root, as `rm` prints it.
        #[arg(long)]
        at: Option<String>,
    },
}

/// Parse, dispatch, and say what went wrong in omh's own voice.
///
/// Split from [`dispatch`] so a failure has somewhere to be *rendered*. With
/// `main() -> Result<()>` the message was anyhow's `{:?}`, which is a debug
/// format: it leads with `Error:` — a word that names neither the program nor
/// the problem — and it is the one piece of output no user can opt out of
/// seeing. Now it goes through `out::problem`, which knows about the palette
/// and prints the whole cause chain.
fn main() -> std::process::ExitCode {
    // A closed pipe (`omh ls | head`) is not a crash. Without this, Rust's
    // default panics on the failed write and prints a backtrace.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    // One sink for every failure, including the ones that happen before there
    // is a `Cli` to ask about colour — `no_command_writes_to_a_stream_behind_
    // the_output_layer` allows exactly one, and the exemption is this renderer
    // rather than whichever error happened to need it first.
    //
    // The palette starts plain and is replaced the moment the parse resolves
    // one, so a refusal is painted the way every other refusal is.
    let mut palette = out::Palette::plain();
    let outcome = (|| -> Result<()> {
        // The session is lifted out of the command line before clap sees it,
        // so every command below reads it from one place.
        let (named, argv) = session_prefix(std::env::args().collect());
        let mut cli = Cli::parse_from(argv);
        cli.session = the_one_session(named, cli.session.take())?;
        let (format, resolved) = cli.output();
        palette = resolved;
        dispatch(
            &cli,
            &out::Ctx {
                format,
                palette: resolved,
            },
        )
    })();

    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprint!("{}", out::problem(&palette, &e));
            std::process::ExitCode::FAILURE
        }
    }
}

/// Whether this command does anything with the session it was handed.
///
/// The selector lifts a leading `sNN` into `cli.session` before the parser
/// sees the line, so **any** command can arrive carrying one — `omh s01 why
/// codegraph` parses as `omh why codegraph` with the id set. A command that
/// does not read it must refuse the line rather than answer it. It used to
/// answer: a correct report about the repo, exit 0, and the `s01` discarded in
/// silence. That is a wrong answer wearing a right one's clothes, and it is the
/// same defect `omh s01 ls` had, fixed there one spelling at a time.
///
/// **Exhaustively matched on purpose, with no wildcard.** A new command is then
/// a compile error until somebody decides which side it is on — the decision
/// cannot be skipped, and a list that rots cannot be the thing that protects
/// this. `the_reads_and_the_refusals_agree_with_the_dispatch` checks the
/// answers here against what the arms actually pass.
fn consumes_session(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Sessions { .. } | Cmd::Attach { .. } | Cmd::Run(_) => true,
        // Only `remember` writes a note against a session; the rest of the
        // store is repo-wide.
        Cmd::Memory { cmd } => matches!(cmd, Some(MemoryCmd::Remember { .. })),
        // `graph` is per repo, not per session, and says so in its own doc
        // comment — every session's graph lives in one volume. It took
        // `cli.session` and bound it `_id`, which is how it came to claim
        // otherwise in the comment above `Cmd::Graph`.
        Cmd::Graph { .. }
        | Cmd::Init
        | Cmd::Doctor { .. }
        | Cmd::Why { .. }
        | Cmd::Auth { .. }
        | Cmd::Ls
        | Cmd::Config { .. }
        | Cmd::Repo { .. }
        | Cmd::Use { .. }
        | Cmd::Unuse { .. }
        | Cmd::Import { .. } => false,
    }
}

fn dispatch(cli: &Cli, ctx: &out::Ctx) -> Result<()> {
    let cwd = std::env::current_dir()?;

    // Before anything reads it. A scope omh cannot honour is refused where it
    // was named, rather than at whatever depth the handler would have ignored
    // it — and refusing is the safe half: a command taught to read one later
    // turns this into an answer, while a command that silently dropped one
    // never announces that it started mattering.
    if let Some(id) = cli.session.as_deref() {
        anyhow::ensure!(
            consumes_session(&cli.cmd),
            "`{id}` names a session, and this command does not act on one:\n  omh <command>    without the session"
        );
    }

    match &cli.cmd {
        Cmd::Init => init(&cwd, ctx),
        Cmd::Auth { harness, account } => auth_cmd(&cwd, harness, account, ctx),
        Cmd::Ls => ls(&cwd, ctx),
        Cmd::Doctor { harness } => doctor_cmd(&cwd, harness.as_deref(), cli.dry_run, ctx),
        Cmd::Why { thing } => why_cmd(&cwd, thing, ctx),
        Cmd::Graph { stop } => graph(&cwd, *stop, ctx),
        Cmd::Attach { editor } => attach(&cwd, cli.session.as_deref(), editor.as_deref(), ctx),

        // No verb: the listing. With a session named, the same listing scoped
        // to it — the prefix means *this one* everywhere else, and this is the
        // last place it did not.
        Cmd::Sessions { cmd: None } => sessions_ls(&cwd, cli.session.as_deref(), ctx),
        Cmd::Sessions { cmd: Some(cmd) } => match cmd {
            // One source for which session a command acts on, now that the
            // prefix and `--session` both land in `cli.session`. `rm` used to
            // require its own positional and `diff` accepted either, which is
            // how the same question came to have two answers.
            SessionsCmd::Rm { force } => {
                let id = cli.session.as_deref().context(
                    "which session? name it first:\n  omh s01 rm\n  omh s      lists them",
                )?;
                rm(&cwd, id, *force, ctx)
            }
            // Refused rather than aliased to the listing. The verb is gone,
            // and a spelling that silently keeps working is a spelling
            // nobody stops typing.
            SessionsCmd::Ls => anyhow::bail!(
                "there is no `ls` verb any more:\n  omh s      is the listing\n  omh s01    is one row of it"
            ),
            SessionsCmd::Down { all } => down(
                &cwd,
                cli.session.as_deref(),
                *all,
                std::io::IsTerminal::is_terminal(&std::io::stdin()),
                ctx,
            ),
            SessionsCmd::Sync { base, down } => {
                sync(&cwd, cli.session.as_deref(), base.as_deref(), *down, ctx)
            }
            SessionsCmd::Log { turns } => log_cmd(&cwd, cli.session.as_deref(), *turns, ctx),
            SessionsCmd::Diff {
                checkpoint,
                base,
                patch,
            } => diff(
                &cwd,
                cli.session.as_deref(),
                *checkpoint,
                base.as_deref(),
                *patch,
                ctx,
            ),
            SessionsCmd::Commit {
                message,
                skip_carried,
                keep,
                edit,
                force,
            } => commit(
                &cwd,
                cli.session.as_deref(),
                match keep.as_deref() {
                    Some(selection) => Landing::Keep {
                        selection,
                        edit: *edit,
                    },
                    None => Landing::Squash(message.as_deref()),
                },
                *skip_carried,
                *force,
                ctx,
            ),
            SessionsCmd::Push { name, pr } => {
                push(&cwd, cli.session.as_deref(), name.as_deref(), *pr, ctx)
            }
        },

        Cmd::Config { cmd } => match cmd {
            None => show_config(&cwd, ctx),
            Some(ConfigCmd::Set { key, value, layer }) => set(
                &cwd,
                key,
                value,
                layer_or(*layer, config::Layer::Personal, ctx),
                ctx,
            ),
            Some(ConfigCmd::Unset { key, layer }) => unset(
                &cwd,
                key,
                layer_or(*layer, config::Layer::Personal, ctx),
                ctx,
            ),
            Some(ConfigCmd::Edit {
                capability,
                name,
                layer,
            }) => edit(
                &cwd,
                capability.as_deref(),
                name.as_deref(),
                layer_or(*layer, config::Layer::Personal, ctx),
            ),
            Some(ConfigCmd::Mcp { cmd }) => mcp(&cwd, cmd, cli.dry_run, ctx),
        },

        Cmd::Repo { cmd } => match cmd {
            None => show_repo(&cwd, ctx),
            Some(RepoCmd::Enable { feature }) => feature_switch(&cwd, feature, true, ctx),
            Some(RepoCmd::Disable { feature }) => feature_switch(&cwd, feature, false, ctx),
            Some(RepoCmd::Set { key, value, shared }) => {
                set(&cwd, key, value, repo_layer(*shared), ctx)
            }
            Some(RepoCmd::Unset { key, shared }) => unset(&cwd, key, repo_layer(*shared), ctx),
        },

        Cmd::Use {
            capability,
            name,
            all,
        } => use_cmd(&cwd, capability.as_deref(), name.as_deref(), *all, ctx),
        Cmd::Unuse { capability, name } => unuse_cmd(&cwd, capability, name, ctx),

        Cmd::Import {
            capability,
            harness,
            from,
        } => import_cmd(&cwd, capability, harness, from.as_deref(), ctx),

        Cmd::Memory { cmd } => match cmd {
            None => memory_ls(&cwd, ctx),
            Some(MemoryCmd::Lint) => memory_lint(&cwd, ctx),
            Some(MemoryCmd::Stale) => memory_stale(&cwd, ctx),
            Some(MemoryCmd::Promote { keys }) => memory_promote(&cwd, keys, ctx),
            Some(MemoryCmd::Serve {
                team,
                local,
                session,
            }) => memory_serve(team.clone(), local.clone(), session.clone()),
            Some(MemoryCmd::Rm { key, layer, at }) => {
                memory_rm(&cwd, key, *layer, at.as_deref(), ctx)
            }
            Some(MemoryCmd::Remember {
                expected,
                observed,
                evidence,
                answers,
                relates_to,
                invalidated_by,
                source,
                if_exists,
            }) => memory_remember(
                &cwd,
                memory::Remembered {
                    expected: expected.clone(),
                    observed: observed.clone(),
                    evidence: evidence.clone(),
                    answers: answers.clone(),
                    relates_to: relates_to.clone(),
                    invalidated_by: invalidated_by.clone(),
                    source: source.clone().unwrap_or_default(),
                    recorded: memory::today(),
                },
                *if_exists,
                cli.session.as_deref(),
                ctx,
            ),
        },

        // Before `run` looks anything up: which flags are whose is a question
        // about the command line, and answering it after resolving an adapter
        // would report an unknown harness for a mistyped flag.
        Cmd::Run(argv) => run(&cwd, &passthrough(argv, &omh_globals())?, cli, ctx),
    }
}

/// What to tell someone whose word matched nothing. Pure so it can be tested:
/// the message is the entire value of this path.
fn tool_hint(name: &str, harnesses: &[String], editors: &[String]) -> String {
    if editors.iter().any(|e| e == name) {
        return format!("`{name}` is an editor — try `omh attach {name}`");
    }
    if RESERVED.contains(&name) {
        return format!("`{name}` is a command — see `omh {name} --help`");
    }
    format!(
        "unknown harness `{name}`\n  available: {}",
        harnesses.join(", ")
    )
}

/// Neither a harness nor a reserved word — say what is available, since the
/// user cannot tell from the name alone which kind they meant.
fn unknown_tool(paths: &Paths, name: &str, original: anyhow::Error) -> anyhow::Error {
    let harnesses: Vec<String> = Adapter::load_dir(&paths.adapters())
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.name)
        .collect();
    if harnesses.is_empty() {
        return original;
    }
    let editors: Vec<String> = editor::Editor::load_dir(&paths.editors())
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.name)
        .collect();
    anyhow::anyhow!("{}", tool_hint(name, &harnesses, &editors))
}

/// The docker half of `container::reuse`: gather the three facts it decides on.
///
/// One exec, not two. Whether the container can be entered and what is running
/// inside it are the same question asked of the same command — and a container
/// that refuses the exec cannot answer the second, which is why an unreadable
/// probe short-circuits to "replace it" rather than to "nothing is running".
///
/// It short-circuits there only for the failure that means it, though. Every
/// other way the exec can fail is a question omh cannot answer, and answering
/// it wrongly costs an agent its turn — so those refuse.
fn reuse_decision(
    backend: &dyn runtime::Runtime,
    name: &str,
    plan: &container::Plan,
    session: &Session,
) -> Result<container::Reuse> {
    let probe = backend.exec_args(name, &image::probe_command(), false);
    container::decide(
        &session.id,
        image::container_probe(backend.program(), &probe),
        || image::container_stamp(backend.program(), name),
        plan,
    )
}

/// Bring a session's sandbox up if it is not already. A session is a *running
/// container*, not a launch — that is what lets an editor attach to the same
/// place the agent is working.
fn session_up(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    opts: container::Options,
    // The recipe behind `opts.image`. Handed in beside it rather than derived
    // here, so the tag a session runs and the layer that gets built come from
    // one `sandbox()` call and cannot describe different images — the split
    // that let `init` build a layer no launch ever ran.
    recipe: &[&str],
    ctx: &out::Ctx,
) -> Result<(Box<dyn runtime::Runtime>, String)> {
    let backend = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))?;
    let name = paths.container(&session.id);
    let running = must_know(
        image::container_running(backend.as_ref(), &name),
        &session.id,
        "start or reuse it",
    )?;

    // Before planning, because the plan mounts the memory server only if a
    // binary exists. Degraded rather than fatal: a session without memory is
    // still a session, and refusing to launch over it would be the tail
    // wagging the dog — the same rule as a capability a harness cannot express.
    //
    // `ensure` is also what *resolves* the path, rather than the caller's
    // earlier `available()`. That ordering is the whole point and it is easy to
    // lose: on a first launch `available()` answers `None` because the binary
    // has not been cross-built yet, `ensure` then builds it, and a plan holding
    // the earlier answer mounts nothing — after printing that it was building
    // the very thing it goes on to ignore. Owning the field here means no
    // caller can sample it too early.
    let mut opts = opts;
    match memory::deliver::ensure(
        backend.program(),
        paths,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    ) {
        Ok(bin) => opts.memory_bin = Some(bin),
        Err(e) => {
            ctx.warn(&format!("memory server unavailable — {e:#}"));
            opts.memory_bin = None;
        }
    }

    // The account must reach *this* plan: this is the container that actually
    // runs. Building it without credentials is how every session started
    // logged out while `--dry-run` advertised the mounts.
    say_selection(paths, profile, &opts.repo, ctx);
    let plan = container::plan(paths, profile, adapter, session, &[], opts)?;
    plan.validate(&backend.caps())?;

    // The plan is built before this rather than after, because the plan *is*
    // the question: a running container is only this session if it was made
    // from the same one. Cheap — `ensure` above is a path check once the binary
    // is cached, and the staging the plan performs happens every launch anyway.
    if running {
        match reuse_decision(backend.as_ref(), &name, &plan, session)? {
            container::Reuse::Attach => return Ok((backend, name)),
            container::Reuse::Blocked { live, changed } => anyhow::bail!(
                "session {id} is running {} and cannot be reused for this launch \
                 ({})\n  stop it with        omh {id} down\n  \
                 or start a fresh one  omh --new {}",
                live.join(", "),
                changed.join(", "),
                adapter.name,
                id = session.id,
            ),
            container::Reuse::Restart(why) => {
                // `warn`, not `progress`. This destroys a running container,
                // and `progress` is suppressed entirely under `--json` — so
                // the one destructive act in a launch was the one a script
                // could not see.
                //
                // An earlier draft justified that by "every other outcome of
                // this match reaches both formats: `Attach` returns, `Blocked`
                // bails", and neither half survives checking. `Attach` reaches
                // *neither* format — it returns silently and the next thing
                // said is `announce`, which is gated on `Format::Human`.
                // `Blocked` bails, which a script sees as exit 1 and prose on
                // stderr rather than as a field. The argument for `warn` does
                // not need them: a destructive act should be visible wherever
                // omh can speak at all.
                ctx.warn(&format!(
                    "restarting the sandbox for {} — {}",
                    session.label(),
                    why.join(", ")
                ));
                // `?`, not `let _`. `container_remove` bails with docker's
                // own stderr and its comment explains why — and both callers
                // threw that away. The launch then failed further down against
                // the same sick daemon, and the user read `the container name
                // is already in use` with nothing connecting it to the restart
                // they were just told about.
                image::container_remove(backend.program(), &name)
                    .with_context(|| format!("replacing the sandbox for {}", session.id))?;
            }
        }
    }

    say_rules(&plan, ctx);
    image::ensure_stack(backend.program(), adapter, recipe, &paths.repo)?;
    image::ensure_network(backend.program(), &plan.network)?;

    let key = ssh::ensure_key(&paths.keys())?;
    let pubkey = std::fs::read_to_string(key.with_extension("pub"))?;
    let port = ssh::port(&paths.repo_name(), &session.id);

    let _ = image::container_remove(backend.program(), &name); // a stopped one blocks --name
    let args = backend.up_args(&plan, &name, port, pubkey.trim());
    let out = Command::new(backend.program()).args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "starting session {}: {}",
            session.id,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // The session's worktree is not the checkout indexed at init — it holds
    // whatever the agent has since written. Index it now; the Stop hook keeps
    // it current from here.
    let project = base::project_name(&paths.repo_name(), &session.id);
    let _ = Command::new(backend.program())
        .args(backend.exec_args(
            &name,
            &[
                base::GRAPH_BIN.into(),
                "cli".into(),
                "index_repository".into(),
                "--repo-path".into(),
                container_workdir().into(),
                "--name".into(),
                project,
                "--mode".into(),
                "fast".into(),
            ],
            false,
        ))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    Ok((backend, name))
}

fn attach(
    cwd: &std::path::Path,
    id: Option<&str>,
    chosen: Option<&str>,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
        .context("no adapters installed — run `omh init`")?;
    let adapter = Adapter::find(&paths.adapters(), &harness)?;
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        sandbox.top_up(
            &paths,
            backend.program(),
            &adapter,
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
            ctx,
        )?;
    }

    std::fs::create_dir_all(paths.worktrees())?;
    let id = session::pick(&paths.worktrees(), id, false);
    let session = Session::new(&paths.worktrees(), id);
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;
    carry_in(&paths, &session, ctx)?;
    let _ = idle::touch(&paths.runs(), &session.id);

    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, None, configured.as_deref())?
        .map(|a| auth::dir(&paths, &adapter.name, &a));
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }

    // Said here, because `attach` is the one launch path that never said it.
    // `run` carries the drop list in its status line, built from the plan it
    // makes itself; `session_up` builds its own plan and discards it, so
    // `omh code` staged a hooks document with hooks removed and reported
    // nothing — and this is the path where it matters most, for the reason
    // `say_selection` gives: it is how you rejoin a session whose setup you
    // have since changed.
    //
    // Through `render::held_back`, so the wording and the set are `init`'s.
    for d in render::held_back(
        &profile.sources(adapter::Capability::Hooks)?,
        &own,
        &repo,
        &sandbox.resolves,
    )? {
        ctx.warn(&format!("`{}` needs {} — held back", d.name, d.wanted));
    }

    session_up(
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: false,
            account_dir: account,
            memory_bin: memory::deliver::available(&paths),
            base: Some(session::default_branch(&paths.repo)),
            omh: own,
            repo,
            image: sandbox.tag.clone(),
            resolves: sandbox.resolves.clone(),
        },
        &sandbox.recipe(),
        ctx,
    )?;

    // The integration point is a managed SSH config include, not an IDE plugin —
    // that is what keeps every editor working without omh knowing about any.
    let home = dirs::home_dir().context("no home directory")?;
    let alias = ssh::host_alias(&paths.repo_name(), &session.id);
    let key = ssh::ensure_key(&paths.keys())?;
    let blocks: Vec<String> = session::list(&paths.worktrees())
        .into_iter()
        .map(|s| {
            ssh::config_block(
                &ssh::host_alias(&paths.repo_name(), &s),
                ssh::port(&paths.repo_name(), &s),
                &key,
            )
        })
        .collect();
    ssh::write_hosts(&home.join(".ssh/config.d/omh"), &blocks)?;
    ssh::ensure_include(&home.join(".ssh/config"))?;

    let fallback = std::env::var("OMH_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .and_then(|e| {
            let base = std::path::Path::new(&e)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some(base)
        });
    let wanted = chosen.map(str::to_string).or(fallback);
    let ed = wanted
        .as_deref()
        .and_then(|n| editor::Editor::find(&paths.editors(), n));

    let editors: Vec<(String, String)> = editor::Editor::load_dir(&paths.editors())?
        .into_iter()
        .map(|e| (e.name.clone(), e.command(&alias).join(" ")))
        .collect();

    // Which editor, if any, actually got a window open. Everything else about
    // the report is the same either way — the URL and the `ssh` line are how
    // you rejoin this session tomorrow, whether or not something opened today.
    let opened_in = match ed {
        // An editor that is not installed is not an error — the URL is still a
        // good answer, and launching nothing silently would not be.
        Some(ed) if runtime::installed(&ed.bin) => {
            let cmd = ed.command(&alias);
            let ok = Command::new(&cmd[0])
                .args(&cmd[1..])
                .status()
                .map(|s| s.success());
            if matches!(ok, Ok(true)) {
                Some(ed.name.clone())
            } else {
                // Remote launches fail for ordinary reasons — missing
                // extension, handshake refused. Saying nothing leaves the user
                // waiting for a window that will never open.
                ctx.warn(&format!("{} did not open the session", ed.name));
                None
            }
        }
        other => {
            if let Some(ed) = other {
                ctx.warn(&format!("`{}` is not installed on this machine", ed.bin));
            } else if let Some(w) = &wanted {
                ctx.warn(&format!("no editor named `{w}` — see `omh ls`"));
            }
            None
        }
    };

    ctx.say(&report::Attached {
        session: session.id.clone(),
        url: ssh::url(&alias),
        alias,
        opened_in,
        editors,
    });
    Ok(())
}

/// Serve the graph UI from the session and open it.
///
/// Started on demand rather than always: the port is reserved when the session
/// is created (it has to be), but a process nobody looks at is waste.
/// The graph UI, once per repo.
///
/// Not per session: every session's graph lives in one volume, so a per-session
/// server showed every other session's graph anyway. Matching the server's
/// scope to its data's scope removes the duplication, survives sessions coming
/// and going, and lets the container mount only the index.
fn graph(cwd: &std::path::Path, stop: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    let container = base::ui_container(&paths.repo_name());

    if stop {
        if !must_know(
            image::container_running(backend.as_ref(), &container),
            "the graph",
            "stop it",
        )? {
            ctx.say(
                &report::Action::new("graph-not-running", "the graph is not running")
                    .data(serde_json::json!({ "running": false })),
            );
            return Ok(());
        }
        image::container_remove(backend.program(), &container)?;
        ctx.say(
            &report::Action::new("graph-stopped", "graph stopped; sessions keep running")
                .data(serde_json::json!({ "running": false })),
        );
        return Ok(());
    }

    let port = base::ui_port(&container);
    if !must_know(
        image::container_running(backend.as_ref(), &container),
        "the graph",
        "start it",
    )? {
        // A stopped container of the same name blocks `run --name`.
        let _ = image::container_remove(backend.program(), &container);

        let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
            .into_iter()
            .map(|a| a.name)
            .collect();
        let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
            .context("no adapters installed — run `omh init`")?;
        let adapter = Adapter::find(&paths.adapters(), &harness)?;
        image::ensure(backend.program(), &adapter)?;

        let out = Command::new(backend.program())
            .args(base::ui_run_args(
                &image::tag_for(&adapter),
                &container,
                &paths.cache_volume(),
                port,
            ))
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "could not start the graph: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }

    let url = format!("http://127.0.0.1:{port}");
    ctx.say(
        &report::Action::new("graph-started", format!("graph at {url}"))
            .next("omh graph --stop")
            .data(serde_json::json!({ "url": url, "port": port, "running": true })),
    );
    ctx.hint("every session's graph for this repo, in one place");
    let _ = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(&url)
    .status();
    Ok(())
}

/// Stop sessions nobody has used for longer than `policy.idle_timeout`.
///
/// N sessions is N containers — the sprawl `docs/design/risks.md` names. Only
/// the container stops; the worktree and branch survive, so relaunching resumes
/// exactly where you left off.
///
/// Best-effort by design: this runs on the way to starting a session, and a
/// failure to reap must never stop you working.
fn reap_idle(paths: &Paths, launching: &str, ctx: &out::Ctx) {
    let Some(raw) = policy_value(paths, "idle_timeout") else {
        return;
    };
    let Some(timeout) = idle::parse_duration(&raw) else {
        // Say so rather than ignoring silently — a setting that resolves with
        // provenance and then does nothing is exactly what this feature was.
        ctx.warn(&format!(
            "ignoring idle_timeout `{raw}` — expected a duration like 30m, 2h, 90s"
        ));
        return;
    };
    let Ok(backend) = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))
    else {
        return;
    };

    let running: Vec<(String, Option<std::time::SystemTime>)> = session::list(&paths.worktrees())
        .into_iter()
        .filter(|id| {
            reapable(&image::container_running(
                backend.as_ref(),
                &paths.container(id),
            ))
        })
        .map(|id| {
            let last = idle::last_used(&paths.runs(), &id);
            (id, last)
        })
        .collect();

    for id in idle::expired(&running, timeout, std::time::SystemTime::now(), launching) {
        match image::container_remove(backend.program(), &paths.container(&id)) {
            Ok(()) => ctx.progress(&format!(
                "stopped {id} — idle over {raw} (worktree and branch survive)"
            )),
            Err(e) => ctx.warn(&format!("could not stop idle session {id}: {e}")),
        }
    }
}

fn down(
    cwd: &std::path::Path,
    id: Option<&str>,
    all: bool,
    terminal: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    let ids = match id {
        Some(i) => vec![i.to_string()],
        None => {
            let every = session::list(&paths.worktrees());
            // Nothing to stop is not a question worth asking.
            if !all && !every.is_empty() {
                // On stderr, like every other prompt here: the answer channel
                // belongs to the report, and `omh s down > log` must not eat
                // the question.
                let agreed = terminal
                    && ask::confirm(
                        &format!("stop every sandbox? {} — {}", every.len(), every.join(", ")),
                        &mut std::io::stdin().lock(),
                        &mut std::io::stderr(),
                    )?;
                anyhow::ensure!(
                    agreed,
                    "nothing stopped:\n  omh s01 down       that one\n  omh s down --all   every one of them"
                );
            }
            every
        }
    };
    // Collected, then said once: with no id this is asked about every session,
    // and one `say` per session is one JSON document per session.
    let mut sessions = Vec::new();
    let mut stuck = 0usize;
    let mut unasked = 0usize;
    for i in &ids {
        let name = paths.container(i);
        match image::container_running(backend.as_ref(), &name) {
            image::Running::No => {
                sessions.push((i.clone(), report::Stopped::WasNotRunning));
                continue;
            }
            // A row in the report, not a warning and a gap. Skipping the push
            // meant `down` over an unreachable daemon printed `no sessions` on
            // **stdout** and `"sessions": []` in JSON — a false all-clear on
            // the answer channel, which is the thing this whole change is
            // about, one report struct over.
            //
            // Counted apart from `stuck` too: omh never asked this one to
            // stop, so *would not stop* is a claim it cannot make.
            image::Running::Unknown(why) => {
                unasked += 1;
                ctx.warn(&format!("could not tell whether {i} is running: {why}"));
                sessions.push((i.clone(), report::Stopped::CouldNotTell(why)));
                continue;
            }
            image::Running::Yes => {}
        }
        match image::container_remove(backend.program(), &name) {
            Ok(()) => sessions.push((i.clone(), report::Stopped::Yes)),
            // Reported and carried on rather than returned: one container that
            // will not go must not hide the ones that did. It still decides
            // the exit code below — a caller whose JSON says nothing stopped
            // needs the status to agree.
            Err(e) => {
                stuck += 1;
                ctx.warn(&format!("{i} is still running: {e:#}"));
            }
        }
    }
    ctx.say(&report::Down { sessions });
    // Both are failures and they are different sentences. The old text called
    // a session omh never asked about one that "would not stop".
    anyhow::ensure!(
        stuck == 0 && unasked == 0,
        "{}",
        [
            (stuck, "would not stop"),
            (unasked, "could not be asked — the runtime did not answer"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} session{} {what}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join("; ")
    );
    Ok(())
}

/// Launch the real image with the real mounts and ask the harness's own paths
/// what they can see. Nothing in process can answer this: a green unit suite
/// proves omh mounts a path, never that anything reads it.
fn doctor_cmd(
    cwd: &std::path::Path,
    harness: Option<&str>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let name = match harness {
        Some(h) => h.to_string(),
        None => {
            let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
                .into_iter()
                .map(|a| a.name)
                .collect();
            detect::preferred_harness(&names, &|h| runtime::installed(h))
                .context("no adapters installed — run `omh init`")?
        }
    };
    let adapter = Adapter::find(&paths.adapters(), &name)?;

    // Credentials are the half no in-process test can reach: whether a token
    // saved here survives depends on how the runtime binds the path.
    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, None, configured.as_deref())
        .unwrap_or(None)
        .map(|a| auth::dir(&paths, &name, &a));

    // Resolved once and used for both the checks and the plan below, so the
    // probe cannot check a session different from the one it launches.
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        sandbox.top_up(
            &paths,
            backend.program(),
            &adapter,
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
            ctx,
        )?;
    }
    let mut checks = doctor::checks(&profile, &adapter, &own, &repo, &sandbox.resolves)?;
    if account.is_some() {
        checks.extend(doctor::credential_checks(&adapter));
    }
    // Only if the resolved profile actually declares it: a check for a server
    // nobody configured would fail honestly and mean nothing.
    //
    // Read through `render::parse_layers` rather than `config::servers`, which
    // returns only each server's *command* — the arguments are what say which
    // directories it will look in, and those are the whole point of the check.
    let declared = render::parse_layers(&profile.sources(adapter::Capability::Mcp)?)?;
    // Not when this repo has switched the feature off: the server is left out
    // of the document on purpose, so checking for it is checking a claim omh
    // deliberately did not make.
    if let Some(server) = declared
        .get(memory::tools::SERVER_KEY)
        .filter(|_| !repo.disabled_servers.contains(memory::tools::SERVER_KEY))
    {
        checks.extend(doctor::memory_checks(server));
    }
    if checks.is_empty() {
        ctx.say(
            &report::Action::new(
                "doctor-nothing-to-check",
                "nothing to check: the profile is empty",
            )
            .data(serde_json::json!({ "harness": name, "checks": 0 })),
        );
        return Ok(());
    }

    let session = Session::scratch(paths.scratch("doctor"), "doctor".into());
    session.ensure(&paths.repo, "")?;

    let opts = container::Options {
        staging: container::Staging::Apply,
        // No dtach and no terminal: the probe's output has to be captured.
        persist: persist::Mode::None,
        tty: false,
        account_dir: account.clone(),
        memory_bin: memory::deliver::available(&paths),
        // The probe has to compose the same rules a launch would, or it proves
        // the harness reads a document nobody will be given.
        base: Some(session::default_branch(&paths.repo)),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }
    say_selection(&paths, &profile, &opts.repo, ctx);
    let mut plan = container::plan(&paths, &profile, &adapter, &session, &[], opts)?;
    say_rules(&plan, ctx);
    plan.argv = vec!["sh".into(), "-c".into(), doctor::probe_script(&checks)];

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    if dry_run {
        // The script itself, unwrapped: this output exists to be piped into a
        // shell or read line by line, and a report around it would have to be
        // stripped back off. `Probe` says so in one place instead of here.
        ctx.say(&report::Probe {
            script: doctor::probe_script(&checks),
            checks: checks.iter().map(|c| c.name.clone()).collect(),
        });
        return Ok(());
    }

    image::ensure_stack(backend.program(), &adapter, &sandbox.recipe(), &paths.repo)?;
    image::ensure_network(backend.program(), &plan.network)?;

    let account_name = account
        .as_ref()
        .map(|a| a.file_name().unwrap_or_default().to_string_lossy().into());
    ctx.progress(&match &account_name {
        Some(a) => format!("checking {name} in {} as {a}…", sandbox.tag),
        None => format!(
            "checking {name} in {} — no account, so credentials go unchecked…",
            sandbox.tag
        ),
    });

    let out = Command::new(backend.program())
        .args(backend.args(&plan))
        .output()?;
    let from_the_sandbox = doctor::parse(&String::from_utf8_lossy(&out.stdout));
    let _ = session.remove(&paths.repo, "", &paths.shadows()); // diagnostic: leave no session behind
                                                               // `with_context` would make the sandbox's stderr the *outer* error, so
                                                               // `out::problem` would print it as omh's own headline and demote omh's
                                                               // explanation to a cause — with an empty stderr rendering as a bare
                                                               // `omh:` and nothing after it. The sentence omh wrote stays first, and
                                                               // what the container said follows it, sanitised: it is not omh's text.
    let outcomes = every_check(from_the_sandbox).map_err(|e| {
        match crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim()) {
            said if said.is_empty() => e,
            said => anyhow::anyhow!("{e}\n{said}"),
        }
    })?;

    let report = report::Doctor {
        harness: name,
        tag: sandbox.tag.clone(),
        account: account_name,
        outcomes,
    };
    ctx.say(&report);
    if !report.passed() {
        anyhow::bail!(
            "{} of {} checks failed",
            report.failed(),
            report.outcomes.len()
        );
    }
    Ok(())
}

fn sessions_ls(cwd: &std::path::Path, only: Option<&str>, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    // Validated before anything is read, so an id nothing created fails the
    // way it fails for every other verb rather than listing nothing and
    // looking like an answer.
    if let Some(id) = only {
        existing_session(&paths, Some(id))?;
    }
    // Said once, about the machine, rather than once per row. The `running`
    // column renders a `None` as an absence — nobody asked — and *why* nobody
    // asked is one fact, not N.
    let backend = match runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        Ok(backend) => Some(backend),
        Err(e) => {
            ctx.warn(&format!("omh cannot say which sandboxes are up: {e:#}"));
            None
        }
    };
    let base = session::default_branch(&paths.repo);

    // What each session is changing, asked **once** and used twice. The count
    // `work_state` renders and the paths the overlap section names are the
    // same answer: read separately they were two subprocesses per session and,
    // worse, two snapshots — a live agent writes between them, so one listing
    // could report `0 uncommitted` beside a collision on a file that session
    // had just stashed.
    let mut changed: Vec<(String, Vec<String>)> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let sessions: Vec<report::Session> = session::list(&paths.worktrees())
        .into_iter()
        .map(|id| {
            let sess = Session::new(&paths.worktrees(), id.clone());
            let touched = sess.changed();
            match &touched {
                Ok(touched) => changed.push((id.clone(), touched.clone())),
                // Carried, not dropped. A session omh cannot read contributes
                // no paths, so it silently belongs to no collision — and *no
                // overlap line* is exactly how "no collisions" is rendered.
                // `work_state` renders the same failure as `Work::Unknown` one
                // column over; the two must not disagree about whether it is
                // worth mentioning.
                Err(_) => unreadable.push(id.clone()),
            }
            report::Session {
                // `None` for *nobody asked*, which is what no backend at all
                // means, and `Unknown` inside it for *asked and could not
                // tell*. The same two-level shape `work` uses one column over,
                // and for the same reason it gives: the mistake becomes
                // unspellable rather than merely absent.
                running: backend.as_ref().map(|b| {
                    let asked = image::container_running(b.as_ref(), &paths.container(&id));
                    // Say why, which the first version of this did not — it
                    // built the reason, carried it through two layers and
                    // dropped it, while both docs promised it reached stderr.
                    // The same defect as the `.ok()` thirteen lines below,
                    // introduced in the change that fixed that one.
                    if let image::Running::Unknown(why) = &asked {
                        ctx.warn(&format!(
                            "could not tell whether {id}'s sandbox is running: {why}"
                        ));
                    }
                    asked
                }),
                label: sess.label().to_string(),
                work: Some(work_state(
                    &sess,
                    &paths.repo,
                    &base,
                    touched.as_ref().ok().map(Vec::len),
                )),
                // Not `.ok()`. The dashboard renders a failed count as a
                // question rather than as zero, which is only half the rule —
                // the other half is that the reason exists and git already
                // said it. `changed()` ten lines up pushes to `unreadable` for
                // the same class of failure, and `log` prints git's own words;
                // this was the one that threw them away.
                behind: match sess.behind(&paths.repo, &base) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        ctx.warn(&format!(
                            "could not tell how far behind {base} {id} is: {e:#}"
                        ));
                        None
                    }
                },
                id,
            }
        })
        .collect();

    // Every session is read even when one is asked for, and that is not
    // waste: a collision is a fact about *two* sessions, so the paths of the
    // others are what make "s01 and s03 both change src/render.rs" sayable at
    // all. What narrows is the display.
    //
    // (An earlier note claimed focusing would be cheaper. It is not, for this
    // reason.)
    let overlaps = report::overlaps(&changed);
    let (sessions, overlaps) = match only {
        None => (sessions, overlaps),
        Some(id) => {
            let rows: Vec<_> = sessions.into_iter().filter(|s| s.id == id).collect();
            // `existing_session` said this id was there, and the filter says
            // it is not. The two ask different questions — one whether the
            // path exists, the other whether it is a directory — and between
            // them sit every subprocess this function runs, so a worktree
            // removed in another terminal lands here too. Rendering it would
            // print `no sessions`: exit 0, and byte-identical to a clean
            // checkout. Refusing says which of the two we could not reconcile.
            anyhow::ensure!(
                !rows.is_empty(),
                "{id} was there when omh looked and is not there now — \
                 removed while this ran? `omh s` lists what is left"
            );
            (
                rows,
                // Kept when they name this session, dropped otherwise — a
                // collision between two other sessions is not this one's
                // business, and one involving it is the most useful line on
                // the screen.
                overlaps
                    .into_iter()
                    .filter(|o| o.sessions.iter().any(|s| s == id))
                    .collect(),
            )
        }
    };

    ctx.say(&report::Sessions {
        sessions,
        // Not swept when one session was asked for. A leftover is an id with
        // no worktree, and the focused id was proved to have one, so a
        // focused sweep can only ever turn up other people's — guaranteed
        // off-topic rather than merely usually. The overlap section is the
        // opposite case and stays: a collision *is* a fact about this
        // session. Skipping it also saves the sweep's `ps` and its walks.
        leftovers: match only {
            None => leftovers(&paths, backend.as_deref(), ctx),
            Some(_) => Vec::new(),
        },
        overlaps,
        // Deliberately *not* narrowed. A session omh could not read is why
        // the overlap answer above may be short a line, and that is a fact
        // about the focused session even though the id named is not.
        unreadable,
        base,
    });
    Ok(())
}

/// Session ids with a container, a run directory or a sandbox repository but
/// no worktree.
///
/// Invisible until now, and not merely untidy: an orphan container holds a
/// session id, and the next session to take that id used to exec straight into
/// it. That was the mount-namespace failure. `s rm` cleans up after itself now,
/// so this reports what older versions left — and anything a hand
/// `git worktree remove` strands from here on.
///
/// A run directory counts only when it carries the marker `idle::touch` writes.
/// `omh doctor` and `omh auth` stage into the same tree under their own names,
/// and neither is a session anybody could resume or would want reported.
fn leftovers(paths: &Paths, backend: Option<&dyn runtime::Runtime>, ctx: &out::Ctx) -> Vec<String> {
    let live = session::list(&paths.worktrees());
    // A sandbox repository with no worktree — [risks](docs/design/risks.md) 8c.
    // The most valuable orphan of the three: a container is re-creatable and a
    // run directory holds a timestamp, while this holds every commit the agent
    // made and nothing points at it. `omh <id> rm` clears it, and since #58
    // says what it would take with it first.
    // `NotFound` is the ordinary "no sandbox has ever been built here". Any
    // other failure is omh being unable to look, and an empty `leftovers`
    // prints *nothing at all* — byte for byte what a clean checkout prints. Of
    // the three orphans this hunts, the repository is the one that holds work.
    let mut found: Vec<String> = match std::fs::read_dir(paths.shadows()) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".git").map(str::to_string)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            ctx.warn(&format!(
                "omh could not read {}, so orphaned sandbox repositories went unchecked: {e}",
                paths.shadows().display()
            ));
            Vec::new()
        }
    };
    found.extend(
        std::fs::read_dir(paths.runs())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| idle::last_used(&paths.runs(), id).is_some()),
    );

    if let Some(backend) = backend {
        let prefix = paths.container("");
        if let Ok(out) = Command::new(backend.program())
            .args(["ps", "-a", "--format", "{{.Names}}"])
            .output()
        {
            found.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|n| n.trim().strip_prefix(&prefix))
                    .map(str::to_string),
            );
        }
    }

    found.retain(|id| !live.contains(id));
    found.sort();
    found.dedup();
    found
}

/// Where a session is in the cycle, phrased as the next thing to do about it.
///
/// Ordered most-actionable first, and deliberately one answer rather than a
/// tally: `omh s` is read at a glance, and a session with uncommitted work needs
/// committing whatever else is also true of it.
fn work_state(
    session: &Session,
    repo: &std::path::Path,
    base: &str,
    uncommitted: Option<usize>,
) -> report::Work {
    use report::Work;

    // A git that cannot answer is never rendered as an answer. Every accessor
    // below runs through the worktree's `.git` pointer, which goes stale when a
    // checkout moves and is already handled as a real case by `Session::remove`
    // — and a blank column reads as "nothing here" for a session that may be
    // holding a day of work the user is about to `s rm`.
    // The count comes from the caller, which already asked. `None` is the same
    // failure this used to discover for itself.
    let (uncommitted, unpushed) = match (uncommitted, session.unpushed()) {
        (Some(uncommitted), Ok(unpushed)) => (uncommitted, unpushed),
        _ => return Work::Unknown,
    };

    if let n @ 1.. = uncommitted {
        return Work::Uncommitted(n);
    }
    match unpushed {
        Some(n @ 1..) => Work::ToPush(n),
        // Nothing origin does not already have. Report the name it went out
        // under, which is what you would look for in a list of PRs — `omh/s01`
        // is not a name anybody searches for.
        Some(_) => match session.published_as() {
            Ok(Some(target)) => Work::Published(target),
            Ok(None) => Work::Clean,
            Err(_) => Work::Unknown,
        },
        // Never pushed, which is not the same as nothing to push: this is the
        // state the loop passes through every time, between `s commit` and the
        // first `s push`. Measured against the base branch instead, because a
        // blank here reads as a session nobody touched.
        None => match session.commits(repo, base) {
            Ok(0) => Work::Clean,
            Ok(n) => Work::ToPush(n),
            // The same rule as the accessors above: a git that cannot answer is
            // never rendered as an answer. `Clean` here would read as a session
            // holding nothing, over a branch nobody can count.
            Err(_) => Work::Unknown,
        },
    }
}

/// Read one policy key through the usual layer merge.
fn policy_value(paths: &Paths, key: &str) -> Option<String> {
    config::policy(paths)
        .ok()?
        .into_iter()
        .find(|s| s.key == key)
        .map(|s| s.value)
}

fn runtime_preference(paths: &Paths) -> String {
    policy_value(paths, "runtime").unwrap_or_else(|| "auto".into())
}

fn parse_layer(s: &str) -> std::result::Result<config::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
}

/// A note's layer, which is a different set from a profile's: notes have no
/// personal layer, and the two they do have never merge.
fn parse_note_layer(s: &str) -> std::result::Result<memory::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
}

/// The agent's working directory inside the sandbox. Named once, so the note
/// store and the launch plan cannot disagree about it.
pub fn container_workdir() -> &'static str {
    "/work"
}

fn parse_if_exists(s: &str) -> std::result::Result<memory::IfExists, String> {
    match s {
        "error" => Ok(memory::IfExists::Error),
        "skip" => Ok(memory::IfExists::Skip),
        "suffix" => Ok(memory::IfExists::Suffix),
        "override" => Ok(memory::IfExists::Override),
        other => Err(format!(
            "unknown --if-exists `{other}` (error, skip, suffix, override)"
        )),
    }
}

/// Record an observation. The key is derived, never chosen: an agent that picks
/// its own cannot be stopped from recording one event twice.
fn memory_remember(
    cwd: &std::path::Path,
    mut input: memory::Remembered,
    if_exists: memory::IfExists,
    session: Option<&str>,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if input.source.trim().is_empty() {
        // Provenance is omh's to supply, so that it cannot be omitted. On the
        // CLI there may be no session, and saying `cli` is honest where
        // inventing a session id would not be.
        input.source = match session {
            Some(id) => format!("session {id}, cli"),
            None => "cli".into(),
        };
    }
    ctx.say(&match memory::remember(&paths, &input, if_exists)? {
        memory::Wrote::Created(path) => {
            report::Action::new("note-recorded", format!("recorded {}", path.display()))
                .data(serde_json::json!({ "path": path.display().to_string(), "replaced": false }))
        }
        // Said out loud: a note that existed is gone, and only `--if-exists
        // override` gets here, so the caller asked for it and can check.
        memory::Wrote::Replaced(path) => report::Action::new(
            "note-replaced",
            format!(
                "replaced {} — the note that was there is gone",
                path.display()
            ),
        )
        .data(serde_json::json!({ "path": path.display().to_string(), "replaced": true })),
        memory::Wrote::Skipped(key) => report::Action::new(
            "note-already-there",
            format!("`{key}` is already recorded; left alone"),
        )
        .data(serde_json::json!({ "key": key, "replaced": false })),
    });
    Ok(())
}

/// Write one note per tracked document, plus one for what `init` derived.
///
/// Into the **committed** layer: a stub is reproducible from a document every
/// teammate already has, so it is not a claim from experience and does not need
/// a human to vouch for it. `promote` stays reserved for what an agent
/// observed.
fn seed_store(paths: &Paths) -> Result<String> {
    let templates = memory::templates(paths)?;
    let today = memory::today();
    let dir = memory::Layer::Team.dir(paths);

    let mut written = 0;
    let mut skipped = 0;
    let mut stubs = Vec::new();
    for doc in memory::ingest::documents(&paths.repo)? {
        let note = memory::ingest::stub(&doc, &templates, &today)?;
        stubs.push(note.key.clone());
        match memory::ingest::write(&dir, &note, memory::IfExists::Skip)? {
            true => written += 1,
            false => skipped += 1,
        }
    }

    let seeds = detect::seeds(
        &stack::load_all(&paths.stacks(), &paths.repo_stacks())?,
        &paths.repo,
    );
    if let Some(note) =
        memory::ingest::overview(&paths.repo_name(), &seeds, &stubs, &templates, &today)?
    {
        if memory::ingest::write(&dir, &note, memory::IfExists::Skip)? {
            written += 1;
        } else {
            skipped += 1;
        }
    }

    if written == 0 && skipped == 0 {
        return Ok("nothing to derive yet".into());
    }
    Ok(format!(
        "{written} note{} written, {skipped} already there",
        if written == 1 { "" } else { "s" }
    ))
}

/// Speak MCP until stdin closes.
///
/// Nothing but protocol may reach stdout, and one stray line breaks the very
/// first handshake. Every other command now writes through `out::Ctx`, which
/// puts answers on stdout and diagnostics on stderr — so the rule this comment
/// used to enforce by vigilance is enforced by the type. This function is the
/// exception that still owns its own stdout, because what it writes there is
/// not a report at all.
fn memory_serve(
    team: std::path::PathBuf,
    local: std::path::PathBuf,
    session: Option<String>,
) -> Result<()> {
    let mut server = memory::tools::Server {
        team,
        local,
        templates: memory::shipped_templates(),
        // omh already sets `OMH_SESSION` in the sandbox, so the base set can
        // declare static arguments and still record real provenance.
        session: session
            .or_else(|| std::env::var("OMH_SESSION").ok())
            .unwrap_or_else(|| "unknown".into()),
        client: None,
        today: memory::today,
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    mcp::serve(stdin, stdout, &mut server)
}

/// A join against facts omh already holds. Three groups, because they are
/// three different claims: the world moved, omh cannot tell, and omh was never
/// asked to tell.
fn memory_stale(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    use memory::expiry::Verdict;
    let paths = Paths::discover(cwd)?;
    let judged = memory::expiry::judge(&paths, &memory::load(&paths)?)?;

    /// Which group a verdict belongs to.
    ///
    /// A `match` on the verdict alone, so the compiler owns the mapping. The
    /// grouping used to match on `(&verdict, <integer tag>)`, where a `_` arm
    /// is unavoidable — a verdict added later fell through it, was counted in
    /// no group and tallied nowhere, so the note simply vanished from the
    /// command. `evaluate` refusing to collapse the third answer buys nothing
    /// if the printer drops the fourth.
    ///
    /// Returns `report::Age`, not the heading text. Handing the renderer a
    /// string put the same failure back one layer out: it filtered on literals
    /// that had to be spelled identically in two files, with nothing checking
    /// they were.
    fn age(verdict: &Verdict) -> report::Age {
        match verdict {
            Verdict::Stale { .. } => report::Age::Stale,
            Verdict::Unknown { .. } => report::Age::Unknown,
            Verdict::NoTrigger => report::Age::NoTrigger,
            Verdict::Fresh => report::Age::Fresh,
        }
    }

    let report = report::Stale {
        judged: judged
            .iter()
            .map(|j| report::Judged {
                key: j.key.clone(),
                layer: j.layer.to_string(),
                recorded: j.recorded.clone(),
                age: age(&j.verdict),
                because: match &j.verdict {
                    Verdict::Stale { because } | Verdict::Unknown { because } => {
                        Some(because.clone())
                    }
                    Verdict::NoTrigger | Verdict::Fresh => None,
                },
            })
            .collect(),
    };

    let stale = report.count(report::Age::Stale);
    let unknown = report.count(report::Age::Unknown);
    ctx.say(&report);

    // The report is the product, so it prints in full before this decides the
    // exit code — the same order `lint` uses.
    //
    // Three states in the output and one in the exit code is the same lie
    // `Unknown` exists to refuse, moved to the boundary a script actually
    // reads. Without a code of its own, a run where git was missing and *not
    // one probe could be answered* is indistinguishable from a clean store.
    if stale > 0 {
        anyhow::bail!(
            "{stale} note{} the world has moved past",
            if stale == 1 { "" } else { "s" }
        );
    }
    if unknown > 0 {
        std::process::exit(2);
    }
    Ok(())
}

/// local → team. §12's one human gate, because it is the one place a wrong
/// note reaches somebody else.
fn memory_promote(cwd: &std::path::Path, keys: &[String], ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let notes = memory::load(&paths)?;
    let repo = paths.repo.clone();

    let steps = match memory::promote::plan(&notes, &paths, keys, &|p: &std::path::Path| {
        memory::promote::git_ignores(&repo, p)
    }) {
        // `git_ignores` now answers or refuses to; a promotion is never
        // planned against a guess about where the note would land.
        Ok(steps) => steps,
        Err(blocked) => {
            // Nothing moved. A partial promotion would leave a store nobody
            // planned, and the human who ran the gate would have to work out
            // which half landed.
            for b in &blocked {
                ctx.warn(&b.say());
            }
            anyhow::bail!("promoted nothing");
        }
    };
    memory::promote::apply(&steps)?;
    ctx.say(&report::Promoted {
        text: memory::promote::report(&steps, &paths),
        keys: steps.iter().map(|s| s.key.clone()).collect(),
    });
    Ok(())
}

/// The store, by layer, with what points at each note.
fn memory_ls(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    ctx.say(&report::Notes {
        notes: memory::load(&paths)?,
    });
    Ok(())
}

/// The store-quality meter. Violations are grouped by rule rather than listed
/// flat, because the count per rule is the signal and the individual lines are
/// how you act on it.
fn memory_lint(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let found = memory::lint(&paths)?;
    let tally = memory::tally(&found);
    let report = report::Lint {
        violations: found,
        tally,
    };

    // The report is the product, so it prints in full before this decides the
    // exit code. Warnings do not fail the command: `Orphan` fires on every
    // note nothing links to, and a gate that is always red gates nothing.
    ctx.say(&report);
    let refused = report.refused();
    if refused > 0 {
        anyhow::bail!(
            "{refused} violation{} the schema refuses",
            if refused == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// One note, and a report of what pointed at it. Deletion never cascades: a
/// dangling link is visible and the lint finds it, while a silently pruned
/// neighbourhood is neither.
fn memory_rm(
    cwd: &std::path::Path,
    key: &str,
    layer: Option<memory::Layer>,
    at: Option<&str>,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let removed = memory::remove(&paths, layer, key, at)?;

    let mut action =
        report::Action::new("note-removed", format!("removed {key} ({})", removed.layer)).data(
            serde_json::json!({
                "key": key,
                "layer": removed.layer.to_string(),
                "committed": removed.layer.is_committed(),
                "inbound": removed.inbound,
            }),
        );
    // The file is gone here, but a teammate still has it until the deletion is
    // committed. Saying so beats letting someone believe a shared note
    // disappeared for everybody.
    if removed.layer.is_committed() {
        action = action.note("it was committed — teammates keep it until you commit the deletion");
    }
    if !removed.inbound.is_empty() {
        action = action.note(format!(
            "still linked from {} — those links now dangle, and `omh memory lint` lists them",
            removed.inbound.join(", ")
        ));
    }
    ctx.say(&action);
    Ok(())
}

fn parse_env(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))
}

fn mcp(cwd: &std::path::Path, cmd: &McpCmd, dry_run: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    match cmd {
        McpCmd::Ls => show_servers(cwd, ctx),

        McpCmd::Add {
            name,
            command,
            args,
            env,
        } => {
            let server = render::Server {
                command: command.clone(),
                args: args.clone(),
                env: env.iter().cloned().collect(),
            };
            let w = config::mcp_add(&paths, name, server)?;
            let mut action =
                report::Action::new("mcp-added", format!("wrote → {}", w.path.display())).data(
                    serde_json::json!({ "server": name, "path": w.path.display().to_string() }),
                );
            if !env.is_empty() {
                // The catalogue is not committed, so nothing here reaches a
                // teammate — but it does reach every repo you work in, which is
                // the wrong scope for a token scoped to one of them.
                action = action.note(format!(
                    "this env applies in every repo. For one repo only, put \
                     [mcp.{name}.env] in .omh/{}",
                    settings::LOCAL
                ));
            }
            ctx.say(&action);
            Ok(())
        }

        McpCmd::Rm { name } => {
            let removed = config::mcp_remove(&paths, name)?;
            ctx.say(
                &report::Action::new(
                    if removed { "mcp-removed" } else { "mcp-absent" },
                    if removed {
                        format!("removed {name} from your catalogue")
                    } else {
                        format!("{name} is not in your catalogue")
                    },
                )
                .data(serde_json::json!({ "server": name, "removed": removed })),
            );
            Ok(())
        }

        McpCmd::Import {
            harness,
            file,
            force,
        } => {
            let adapter = Adapter::find(&paths.adapters(), harness)?;
            let binding = adapter
                .supports(adapter::Capability::Mcp)
                .with_context(|| format!("{harness} has no MCP capability to import from"))?;

            let home = dirs::home_dir().context("no home directory")?;
            let source = match file {
                Some(f) => f.clone(),
                None => {
                    let template = binding.import.as_deref().with_context(|| {
                        format!("adapter {harness} does not say where to import from; pass --file")
                    })?;
                    adapter::expand_host(template, &home, &paths.repo)
                }
            };

            let raw = std::fs::read_to_string(&source).with_context(|| {
                format!(
                    "reading {} — pass --file to point somewhere else",
                    source.display()
                )
            })?;
            let incoming = render::parse(binding.render, &raw)?;

            let outcome = config::mcp_import(&paths, incoming, *force, dry_run)?;
            let wrote = (!dry_run && !outcome.added.is_empty())
                .then(|| config::mcp_path(&paths).display().to_string());

            let considered = outcome
                .added
                .iter()
                .map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Took,
                    detail: String::new(),
                })
                .chain(outcome.unchanged.iter().map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Kept,
                    detail: "already identical".into(),
                }))
                .chain(outcome.conflicts.iter().map(|name| report::Considered {
                    name: name.clone(),
                    verdict: report::Verdict::Conflict,
                    detail: "differs — keeping yours; --force to overwrite".into(),
                }))
                .collect();

            ctx.say(&report::Imported {
                what: harness.clone(),
                source: source.display().to_string(),
                considered,
                noun: "servers".into(),
                dry_run,
                wrote,
                selected_in: Vec::new(),
            });
            Ok(())
        }
    }
}

/// Does this repo already say what it uses?
///
/// Read from the committed file directly rather than through `settings::resolve`,
/// which merges three layers: a `[use]` in *your* personal file is your default
/// everywhere and is not this repo having decided anything, so treating it as
/// one would leave a fresh checkout with no list of its own.
fn repo_has_selection(paths: &Paths) -> Result<bool> {
    // Through `config`, which distinguishes absent from unreadable. Reading the
    // file here with `let Ok(..) else { return Ok(false) }` reintroduced the
    // exact conflation `config::read_layer` was written about, in the one place
    // where the answer decides whether `init` overwrites a curated list — and
    // it was a third parse strategy for a file that already had two.
    config::declares(paths, config::Layer::Shared, config::USE)
}

/// The catalogue's MCP servers, with whose each one is.
fn show_servers(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    ctx.say(&report::Servers {
        servers: config::servers(&paths)?
            .into_iter()
            .map(|s| report::Setting {
                key: s.key,
                value: s.value,
                whose: Some(s.layer.whose().to_string()),
            })
            .collect(),
    });
    Ok(())
}

/// Select a catalogue entry for this repo, or resync the whole list.
///
/// Writes the **committed** file. What a project uses is a fact about the
/// project, and a teammate cloning it should get the same selection — the
/// opposite default from `omh repo set`, which holds secrets.
///
/// A capability with no list is following the whole catalogue, so adding one
/// name to it has to write the catalogue out first. Writing `["tdd"]` alone
/// would silently turn off everything else, which is the one thing a command
/// called `use` must never do.
fn use_cmd(
    cwd: &std::path::Path,
    capability: Option<&str>,
    name: Option<&str>,
    all: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    if all {
        if capability.is_some() {
            anyhow::bail!("`--all` resyncs every capability — it takes no arguments");
        }
        let lists = catalogue_lists(&paths)?;
        ctx.say(&report::Resynced {
            wrote: write_lists(&paths, &lists)?
                .into_iter()
                .map(|w| w.path.display().to_string())
                .collect(),
            counts: lists
                .iter()
                .map(|(cap, names)| (cap.to_string(), names.len()))
                .collect(),
        });
        return Ok(());
    }

    let (Some(key), Some(name)) = (capability, name) else {
        anyhow::bail!(
            "omh use <capability> <name>, or omh use --all\n  capabilities: {}",
            capability_list()
        );
    };
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    // A name nothing answers to is a typo far more often than a plan, and the
    // launcher would only report it later. `omh config edit` is how you create
    // the entry first.
    let available = catalogue_names(&paths, cap)?;
    if !available.iter().any(|n| n == name) {
        anyhow::bail!(
            "your catalogue has no {cap} called `{name}`. `omh config edit {cap} {name}` \
             creates it.\n  {cap}: {}",
            if available.is_empty() {
                "(empty)".to_string()
            } else {
                available.join(", ")
            }
        );
    }
    let already = names.iter().any(|n| n == name);
    // "Already used" only means something once there *is* a list. While a
    // capability is still following the whole catalogue every name is used, and
    // saying so would leave `omh use` unable to start a selection at all.
    if already && !was_open {
        ctx.say(
            &report::Action::new(
                "capability-already-used",
                format!("{cap}/{name} is already used here"),
            )
            .data(serde_json::json!({
                "capability": cap.to_string(),
                "name": name,
                "changed": false,
            })),
        );
        return Ok(());
    }
    if !already {
        names.push(name.to_string());
    }
    let written = write_lists(
        &paths,
        &std::collections::BTreeMap::from([(cap, names.clone())]),
    )?;
    // Said out loud, because this is the moment a capability turns from
    // "follows the catalogue" into "this list" — everything is still selected,
    // but from now on by name, and an entry added later will not be.
    let froze = was_open.then(|| {
        format!(
            "{cap} was following your whole catalogue; wrote its {} entries as the list",
            names.len()
        )
    });
    let paths = written_paths(&written);
    let mut action = report::Action::new("capability-used", format!("using {cap}/{name}")).data(
        serde_json::json!({
            "capability": cap.to_string(),
            "name": name,
            "changed": true,
            "froze_selection": was_open,
            "paths": paths,
        }),
    );
    if let Some(line) = &froze {
        action = action.note(line);
    }
    for path in &paths {
        action = action.note(format!("wrote → {path}"));
    }
    ctx.say(&action);
    Ok(())
}

/// Every file a write landed in, collapsed into one list for one report.
///
/// **This is what stops a command saying itself twice.** A repo can declare a
/// capability in both its shared and its gitignored layer, so these writers
/// loop; a `ctx.say` inside that loop emits a JSON document per layer, and two
/// documents concatenated are a parse error in whatever reads them. Calling
/// this is the shape that cannot make the mistake — the plural is in the value
/// rather than in the number of times the command speaks.
///
/// Guarded by `every_json_answer_is_one_document_and_not_several`.
fn written_paths(written: &[config::Written]) -> Vec<String> {
    written
        .iter()
        .map(|w| w.path.display().to_string())
        .collect()
}

/// Stop using a catalogue entry here.
fn unuse_cmd(cwd: &std::path::Path, key: &str, name: &str, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let (cap, mut names, was_open) = current_list(&paths, key, name)?;
    if !names.iter().any(|n| n == name) {
        // Refused rather than written as a no-op: a name this repo never used
        // is a typo, and writing the list back would report success for it.
        anyhow::bail!(
            "{cap}/{name} is not used here. `omh repo` lists what is.\n  \
             using: {}",
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            }
        );
    }
    names.retain(|n| n != name);
    // The same disclosure `use_cmd` makes, and for the same reason: this is the
    // moment the capability stops following the catalogue. Discarding the flag
    // here was an oversight rather than a decision — `unuse` performs the
    // identical conversion, so a repo with no list at all freezes into one on
    // the command that was meant to remove one name.
    let froze = was_open.then(|| {
        format!(
            "{cap} was following your whole catalogue; wrote its remaining {} entries as the list",
            names.len()
        )
    });
    let remaining = names.len();
    let written = write_lists(&paths, &std::collections::BTreeMap::from([(cap, names)]))?;
    let paths = written_paths(&written);
    let mut action =
        report::Action::new("capability-unused", format!("no longer using {cap}/{name}")).data(
            serde_json::json!({
                "capability": cap.to_string(),
                "name": name,
                "froze_selection": was_open,
                "remaining": remaining,
                "paths": paths,
            }),
        );
    if let Some(line) = &froze {
        action = action.note(line);
    }
    for path in &paths {
        action = action.note(format!("wrote → {path}"));
    }
    ctx.say(&action);
    Ok(())
}

/// Write these lists to every repo layer that has a say in them.
///
/// One capability at a time, because which layers declare `skills` and which
/// declare `mcp` are different questions — `omh use --all` in a repo whose
/// gitignored file overrides exactly one capability must not acquire the other
/// five there.
fn write_lists(
    paths: &Paths,
    lists: &std::collections::BTreeMap<adapter::Capability, Vec<String>>,
) -> Result<Vec<config::Written>> {
    let mut out = Vec::new();
    for (cap, names) in lists {
        let one = std::collections::BTreeMap::from([(*cap, names.clone())]);
        for layer in config::declaring(paths, config::USE, &cap.to_string())? {
            out.push(config::write_selection(paths, layer, &one)?);
        }
    }
    // Two capabilities can share a layer, and reporting the same file twice
    // reads as two writes.
    //
    // **Sorted first.** `dedup_by` only drops *adjacent* duplicates, and this
    // vec is built capability-outer/layer-inner, so a repo whose shared and
    // local files both declare `[use]` produces `[shared, local, shared,
    // local, …]` — where no two duplicates are ever adjacent and the dedup
    // removes nothing. `omh use --all` reported five writes to two files, and
    // `--json` said so in a five-element array.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// This capability's effective list, and whether it had one at all.
///
/// The name is validated here rather than at the write, which is the same rule
/// `[use]` follows: a name is checked where it is minted, so `omh use` cannot
/// put something in the file that reading the file would refuse.
/// Bring one capability across from a harness you already use.
///
/// **Hooks go to the repo; everything else goes to the catalogue.** That
/// asymmetry is the design rather than an accident: a hook binds to one
/// project's commands, and a skill, a rule or a command is a way *you* work and
/// travels with you. Importing a skill into a repo would be a skill you only
/// had in one place; importing a hook into the catalogue would put one
/// project's formatter in front of every other project you open.
fn import_cmd(
    cwd: &std::path::Path,
    capability: &str,
    harness: &str,
    from: Option<&std::path::Path>,
    ctx: &out::Ctx,
) -> Result<()> {
    let cap = adapter::Capability::from_key(capability).with_context(|| {
        format!(
            "`{capability}` is not a capability — expected {}",
            capability_list()
        )
    })?;
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let binding = adapter
        .supports(cap)
        .with_context(|| format!("{harness} has no {cap} for omh to read"))?;

    let source = match from {
        Some(f) => f.to_path_buf(),
        None => {
            let template = binding.import.as_deref().with_context(|| {
                format!(
                    "{harness} keeps its {cap} somewhere omh cannot read — \
                     `omh import {capability} {harness} --from <path>` if you know where"
                )
            })?;
            let home = dirs::home_dir().context("no home directory")?;
            adapter::expand_host(template, &home, &paths.repo)
        }
    };
    if !source.exists() {
        ctx.say(
            &report::Action::new(
                "import-nothing-there",
                format!("{harness} has no {cap} here ({})", source.display()),
            )
            .data(serde_json::json!({
                "harness": harness,
                "capability": cap.to_string(),
                "source": source.display().to_string(),
                "exists": false,
            })),
        );
        return Ok(());
    }

    match cap {
        // Hooks are translated rather than copied — they are the one capability
        // whose format is omh's own — and they land in the repo.
        adapter::Capability::Hooks => import_hooks(&paths, &adapter, binding, &source, ctx),
        adapter::Capability::Mcp => anyhow::bail!(
            "MCP servers are `omh config mcp import {harness}` — a server is a \
             record in one file, not an entry with its own"
        ),
        _ => import_entries(&paths, harness, cap, binding.render, &source, ctx),
    }
}

/// Copy into the catalogue what a harness already holds, entry by entry.
///
/// **Into `~/.omh/`, not the repo** — the opposite of hooks, and for the reason
/// `docs/configuration.md` gives: a skill is a way *you* work and travels with
/// you across projects, while a hook binds to one repo's commands. Importing a
/// skill into a repo would be a skill you only had in one place.
///
/// Rules are one file becoming one entry named after the harness it came from;
/// everything else is a directory whose children each become an entry. Which
/// shape a capability has is read off the adapter's `render`, not hardcoded —
/// the same field the launcher stages by.
///
/// Never clobbers. An entry already in your catalogue is left exactly as it is
/// and reported, so re-running is a no-op and an import cannot quietly replace
/// something you have since edited.
fn import_entries(
    paths: &Paths,
    harness: &str,
    cap: adapter::Capability,
    render: adapter::Render,
    source: &std::path::Path,
    ctx: &out::Ctx,
) -> Result<()> {
    let dest = paths.root.join(cap.source());

    let entries: Vec<(String, std::path::PathBuf)> = match render {
        // One file, one entry. Named after the harness rather than after the
        // file: `CLAUDE.md` in your catalogue says nothing about whose rules
        // they were, and `omh why rules/claude` is the question somebody asks.
        adapter::Render::Concat => vec![(format!("{harness}.md"), source.to_path_buf())],
        _ => {
            let mut found = Vec::new();
            let listing = std::fs::read_dir(source)
                .with_context(|| format!("reading {}", source.display()))?;
            for entry in listing {
                let path = entry
                    .with_context(|| format!("reading {}", source.display()))?
                    .path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                found.push((name.into_owned(), path));
            }
            found.sort();
            found
        }
    };

    let mut considered = Vec::new();
    for (name, from) in entries {
        // The stem, because a catalogue entry is a name and `review-diff.md` is
        // a filename. `validate_entry_name` then refuses `..`, a separator, and
        // every dotfile in one arm — so `../evil` cannot name an entry, and a
        // path cannot be smuggled in as one.
        let stem = std::path::Path::new(&name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if let Err(e) = selection::validate_entry_name(&stem, cap, source) {
            considered.push(report::Considered {
                name,
                verdict: report::Verdict::Skipped,
                detail: format!("{e:#}"),
            });
            continue;
        }
        let to = dest.join(if from.is_dir() {
            stem.clone()
        } else {
            name.clone()
        });
        if to.exists() {
            considered.push(report::Considered {
                name: stem,
                verdict: report::Verdict::Kept,
                detail: "already in your catalogue".into(),
            });
            continue;
        }
        considered.push(match copy_entry(&from, &to) {
            Ok(()) => report::Considered {
                name: stem,
                verdict: report::Verdict::Took,
                detail: String::new(),
            },
            Err(e) => report::Considered {
                name: stem,
                verdict: report::Verdict::Skipped,
                detail: format!("{e:#}"),
            },
        });
    }

    // Where the entries landed. `None` here said "nothing was written" to both
    // audiences on a run that had just copied files into the catalogue —
    // `mcp import` sets this and these did not, which is what made it an
    // omission rather than a convention.
    let took = considered
        .iter()
        .any(|c| c.verdict == report::Verdict::Took);
    ctx.say(&report::Imported {
        what: format!("{harness} {cap}"),
        source: source.display().to_string(),
        considered,
        noun: cap.to_string(),
        dry_run: false,
        wrote: took.then(|| dest.display().to_string()),
        selected_in: Vec::new(),
    });
    Ok(())
}

/// Copy one catalogue entry — a file, or a directory whole.
///
/// **Refuses any symlink**, at any depth, rather than following it or copying
/// it as a link. Following one lets a skill directory reach outside itself, and
/// the catalogue is mounted into every sandbox omh launches — so a link to
/// `~/.ssh` in somebody's skill would become a file the agent can read, in
/// every project, from a copy they had no reason to inspect. Copying the link
/// verbatim is no better: it points somewhere that means something else once
/// the entry has moved.
///
/// Refusing whole rather than skipping the link: an entry with a piece missing
/// is not a smaller version of that entry, and this is the same rule
/// `render::parse_hooks` applies to a handler it cannot say completely.
fn copy_entry(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    // Looked at **before** anything is written, so the common refusal never
    // starts a copy — and undone below if a write fails for any other reason,
    // because "refused whole" has to mean nothing was left behind. A
    // half-copied skill is mounted into every sandbox exactly as a whole one
    // is, and reads as an entry somebody chose.
    refuse_symlinks(from)?;
    if let Err(e) = copy_tree(from, to) {
        // Safe to remove: `import_entries` only calls this for a destination
        // that did not exist, so everything here is what this call just wrote.
        let undone = if to.is_dir() {
            std::fs::remove_dir_all(to)
        } else {
            std::fs::remove_file(to)
        };
        // **And the undo is not allowed to fail quietly.** It fails for the
        // same reasons the copy did — a read-only destination, a
        // permission-denied child — so the residue survives precisely in the
        // cases that produced it. The caller then prints `skipped`, which means
        // *nothing was written*, and the **next** run sees the partial entry,
        // reports `kept — already in your catalogue`, and mounts it into every
        // sandbox omh launches. A skill with its `SKILL.md` and none of its
        // scripts, presented as one somebody chose to keep.
        if let Err(u) = undone {
            return Err(e).with_context(|| {
                format!(
                    "and {} could not be removed ({u}) — a partial copy is still \
                     there, and the next import will report it as an entry you \
                     already have. Delete it before re-running.",
                    to.display()
                )
            });
        }
        return Err(e);
    }
    Ok(())
}

/// Refuse a symlink at any depth, before a byte is written.
fn refuse_symlinks(from: &std::path::Path) -> Result<()> {
    let meta =
        std::fs::symlink_metadata(from).with_context(|| format!("reading {}", from.display()))?;
    anyhow::ensure!(
        !meta.file_type().is_symlink(),
        "{} is a symlink, and omh will not copy one into a catalogue that is \
         mounted into every sandbox",
        from.display()
    );
    if meta.is_dir() {
        let listing =
            std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))?;
        for entry in listing {
            refuse_symlinks(&entry?.path())?;
        }
    }
    Ok(())
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        let listing =
            std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))?;
        for entry in listing {
            let child = entry?.path();
            let name = child
                .file_name()
                .context("a path from read_dir has a name")?;
            copy_tree(&child, &to.join(name))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    Ok(())
}

/// Harnesses on this machine whose hooks omh could bring across.
///
/// **A report, never an action.** Importing writes executable content into
/// somebody's repo, and doing that because `init` found a file would be omh
/// deciding on their behalf what runs at the end of their turns. So `init`
/// names what is there and what would take it; `omh import hooks` is a
/// separate act somebody chooses.
///
/// Never fatal and never noisy: a harness with no config, a config that will
/// not parse, an adapter that declares no import path — all of them are simply
/// not mentioned. There is nothing to tell somebody about a file that is not
/// there.
fn importable(paths: &Paths, harnesses: &[String]) -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for name in harnesses {
        let Ok(adapter) = Adapter::find(&paths.adapters(), name) else {
            continue;
        };
        let Some(binding) = adapter.supports(adapter::Capability::Hooks) else {
            continue;
        };
        let Some(template) = binding.import.as_deref() else {
            continue;
        };
        let source = adapter::expand_host(template, &home, &paths.repo);
        // **Absent and unreadable are not the same thing**, and this function's
        // own justification used to conflate them: "there is nothing to tell
        // somebody about a file that is not there" is true, and a
        // `~/.claude/settings.json` full of hooks that is one comma short of
        // parsing *is* there. Silent, it produces the same output as a clean
        // machine — so somebody works in omh with none of their hooks, believes
        // omh found nothing of theirs, and never runs the one command that
        // would print the reason.
        let raw = match std::fs::read_to_string(&source) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.push(format!(
                    "import     {name}'s hooks are at {} and omh could not read \
                     it ({e})",
                    source.display()
                ));
                continue;
            }
        };
        let Ok(vocab) = hook::Vocabulary::of(binding, &adapter.tools) else {
            continue;
        };
        let (found, residue) = match render::parse_hooks(&raw, &vocab) {
            Ok(v) => v,
            Err(e) => {
                out.push(format!(
                    "import     {name} has hooks in {} that omh could not read \
                     ({e:#}) — omh import hooks {name} to see why",
                    source.display()
                ));
                continue;
            }
        };
        if found.is_empty() && residue.is_empty() {
            continue;
        }
        out.push(format!(
            "import     {name} has {} hook{} omh can read{} — omh import hooks {name}",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            if residue.is_empty() {
                String::new()
            } else {
                format!(" and {} it cannot", residue.len())
            }
        ));
    }

    // And the capabilities that are copied rather than translated. Counted by
    // what is actually there — an empty `~/.claude/commands` says nothing worth
    // a line, and a line per harness per capability would bury the report in
    // things nobody has.
    for name in harnesses {
        let Ok(adapter) = Adapter::find(&paths.adapters(), name) else {
            continue;
        };
        for cap in adapter::Capability::ALL {
            if matches!(cap, adapter::Capability::Hooks | adapter::Capability::Mcp) {
                continue;
            }
            let Some(template) = adapter.supports(cap).and_then(|b| b.import.as_deref()) else {
                continue;
            };
            let source = adapter::expand_host(template, &home, &paths.repo);
            let held = match std::fs::read_dir(&source) {
                Ok(listing) => listing.count(),
                // A rules import is one file rather than a directory, so it
                // counts as one thing when it is there.
                Err(_) if source.is_file() => 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                // Same rule as the hooks half: a directory omh cannot read is
                // not a directory with nothing in it, and reporting zero would
                // be indistinguishable from a machine that has none.
                Err(e) => {
                    out.push(format!(
                        "import     {name}'s {cap} are at {} and omh could not \
                         read it ({e})",
                        source.display()
                    ));
                    continue;
                }
            };
            if held > 0 {
                out.push(format!(
                    "import     {name} has {held} {cap} — omh import {cap} {name}"
                ));
            }
        }
    }
    out
}

/// Bring hooks somebody already configured in a harness into this repo.
///
/// **Into `<repo>/.omh/hooks/`, never the catalogue.** A catalogue hook runs in
/// every repo you ever open, so importing one project's `prettier --write`
/// there would put it in front of every other project you touch — worse than
/// not importing at all, and invisible until it ran somewhere it should not
/// have.
///
/// **Copy, never move.** The harness keeps working exactly as it did; adopting
/// omh is not a migration you cannot back out of. The source file is not
/// touched at all.
///
/// Two failure modes this is written against, and both are silent:
///
/// - **A hook that lands and never runs.** `[use]` is what the launcher reads,
///   so a file written without being selected is a hook `omh import` counted
///   and no session will ever ship. The report would say `+6` and the launch
///   would ship none.
/// - **A hook that stops every launch.** A file answering to a name omh's base
///   manifest owns makes `merge_hooks` bail, which fails the whole session
///   rather than that one hook. Refused here, by name.
fn import_hooks(
    paths: &Paths,
    adapter: &Adapter,
    binding: &adapter::Binding,
    source: &std::path::Path,
    ctx: &out::Ctx,
) -> Result<()> {
    let harness = &adapter.name;
    let raw =
        std::fs::read_to_string(source).with_context(|| format!("reading {}", source.display()))?;

    let vocab = hook::Vocabulary::of(binding, &adapter.tools)
        .with_context(|| format!("reading {harness}'s vocabulary backwards"))?;
    let (found, residue) = render::parse_hooks(&raw, &vocab)?;

    let manifest = base::Manifest::load_dir(&paths.base())?;
    // Every hook name the manifest owns, whether or not its feature is on
    // here — a repo with `codegraph` disabled must still not be handed a file
    // called `graph-refresh`, because enabling it later would then fail every
    // launch rather than that one hook.
    let reserved: std::collections::BTreeSet<String> = manifest
        .owns()
        .get(&adapter::Capability::Hooks)
        .map(|owned| owned.keys().cloned().collect())
        .unwrap_or_default();
    let dir = paths.repo.join(".omh/hooks");

    let mut considered = Vec::new();
    let mut written = Vec::new();
    for (name, hook) in &found {
        // A name omh's manifest owns is not a hook that would be shadowed —
        // it is a file `merge_hooks` refuses, which takes the whole session
        // down rather than just this hook. Refused here, where the person can
        // still see why.
        if reserved.contains(name) {
            considered.push(report::Considered {
                name: name.clone(),
                verdict: report::Verdict::Skipped,
                detail: "omh ships a hook by that name".into(),
            });
            continue;
        }
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            considered.push(report::Considered {
                name: name.clone(),
                verdict: report::Verdict::Kept,
                detail: "already here, left as it is".into(),
            });
            continue;
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(hook)?))?;
        considered.push(report::Considered {
            name: name.clone(),
            verdict: report::Verdict::Took,
            detail: hook.does().to_string(),
        });
        written.push(name.clone());
    }

    // Selected, or they land dead. This is the failure the whole feature is
    // most likely to have: files on disk, a report saying six, and a launch
    // that ships none of them because `[use]` never named them.
    let mut selected_in = Vec::new();
    if !written.is_empty() && repo_has_selection(paths)? {
        let (cap, mut names, _) = current_list(paths, "hooks", &written[0])?;
        names.extend(written.iter().cloned());
        names.sort();
        names.dedup();
        let lists = std::collections::BTreeMap::from([(cap, names)]);
        for w in write_lists(paths, &lists)? {
            selected_in.push(w.path.display().to_string());
        }
    }

    // Named, never silently left behind. A hook omh could not bring across is
    // still in the harness's own file and still running there, which is the
    // honest outcome — but somebody who was not told would think omh had taken
    // everything.
    for d in &residue {
        considered.push(report::Considered {
            name: d.name.clone(),
            verdict: report::Verdict::Left,
            detail: d.wanted.clone(),
        });
    }

    ctx.say(&report::Imported {
        what: format!("{harness} hooks"),
        source: source.display().to_string(),
        considered,
        noun: "hooks".into(),
        dry_run: false,
        // The hooks directory, for the same reason as `import_entries`: a run
        // that wrote files has to say where they went.
        wrote: (!written.is_empty()).then(|| dir.display().to_string()),
        selected_in,
    });
    Ok(())
}

fn current_list(
    paths: &Paths,
    key: &str,
    name: &str,
) -> Result<(adapter::Capability, Vec<String>, bool)> {
    let cap = adapter::Capability::from_key(key).with_context(|| {
        format!(
            "`{key}` is not a capability — expected {}",
            capability_list()
        )
    })?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let policy = settings::resolve(paths, &manifest)?;
    let file = config::Layer::Shared.file(paths);
    selection::validate_entry_name(name, cap, &file)?;
    if let Some(feature) = manifest
        .owns()
        .get(&cap)
        .and_then(|owned| owned.get(name))
        .cloned()
    {
        anyhow::bail!(
            "{cap}/{name} is omh's — part of the `{feature}` feature. `[use]` names \
             your entries; a feature is all or nothing, so `omh repo enable {feature}` \
             and `omh repo disable {feature}` are its switches."
        );
    }
    match policy.selection.order(cap) {
        Some(names) => Ok((cap, names.to_vec(), false)),
        // No list: this capability follows the whole catalogue, so the list
        // that keeps that true is the catalogue itself.
        None => Ok((cap, catalogue_names(paths, cap)?, true)),
    }
}

/// Every capability's catalogue entries, minus the ones omh owns.
fn catalogue_lists(
    paths: &Paths,
) -> Result<std::collections::BTreeMap<adapter::Capability, Vec<String>>> {
    let mut out = std::collections::BTreeMap::new();
    for cap in adapter::Capability::ALL {
        out.insert(cap, catalogue_names(paths, cap)?);
    }
    Ok(out)
}

/// Which of these hooks this repo could ever take.
///
/// A hook naming an ecosystem this repo is not is dropped; a hook naming none
/// is kept, and so is a name nothing declared. **Applicability, not
/// selection** — `[use]` records what you chose from what you could have
/// chosen, and offering a rust repo `go-test` makes the unselected report
/// unreadable rather than more complete.
///
/// The asymmetry is deliberate: this drops what names an *undetected* stack,
/// rather than keeping what names a detected one. Written the other way it
/// would hide every hook that belongs everywhere, which is most of them.
/// Which of **this repo's** ecosystems something already speaks for.
///
/// The intersection is the whole of it, and leaving it out made a milestone's
/// worth of code unreachable. `declared_stacks` over the catalogue answers
/// `{rust, go, python}` in every repo on earth, because that is what omh
/// ships — so handed to `derive::hooks` as *covered* it meant
/// `covered.is_empty()` was never true, and every `Makefile`, `justfile` and
/// `Taskfile` derivation could not fire for anybody. Only node worked, because
/// omh ships no node hook, which is why nothing looked broken.
///
/// The user-visible end was worse than a missing hook: `ask::what_tests_it`
/// then said *"no stack it knows, no lockfile, no runner"* about a repo whose
/// `Makefile` omh had just read and whose `test` target it had found.
fn covered_here(
    hook_dirs: &[std::path::PathBuf],
    detected: &[&stack::Definition],
) -> Result<BTreeSet<String>> {
    Ok(render::declared_stacks(hook_dirs)?
        .into_values()
        .flatten()
        .filter(|named| detected.iter().any(|d| &d.name == named))
        .collect())
}

fn applicable_hooks(
    names: Vec<String>,
    declared: &BTreeMap<String, Option<String>>,
    detected: &BTreeSet<String>,
) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| match declared.get(n) {
            Some(Some(stack)) => detected.contains(stack),
            _ => true,
        })
        .collect()
}

/// The names a `[use]` list may hold for `cap`: what the catalogue and this
/// repo declare, minus omh's own, which `[omh]` governs and `[use]` refuses.
fn catalogue_names(paths: &Paths, cap: adapter::Capability) -> Result<Vec<String>> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let owned = manifest.owns();
    let profile = Profile::resolve(paths);
    let names: Vec<String> = profile
        .entries(cap)?
        .into_iter()
        .filter(|n| !owned.get(&cap).is_some_and(|o| o.contains_key(n)))
        .collect();
    if cap != adapter::Capability::Hooks {
        return Ok(names);
    }
    // Hooks alone can belong to an ecosystem, and omh now ships one set per
    // ecosystem. Offering a rust repo `go-test` would put every stack omh
    // knows into the list `init` writes and the launcher reports.
    let defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected: BTreeSet<String> = stack::detected(&defs, &paths.repo)
        .into_iter()
        .map(|d| d.name.clone())
        .collect();
    let declared = render::declared_stacks(&profile.sources(cap)?)?;
    Ok(applicable_hooks(names, &declared, &detected))
}

/// Your defaults and your catalogue.
///
/// Deliberately not the resolved three-layer merge any more — that question is
/// "what is effective *here*", and it moved to `omh repo` with the rest of the
/// repo-scoped reporting. This command narrows to mean **you**.
fn show_config(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);

    let mut catalogue = Vec::new();
    for cap in adapter::Capability::ALL {
        catalogue.push(report::Catalogue {
            capability: cap.to_string(),
            entries: profile.entries(cap)?,
        });
    }

    ctx.say(&report::Config {
        defaults_file: config::Layer::Personal.file(&paths).display().to_string(),
        settings: config::policy(&paths)?
            .into_iter()
            .filter(|s| s.layer == config::Layer::Personal)
            .map(|s| report::Setting {
                key: s.key,
                value: s.value,
                whose: None,
            })
            .collect(),
        catalogue_dir: paths.root.display().to_string(),
        catalogue,
    });
    Ok(())
}

/// What is effective in this checkout, and which file decided it.
///
/// Where the reporting this design keeps promising actually surfaces. With a
/// curated list the useful question stops being "what is this set to" and
/// becomes "why is this skill not here", and that needs the selection, the
/// features and the settings in one place.
fn show_repo(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let policy = settings::resolve(&paths, &manifest)?;

    let settings = config::policy(&paths)?
        .into_iter()
        .map(|s| report::Effective {
            key: s.key,
            value: s.value,
            layer: s.layer.to_string(),
            shadows: s.shadows.iter().map(|l| l.to_string()).collect(),
        })
        .collect();

    let mut names: Vec<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    names.sort();
    names.dedup();
    let features = names
        .into_iter()
        .map(|feature| report::Feature {
            name: feature.to_string(),
            on: !policy.off.contains(feature),
        })
        .collect();

    let mut using = Vec::new();
    for cap in adapter::Capability::ALL {
        let entries = profile.entries(cap)?;
        let unselected = policy.selection.unselected(cap, &entries);
        // `None` rather than a list identical to the catalogue's, because the
        // two are different states: one follows the catalogue as it grows and
        // the other is a list that happens to be complete today.
        //
        // Kept in the **declared** order, not `entries`' alphabetical one. For
        // `rules` that order is the whole feature — this page's own docs say
        // "the list is the order" — and building the line from the sorted
        // catalogue made `omh repo` the one place that contradicted it. Filtered
        // by what the catalogue actually holds, so a name nothing answers to is
        // reported as missing rather than listed as used.
        using.push(report::Using {
            capability: cap.to_string(),
            selected: policy.selection.order(cap).map(|order| {
                order
                    .iter()
                    .filter(|n| entries.iter().any(|e| e == *n))
                    .cloned()
                    .collect()
            }),
            unselected,
        });
    }

    ctx.say(&report::Repo {
        dir: paths.repo.join(".omh").display().to_string(),
        settings,
        features,
        using,
        notices: notice::selection(&profile, &policy.selection, &catalogue_lists(&paths)?)?,
    });
    Ok(())
}

/// `--layer` is going away. Accepted for one release, saying what replaced it.
///
/// The `keys.toml` treatment minus the refusal: this one is recoverable by
/// retyping, so a hard error would cost more than it protects. What it must not
/// do is keep working silently — a flag that outlives its documentation is how
/// people learn a command by copying a form that is about to stop existing.
fn layer_or(named: Option<config::Layer>, default: config::Layer, ctx: &out::Ctx) -> config::Layer {
    let Some(layer) = named else {
        return default;
    };
    let replacement = match layer {
        config::Layer::Personal => "omh config set",
        config::Layer::Shared => "omh repo set --shared",
        config::Layer::Local => "omh repo set",
    };
    ctx.warn(&format!(
        "--layer {layer} is going away — that is `{replacement}` now. \
         Two scopes, two commands: `omh config` is you, `omh repo` is this checkout."
    ));
    layer
}

/// `omh repo set` writes the gitignored file; `--shared` writes the committed
/// one. The opposite default from `omh use`, deliberately: these carry
/// `carry_in` paths and MCP env, and a mistyped key must not be committable by
/// accident.
fn repo_layer(shared: bool) -> config::Layer {
    if shared {
        config::Layer::Shared
    } else {
        config::Layer::Local
    }
}

fn set(
    cwd: &std::path::Path,
    key: &str,
    value: &str,
    layer: config::Layer,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let w = config::set(&paths, key, value, layer)?;
    // Written either way. A settings file is hand-editable and a key a newer
    // omh will read must not be refused by this one — but a key *this* omh
    // reads nothing from looks identical to one that took, and `carry_ins` is
    // a plausible thing to type. Named, not refused.
    match key::describes(key) {
        None => ctx.warn(&format!(
            "nothing in omh reads `{key}` — it is written, and it will sit there"
        )),
        // Written either way, for the same reason: a value a newer omh will
        // accept must not be refused by this one. But `persistence = tmux`
        // otherwise surfaces at the next launch, in a different command,
        // minutes later.
        Some(k) => {
            if let Some(quarrel) = key::quarrel(k, value) {
                ctx.warn(&format!("{quarrel} — written anyway"));
            }
        }
    }
    ctx.say(
        &report::Action::new("setting-written", format!("wrote → {}", w.path.display())).data(
            serde_json::json!({
                "key": key,
                "value": value,
                "layer": w.layer.to_string(),
                "committed": w.committed,
                "path": w.path.display().to_string(),
            }),
        ),
    );
    // The one mistake git makes unrecoverable. On stderr through `warn`, so it
    // survives `omh config set … > log` — which is exactly the invocation a
    // script that is about to commit a secret would use.
    if w.committed {
        ctx.warn(&format!(
            "the {} layer is COMMITTED — never put a secret here",
            w.layer
        ));
        // The general sentence fires for `account` — a name — exactly as it
        // does for `carry_in`, and a warning that cannot tell those apart is
        // one people learn to scroll past. Where omh knows the key reaches a
        // credential, it says so and says where the value would have gone.
        if let Some(k) = key::describes(key) {
            if k.secret == key::Secret::Yes {
                ctx.warn(&format!(
                    "  `{key}` is one of those — it belongs in {}",
                    k.default_layer().file(&paths).display()
                ));
            }
        }
    }
    Ok(())
}

fn unset(cwd: &std::path::Path, key: &str, layer: config::Layer, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let removed = config::unset(&paths, key, layer)?;
    ctx.say(
        &report::Action::new(
            if removed {
                "setting-removed"
            } else {
                "setting-absent"
            },
            if removed {
                format!("removed {key} from the {layer} layer")
            } else {
                format!("{key} was not set in the {layer} layer")
            },
        )
        .data(serde_json::json!({
            "key": key,
            "layer": layer.to_string(),
            "removed": removed,
        })),
    );
    Ok(())
}

/// `$EDITOR` on your settings, or on one catalogue entry.
///
/// Once `$EDITOR` is spawned it is a full program running as you, and any fence
/// omh drew around it would be decorative — there is no trust boundary between
/// omh and the person whose home directory this is. The boundary that matters
/// is structural and already there: every catalogue directory a sandbox is given
/// is mounted **read-only**.
///
/// This used to say `~/.omh` is not mounted at all, which is simply false —
/// `container.rs` binds each catalogue source at `/omh/layers/<n>/<cap>` — and
/// it is the kind of claim a reader takes on trust. Read-only is the true
/// version and carries the same argument.
///
/// What does need a guard is the **name**, the moment this takes one and joins
/// it to a directory: `omh config edit skills ../../../.ssh/id_rsa` is
/// traversal. Same rule and same function as `[use]` uses, because it is the
/// same act — a name being minted.
fn edit(
    cwd: &std::path::Path,
    capability: Option<&str>,
    name: Option<&str>,
    layer: config::Layer,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let file = match capability {
        None => layer.file(&paths),
        Some(key) => {
            let cap = adapter::Capability::from_key(key).with_context(|| {
                format!(
                    "`{key}` is not a capability — expected {}",
                    capability_list()
                )
            })?;
            let dir = paths.root.join(cap.source());
            match name {
                None => dir,
                Some(name) => {
                    selection::validate_entry_name(name, cap, &dir)?;
                    dir.join(name)
                }
            }
        }
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    Command::new(editor).arg(&file).status()?;
    Ok(())
}

fn capability_list() -> String {
    adapter::Capability::ALL
        .iter()
        .map(adapter::Capability::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Switch one of omh's features on or off in this checkout.
///
/// `enable`/`disable` rather than `use`/`unuse`, because the CLI should teach
/// the file's structure rather than flatten it: if `omh repo disable` took a
/// skill name, the difference between *an entry you chose* and *a feature omh
/// ships* would exist only in the docs.
fn feature_switch(cwd: &std::path::Path, feature: &str, on: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let features: std::collections::BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    if !features.contains(feature) {
        // The entry-name case is the interesting error: it is how somebody
        // discovers the grouping without reading the manifest.
        if let Some(entry) = manifest.entry(feature) {
            anyhow::bail!(
                "`{feature}` is part of the `{}` feature, not a feature itself. \
                 A feature is all or nothing — `omh repo disable {}` switches all of it off.",
                entry.feature,
                entry.feature
            );
        }
        anyhow::bail!(
            "`{feature}` is not one of omh's features ({}). \
             A catalogue entry of yours is `omh use`/`omh unuse`.",
            features.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    // The committed file: which of omh's features a project runs with is a fact
    // about the project, the same argument `omh use` writes there on.
    // ...and the gitignored one too when it already declares this feature, or
    // the switch reports a change the layer beneath it overrules.
    let mut written = Vec::new();
    for layer in config::declaring(&paths, config::OMH, feature)? {
        written.push(config::write_feature(&paths, layer, feature, on)?);
    }
    let paths = written_paths(&written);
    let mut action = report::Action::new(
        if on { "feature-on" } else { "feature-off" },
        format!("{feature} is {} here", if on { "on" } else { "off" }),
    )
    .data(serde_json::json!({
        "feature": feature,
        "on": on,
        "paths": paths,
    }));
    if !on {
        action = action.note("nothing was uninstalled; the next repo gets it back");
    }
    for path in &paths {
        action = action.note(format!("wrote → {path}"));
    }
    ctx.say(&action);
    Ok(())
}

/// Say what composing the project's rules turned up, if anything.
///
/// Called from every path that builds a plan, not just `run`: `attach` and
/// `doctor` compose the same document, and a fallback announced on one path in
/// three is the same silence the notice exists to break. Only when there is
/// something to say — a line printed every launch is a line nobody reads.
fn say_rules(plan: &container::Plan, ctx: &out::Ctx) {
    for notice in plan.rules.notices() {
        ctx.warn(&notice.to_string());
    }
}

/// What the launcher noticed about this repo's hooks: which ones it has, which
/// are new or changed, and where detection and the directory disagree.
///
/// Reported on every launch including a dry run — a dry run is exactly when you
/// want to be told what a launch would hand your agent. The returned `Record`
/// is the *other* half: committing it is what spends the "new or changed"
/// call-out, so only a session that actually started may do it.
///
/// Never fatal. A repo whose hook drift cannot be computed is still a repo you
/// can work in; and an unreadable hooks directory stops the launch anyway, in
/// `render::merge_hooks`, which is where it should.
fn say_hooks(paths: &Paths, ctx: &out::Ctx) -> Option<notice::Record> {
    // An unreadable stacks directory is the same class of non-fatal as the rest
    // of this function: it costs the drift report, not the session. Reported
    // and withdrawn, never defaulted to empty — `notice::hooks` reads "no
    // definitions" as "no stack answers to that name", so an empty list does
    // not weaken the report, it inverts it and prints the inversion in omh's
    // own voice.
    let defs = match stack::load_all(&paths.stacks(), &paths.repo_stacks()) {
        Ok(defs) => defs,
        Err(e) => {
            ctx.warn(&format!(
                "could not read your stacks, so this repo's hooks went unchecked — {e:#}"
            ));
            return None;
        }
    };
    // The same withdrawal for the same reason: which ecosystems are covered and
    // what each hook file claims both come from reading the hook directories,
    // and a report built on half of that is a wrong report rather than a
    // shorter one.
    // Same withdrawal for the same reason: what each hook file claims comes
    // from reading the hook directories, and a drift report built on half of
    // that is a wrong report rather than a shorter one.
    let dirs = match Profile::resolve(paths).sources(adapter::Capability::Hooks) {
        Ok(dirs) => dirs,
        Err(e) => {
            ctx.warn(&format!("could not read your hooks — {e:#}"));
            return None;
        }
    };
    let declared = match render::declared_stacks(&dirs) {
        Ok(declared) => declared,
        Err(e) => {
            ctx.warn(&format!(
                "could not read your hooks, so drift went unchecked — {e:#}"
            ));
            return None;
        }
    };
    let detected = stack::detected(&defs, &paths.repo);
    match notice::hooks(paths, &detected, &declared) {
        Ok((notices, record)) => {
            for notice in notices {
                ctx.warn(&notice.to_string());
            }
            Some(record)
        }
        Err(e) => {
            ctx.warn(&format!("could not check this repo's hooks — {e:#}"));
            None
        }
    }
}

/// Say what this repo is not using from your catalogue, and what it named that
/// nothing answers to.
///
/// Called from **every path that builds a plan**, which is the rule `say_rules`
/// states and this broke on arrival: it was wired into `run` alone, so `attach`
/// and `doctor` composed the same profile and said nothing. `attach` is the path
/// where it matters most — it is how you rejoin a session that staged the
/// selection you have since changed.
///
/// Beside `say_hooks` and on the same terms otherwise: reported on every launch
/// including a dry run, never fatal. A selection omh cannot compute is not a
/// reason to refuse a session — and it cannot be one, because the report exists
/// to cover a silence rather than to guard anything.
fn say_selection(paths: &Paths, profile: &Profile, repo: &settings::RepoPolicy, ctx: &out::Ctx) {
    // Resolved here rather than inside `notice`: which ecosystems this repo is
    // takes the stack definitions and the checkout, and a report module that
    // read those would be deciding what it is meant to describe.
    let applicable = match catalogue_lists(paths) {
        Ok(lists) => lists,
        Err(e) => {
            ctx.warn(&format!("could not check what this repo uses — {e:#}"));
            return;
        }
    };
    match notice::selection(profile, &repo.selection, &applicable) {
        Ok(notices) => {
            for notice in notices {
                ctx.warn(&notice.to_string());
            }
        }
        Err(e) => ctx.warn(&format!("could not check what this repo uses — {e:#}")),
    }
}

/// Mark this repo's hooks as seen, now that a session is actually running.
///
/// Deliberately after the container is up rather than beside the report. The
/// snapshot is what makes "new or changed" fire exactly once, so writing it
/// from a launch that then died — Docker not running, an image that would not
/// build — spent the one notification about somebody else's executable content
/// changing under you, and the retry was silent. A dry run never gets here at
/// all, which is the other half of the same rule.
fn remember_hooks(record: Option<notice::Record>, ctx: &out::Ctx) {
    if let Some(record) = record {
        if let Err(e) = record.commit() {
            // The check succeeded and its notices are already printed; only the
            // bookkeeping failed. Saying "could not check" would send the user
            // looking at their hooks instead of at `~/.omh/run`.
            ctx.warn(&format!("this repo's hooks were not recorded — {e:#}"));
        }
    }
}

/// Copy the checkout's untracked essentials into a worktree, and say what
/// happened — a `.env` you thought you were carrying and are not is exactly the
/// failure that wastes an hour inside the sandbox.
fn carry_in(paths: &Paths, session: &Session, ctx: &out::Ctx) -> Result<()> {
    // The rules themselves are mounted, not written here — this covers the
    // empty placeholder each mount lands on, and any backend that cannot mount
    // a single file. It must run before `plan` places those placeholders.
    carry::hide_staged_rules(&session.worktree)?;

    let patterns = config::policy_list(paths, "carry_in");
    if patterns.is_empty() {
        return Ok(());
    }
    for item in carry::apply(&paths.repo, &session.worktree, &patterns)? {
        match item.action {
            // What was carried is progress, not a warning: it is the launcher
            // saying what it did, and it happens on every normal launch.
            // A directory is carried and *not* protected: `stage_for_mount`
            // mounts files only, so `git clean -fdx` in the sandbox removes a
            // carried directory and everything in it — silently, and with a
            // success code. Said here because `carried certs/` and `carried
            // .env` are otherwise the same sentence for two different fates,
            // and only one of them survives the agent tidying up.
            carry::Action::Copied | carry::Action::Refreshed
                if paths.repo.join(item.path.trim_end_matches('/')).is_dir() =>
            {
                ctx.warn(&format!(
                    "carried {} — a directory, so `git clean` in the sandbox can \
                     still remove it. Carrying the files individually keeps them.",
                    item.path
                ));
            }
            carry::Action::Copied => ctx.progress(&format!("carried {}", item.path)),
            carry::Action::Refreshed => ctx.progress(&format!("refreshed {}", item.path)),
            // The mistake, named where it is made rather than three commands
            // later at `s commit`. `carry_in` is for what a worktree does not
            // get; a tracked file is already on the branch.
            carry::Action::AlreadyTracked => ctx.warn(&format!(
                "carry_in lists {} — git already tracks it, so the worktree has it \
                 already. Not carried; drop it with `omh repo set carry_in`.",
                item.path
            )),
            carry::Action::Missing => ctx.warn(&format!(
                "carry_in lists {} — not in this checkout",
                item.path
            )),
            carry::Action::Unchanged => {}
        }
    }
    Ok(())
}

fn run(cwd: &std::path::Path, argv: &[String], cli: &Cli, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let name = &argv[0];

    let adapter =
        Adapter::find(&paths.adapters(), name).map_err(|e| unknown_tool(&paths, name, e))?;
    let profile = Profile::resolve(&paths);

    // A dry run must leave no trace: no branch, no worktree, no staged files.
    // Which identity this session runs as. Ambiguity is an error rather than a
    // guess: silently using the wrong account is expensive and invisible.
    let configured = policy_value(&paths, "account");
    let account = auth::resolve_for_launch(
        &paths,
        &adapter,
        cli.account.as_deref(),
        configured.as_deref(),
    )?
    .map(|a| auth::dir(&paths, name, &a));
    if let Some(account_dir) = &account {
        // The mountpoints have to exist before docker binds over them.
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }

    // Always the trunk, never wherever HEAD happens to be: a session started on
    // a feature branch produces a diff against the wrong baseline. You attach to
    // a session, not to a branch — choosing a base was a knob nobody needed.
    //
    // Resolved before the options rather than beside the session, because the
    // plan needs it too: it is where the project's own rules come from when the
    // worktree has none of its own.
    let base = session::default_branch(&paths.repo);
    let (own, repo) = resolved(&paths)?;
    let mut sandbox = sandbox(&paths, &adapter, &repo)?;
    // Not on a dry run, which promises to leave no trace: topping up starts a
    // container and writes `~/.omh/facts.json`. What is already cached is used,
    // so the plan it prints is the plan a real launch would build from the same
    // knowledge.
    if !cli.dry_run {
        if let Ok(backend) =
            runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))
        {
            sandbox.top_up(
                &paths,
                backend.program(),
                &adapter,
                &profile.sources(adapter::Capability::Hooks)?,
                &own,
                &repo,
                ctx,
            )?;
        }
    }

    let opts = container::Options {
        // A dry run must leave no trace: no branch, no worktree, no staged files.
        staging: if cli.dry_run {
            container::Staging::Skip
        } else {
            container::Staging::Apply
        },
        persist: policy_value(&paths, "persistence")
            .as_deref()
            .unwrap_or("dtach")
            .parse()?,
        tty: true,
        account_dir: account,
        memory_bin: memory::deliver::available(&paths),
        base: Some(base.clone()),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };

    std::fs::create_dir_all(paths.worktrees())?;
    if let Some(explicit) = cli.session.as_deref() {
        session::validate_id(explicit)?;
    }
    let id = session::pick(&paths.worktrees(), cli.session.as_deref(), cli.new);
    let session = Session::new(&paths.worktrees(), id);
    if opts.staging == container::Staging::Apply {
        session.ensure(&paths.repo, &base)?;
        carry_in(&paths, &session, ctx)?;
        // Reap before starting another container, and record that this one is
        // in use so it is not reaped by the next launch.
        reap_idle(&paths, &session.id, ctx);
        let _ = idle::touch(&paths.runs(), &session.id);
    }

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &argv[1..],
        opts.clone(),
    )?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    plan.validate(&backend.caps())?;

    say_rules(&plan, ctx);
    say_selection(&paths, &profile, &opts.repo, ctx);
    let hooks_seen = say_hooks(&paths, ctx);

    // Without the `omh: ` prefix, which `Ctx` now owns: the launch line is a
    // diagnostic and goes through the same voice as every other one, so it
    // paints and prefixes the same way.
    let status_line = match plan.degradation() {
        Some(d) => format!("{} on {} — {d}", adapter.name, session.label()),
        None => format!("{} on {}", adapter.name, session.label()),
    };

    if cli.dry_run {
        ctx.say(&report::DryRun {
            status: status_line,
            worktree: session.worktree.display().to_string(),
            argv: std::iter::once(backend.program().to_string())
                .chain(backend.args(&plan))
                .collect(),
        });
        return Ok(());
    }

    // The session is a running container. Exec into it rather than starting a
    // throwaway, so MCP daemons stay warm and `omh code` has something to
    // attach to.
    //
    // "Many harnesses take turns inhabiting it" is what this comment used to
    // claim, and it was not true: an image is built per harness, so the second
    // harness execed a binary the image does not contain. `session_up` restarts
    // on that mismatch now — a few seconds, not instant. Making it instant again
    // means one image carrying every installed harness.
    let (backend, name) = session_up(
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            tty: false,
            ..opts.clone()
        },
        &sandbox.recipe(),
        ctx,
    )?;
    // The container is up, so the launch happened and the call-out is spent.
    remember_hooks(hooks_seen, ctx);
    ctx.announce(&status_line);
    let status = Command::new(backend.program())
        .args(backend.exec_args(&name, &plan.argv, true))
        .status()?;
    // `omh s01 diff`, not `omh diff`. There is no top-level `diff` — the name
    // is not in `RESERVED`, so it falls through to the harness arm and comes
    // back as ``unknown harness `diff` ``. This line has been wrong since it
    // was written, in two different ways: it named a positional that the
    // session prefix has since deleted, so `the_session_lines_omh_prints_are_
    // lines_omh_accepts` now reads it, and would have caught both.
    ctx.hint(&format!("\nreview with  omh {} diff", session.id));
    std::process::exit(status.code().unwrap_or(1));
}

/// The two things a launch needs from outside the plan: what omh contributes,
/// and what this repo decided.
///
/// Resolved by the caller of `container::plan` rather than inside it, the rule
/// `memory_bin` and `base` already follow — the manifest is a file, and a probe
/// inside `plan` is a probe no test can reach.
///
/// Returned as a pair rather than merged. They arrive together and travel
/// together, which is exactly what made one struct tempting, but "omh generated
/// this" and "this repo asked for this" are the two answers `omh why` exists to
/// keep apart — and a type that holds both cannot help blurring them.
fn resolved(paths: &Paths) -> Result<(base::Own, settings::RepoPolicy)> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let repo = settings::resolve(paths, &manifest)?;
    // What the catalogue still declares, so removing a server takes its feature
    // with it. `omh config mcp rm codegraph` edits `mcp.json` and nothing
    // else, so this read is where that instruction is kept or broken.
    let installed = config::servers(paths)?.into_iter().map(|s| s.key).collect();
    Ok((base::own(&manifest, &repo.off, &installed)?, repo))
}

/// `omh why <thing>` — who put this here, and on what grounds.
///
/// Needs no container and no session: it is a pure function of the manifest and
/// the resolved profile, which is why it can answer even for something you have
/// removed.
fn why_cmd(cwd: &std::path::Path, thing: &str, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;

    // Servers and hooks are the same kind of thing here: installed, from a
    // layer, chosen by omh or by you.
    let mut installed = config::servers(&paths)?;
    installed.extend(config::hooks(&paths)?);

    // What omh ships, for deciding whether your copy has been changed. MCP
    // servers only: hooks and rules sections are generated at launch, so there
    // is nothing of yours to compare — a file of that name is a leftover, and
    // the `Generated` verdict names it as one rather than as your edit.
    let baselines: std::collections::BTreeMap<String, String> = manifest
        .entries
        .iter()
        .filter_map(|e| e.command.clone().map(|c| (e.name.clone(), c)))
        .collect();

    // Hooks that belong to a detected ecosystem are omh's opinion about that
    // ecosystem, not about this repo. Reported as neither the base set nor
    // yours, because claiming either would be false in a way this command
    // exists to prevent.
    //
    // The command travels with the name, so the claim is checkable: this reads
    // the hook that would actually ship rather than matching on a name anyone
    // could give a file. What changed with the catalogue is where the body
    // comes from — the file, not a `match` in Rust — and that a repo shadowing
    // the name is reported with *its* command, which is the honest answer.
    let mut derived = std::collections::BTreeMap::new();
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected = stack::detected(&stack_defs, &paths.repo);
    let (own, repo_policy) = resolved(&paths)?;
    let merged = render::merge_hooks(
        &Profile::resolve(&paths).sources(adapter::Capability::Hooks)?,
        &own,
        &repo_policy,
    )?;
    for (name, hook) in &merged {
        let Some(stack) = hook.stack.as_deref() else {
            continue;
        };
        let Some(def) = detected.iter().find(|d| d.name == stack) else {
            continue;
        };
        derived.insert(
            name.clone(),
            why::Derived {
                from: format!("{}, detected from {}", def.name, def.marker),
                command: hook.does().to_string(),
                layer: config::Layer::Shared,
            },
        );
    }

    let source = manifest.source();
    let version = manifest.version.clone();
    let catalog = why::Catalog {
        off: settings::resolve(&paths, &manifest)?.off,
        manifest: &manifest,
        baselines,
        installed,
        derived,
    };
    ctx.say(&report::Why {
        thing: thing.to_string(),
        text: why::render_with_source(&catalog, &catalog.why(thing), &version, &source),
    });
    Ok(())
}

fn init(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    // Fail fast. Everything below is wasted work outside a repo.
    let paths = Paths::discover(cwd)?;

    // Filled in as the run goes and reported once at the end. See
    // `report::Init` for why this is not printed as it happens.
    let mut summary = report::Init::default();

    // A fresh install has no adapters, so `omh <harness>` would fail no matter
    // what else init did. Ship them before anything else.
    let adapters = install_bundled_adapters(&paths, ctx)?;
    let editors = install_bundled(&paths.editors(), bundled::Shipped::Editors, ctx)?;
    // The base set ships as data next to the adapters, for the same reason: the
    // opinion should be reviewable by the people it is imposed on. It travels
    // *inside* the binary now — otherwise a released omh installs nothing — but
    // it still lands as a file in `~/.omh/base`, which is where the
    // reviewability actually lives. `omh why` reads the file init seeds from.
    install_bundled(&paths.base(), bundled::Shipped::Base, ctx)?;
    // The stacks, for the same reason and by the same route: what a project
    // needs installed is omh's opinion, and an opinion imposed on somebody
    // should be one they can read. Managed, so a shipped fix always lands.
    install_bundled(&paths.stacks(), bundled::Shipped::Stacks, ctx)?;
    // And the conventional hooks, which used to be a `match` in Rust written
    // into every repo as two files. As catalogue data they are one body per
    // ecosystem instead of one per checkout, so a fix reaches everybody; a repo
    // needing its own spelling shadows the name, which is the rule hooks
    // already had. Each names the stack it belongs to and nothing else about
    // it — the marker stays in `stacks/`, so the two cannot drift.
    install_bundled(&paths.hooks(), bundled::Shipped::Hooks, ctx)?;
    // And the markers: ecosystems omh can recognise and cannot yet set up.
    // Data rather than a `match` for the same reason the stacks are — a marker
    // is removed by the same release that ships its stack, and the curation
    // test refuses the pair being true at once.
    install_bundled(&paths.markers(), bundled::Shipped::Markers, ctx)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    std::fs::create_dir_all(paths.worktrees())?;

    // The catalogue, empty and ready. Created rather than left absent so
    // `omh config edit` has somewhere to open and the shape is discoverable
    // without reading a document.
    for cap in adapter::Capability::ALL {
        if cap != adapter::Capability::Mcp {
            std::fs::create_dir_all(paths.root.join(cap.source()))?;
        }
    }

    // Detect rather than ask — from the stacks just installed above, so this,
    // the provisioning below and the hook catalogue all read one set of
    // definitions rather than registries free to drift.
    //
    // One list now, where there used to be two: detection filtered through a
    // view that dropped any stack omh had no hook opinion about, so a
    // contributed ecosystem was provisioned and invisible in the report. A hook
    // names its stack instead, so a stack with no hooks is simply a stack with
    // no hooks — visible, provisioned, and waiting for somebody to contribute
    // one.
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let stacks = stack::detected(&stack_defs, &paths.repo);
    let names: Vec<String> = adapters.to_vec();
    let harness = detect::preferred_harness(&names, &|h| runtime::installed(h));

    // What a repo holds: settings, memory configuration, and hooks. No skills,
    // no MCP servers, no commands, no subagents — those are yours, and a repo
    // names them rather than shipping them.
    let repo_omh = paths.repo.join(".omh");
    std::fs::create_dir_all(repo_omh.join("hooks"))?;
    // Both halves of the note store. The committed half lives in the repo
    // because that is what makes it reach a teammate; the local half lives
    // under `~/.omh`, because a worktree holds only tracked files and
    // `omh s rm` removes it with `--force`.
    for layer in memory::Layer::ALL {
        std::fs::create_dir_all(layer.dir(&paths))?;
    }
    // `write_if_absent`, never the refresh path the adapters use: a shipped
    // template that changed under an existing store would silently re-key
    // every note in it, and every existing key would stop being derivable.
    write_if_absent(&repo_omh.join(memory::TEMPLATES), memory::SHIPPED_KEYS)?;
    // No `AGENTS.md` is written. omh's own sections are base-set entries,
    // composed into every session from the manifest, which is what lets a fix
    // reach a repo that ran `init` a year ago. The detected stack is not prose
    // either: it produces hooks, and a sentence describing a test command is
    // not the thing that runs it.

    // The base set: omh's opinion, seeded into your catalogue where it is
    // visible, reviewable, and removable rather than hidden in the binary.
    // `write_if_absent`, so a server you removed does not come back.
    let base_mcp =
        serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": manifest.servers() }))?
            + "\n";
    write_if_absent(&config::mcp_path(&paths), &base_mcp)?;
    write_if_absent(
        &repo_omh.join("settings.toml"),
        "# What this repo decided. Settings at the top level; `[omh]` switches\n\
         # omh's own features off here without uninstalling anything.\n\
         #\n\
         # Untracked files the worktree needs — a worktree holds only tracked\n\
         # files, so without this the agent lands somewhere that cannot run your\n\
         # app. This is the ONLY path by which a secret reaches the agent, so\n\
         # keep it short and explicit. node_modules belongs in the image, not here.\n\
         #\n\
         # carry_in = [\".env.local\", \"certs/\"]\n\
         carry_in = []\n\
         \n\
         # [omh]\n\
         # codegraph = false\n",
    )?;
    // No hooks are seeded into the repo. omh's own are generated from the
    // manifest at launch, which is the only arrangement in which omh can ship a
    // fix to them: `write_if_absent` never revisits, so a repo initialised
    // before `git-unavailable` was rewritten would have run the broken pattern
    // forever. The conventional ones are catalogue files for the same reason —
    // `cargo test` is what a rust project runs, not what *this* rust project
    // runs, so one body per ecosystem is the honest scope and a fix reaches
    // everybody who already ran `init`.
    //
    // What a repo still declares is a hook only it could want, in
    // `<repo>/.omh/hooks/`, which shadows a catalogue name by the rule
    // `merge_hooks` already applies. That is the whole of what changed: the
    // *scope* of the conventional hooks, not whether a repo may have its own.
    //
    // Some of those omh can work out. A node project's test command depends on
    // which package manager it uses and whether it declared a `test` script at
    // all, so the catalogue cannot hold it and `derive` reads it off the files
    // the project already commits — for ecosystems the catalogue does not
    // already cover, so a rust repo's `Makefile` does not earn a second hook
    // that runs the suite again.
    //
    // `write_if_absent`, so a hook somebody has since edited is never
    // rewritten, and **serialised** rather than formatted: a command with a
    // quote in it — which is now a command omh read out of somebody's
    // `package.json` rather than one of four literals — would otherwise
    // produce a file nothing can parse.
    let covered = covered_here(&[paths.hooks()], &stacks)?;
    let derived = derive::hooks(
        &paths.repo,
        &settings::resolve(&paths, &manifest)?.provision,
        &covered,
    );
    if !derived.is_empty() {
        std::fs::create_dir_all(repo_omh.join("hooks"))?;
        for d in &derived {
            write_if_absent(
                &repo_omh.join("hooks").join(format!("{}.json", d.name)),
                &format!("{}\n", serde_json::to_string_pretty(&d.hook)?),
            )?;
        }
    }

    // And only now, the two questions — after every derivation has had its go,
    // which is what makes them *last* resort rather than a wizard's opening.
    //
    // Two conditions, both narrow. A marker omh recognises and no stack claims
    // is the one case where the repo plainly is something and omh cannot say
    // what its sandbox needs. A project with no test hook from any source is
    // the one case where the agent cannot check its own work.
    let markers = stack::markers(&paths.markers())?;
    let unclaimed = stack::unclaimed(&markers, &stack_defs, &paths.repo);
    let has_test = covered.iter().any(|s| stacks.iter().any(|d| &d.name == s))
        || derived.iter().any(|d| d.hook.on == hook::Event::TurnEnd)
        || repo_omh.join("hooks").join("test.json").exists();
    let (asked, answered) = questions(&repo_omh, &unclaimed, has_test, ctx)?;

    // **Reloaded, because an answer is a stack file.** `how_is_it_installed`
    // writes `<repo>/.omh/stacks/<name>.toml`, and everything below — the
    // report, the predicates, the recorded resolution, the image layer — reads
    // `stack_defs`. Left stale, somebody typed how to install elixir, watched
    // omh say `stack elixir — from what you told it`, and then watched the same
    // run print `stack none detected` and build a sandbox with no elixir in it.
    // Their answer took effect on the *next* `init`, and nothing said so.
    //
    // Unconditional rather than gated on `asked > 0`: it costs one directory
    // read, and a gate is a second thing to keep true.
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let stacks = stack::detected(&stack_defs, &paths.repo);
    //
    // The selection, written out with every catalogue entry named — after the
    // catalogue is installed and the derived hooks are written, so both are in
    // the list it writes.
    //
    // Expanded rather than `"*"`, because an explicit list is editable and
    // reviewable in a way a wildcard is not: you curate by deleting lines. That
    // has one failure mode — an entry added to the catalogue *afterwards* is not
    // in the list, so it is off and the reason is invisible — and the launcher
    // reports exactly that, which is what makes writing it expanded safe.
    //
    // Only when there is no `[use]` already: `write_if_absent` guards the file,
    // not the table, and re-running `init` in a curated repo must not resync a
    // list somebody pruned on purpose. `omh use --all` is how you ask for that.
    if !repo_has_selection(&paths)? {
        let lists = catalogue_lists(&paths)?;
        config::write_selection(&paths, config::Layer::Shared, &lists)?;
    } else {
        // A curated list is not resynced — and a hook **this run just wrote**
        // still has to reach it, or it lands dead. `merge_hooks` drops any hook
        // the selection does not name, so a repo `init`ed six months ago that
        // has since gained a `package.json` gets `pnpm-test.json` written, sees
        // it reported, and never runs it. `import_hooks` already guards exactly
        // this; the same rule applies to what `init` writes.
        //
        // Added, never resynced: the point of a curated list is that omh does
        // not put back what somebody pruned. These are names that did not exist
        // when they pruned it.
        let mine: Vec<String> = derived
            .iter()
            .map(|d| d.name.clone())
            .chain(answered.iter().cloned())
            .collect();
        if !mine.is_empty() {
            let (cap, mut names, _) = current_list(&paths, "hooks", &mine[0])?;
            names.extend(mine);
            names.sort();
            names.dedup();
            let lists = std::collections::BTreeMap::from([(cap, names)]);
            write_lists(&paths, &lists)?;
        }
    }

    // Appended, not overwritten: re-running init must not eat a line you added.
    let gitignore = paths.repo.join(".omh/.gitignore");
    // Left tracked, a machine-local override gets committed to the team's repo.
    ensure_line(&gitignore, settings::LOCAL)?;

    // Only now the image, and the question about what it turned out to hold.
    //
    // Everything above configures the repo and cannot fail for want of a
    // container; everything here needs one and propagates when there is none.
    // Ordered this way round deliberately: an earlier arrangement built the
    // image first, so `omh init` on a box with no runtime — somebody who
    // installed omh before docker, which is the order most people do it in —
    // left the repo with hooks, no `[use]` list, and `settings.local.toml`
    // still tracked. Setting a repo up must not be abandoned half-done because
    // the machine cannot build an image yet.
    // Which of this repo's hooks the sandbox turned out to be unable to run.
    // Measured, not asked about — see the block below.
    let mut held_back: Vec<hook::Dropped> = Vec::new();
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        // Without it the headline command cannot run, so init is not finished
        // until this exists — and until it exists there is no sandbox to ask
        // about a toolchain.
        if image::exists(backend.program(), &image::tag_for(&adapter)) {
            summary.image = Some(format!("{} (already built)", image::tag_for(&adapter)));
        } else {
            // Progress, not report: this is the minutes-long step, and
            // somebody watching a blank terminal needs to know it is alive.
            ctx.progress(&format!(
                "building {} — first run only…",
                image::tag_for(&adapter)
            ));
            image::ensure(backend.program(), &adapter)?;
            summary.image = Some(image::tag_for(&adapter));
        }

        // Which provides apply here. Evaluated **in the sandbox**, with the repo
        // mounted read-only: a predicate is arbitrary shell out of a stack file,
        // and running it on the host during `init` is the one thing omh exists
        // to avoid.
        let detected = stack::detected(&stack_defs, &paths.repo);
        let candidates: Vec<(String, Option<&str>)> = detected
            .iter()
            .flat_map(|d| {
                d.provides
                    .iter()
                    .map(move |p| (stack::key(&d.name, &p.name), p.when.as_deref()))
            })
            .collect();

        {
            // No `if !candidates.is_empty()` guard. A repo with nothing to ask
            // has still been answered — the answer is "nothing applies" — and
            // skipping would leave a resolution recorded when this repo *was* a
            // rust project asserting `rust/toolchain = true` for ever.
            let answered = if candidates.is_empty() {
                Vec::new()
            } else {
                match Command::new(backend.program())
                    .args(stack::predicate_args(
                        &image::tag_for(&adapter),
                        &paths.repo,
                        &stack::predicate_script(&candidates),
                    ))
                    .output()
                {
                    // A container that ran and failed is not an answer. Only
                    // `Err` was handled before, so `docker run` failing — image
                    // gone, mount refused, no space — produced empty stdout,
                    // read as "nothing applies", and `init` went on to print
                    // its summary with nothing said. The `Err` arm's own
                    // comment forbids exactly that.
                    Ok(out) if !out.status.success() => {
                        summary.provision_problems.push(format!(
                            "the sandbox could not be asked ({}) — nothing recorded",
                            out.status
                        ));
                        for line in String::from_utf8_lossy(&out.stderr).lines().take(3) {
                            summary.provision_problems.push(line.to_string());
                        }
                        Vec::new()
                    }
                    Ok(out) => doctor::parse(&String::from_utf8_lossy(&out.stdout)),
                    Err(e) => {
                        // Non-fatal, and never fatal *silently*: `init` sets a
                        // repo up, and failing that over a diagnostic would be
                        // the tail wagging the dog — but saying nothing would
                        // let somebody believe the sandbox had been checked.
                        summary.provision_problems.push(format!(
                            "could not ask the sandbox ({e}) — nothing recorded"
                        ));
                        Vec::new()
                    }
                }
            };

            for a in answered.iter().filter(|a| !a.ok) {
                if let stack::Verdict::CouldNotAnswer(code) = stack::verdict(a) {
                    summary.provision_problems.push(format!(
                        "{}'s condition could not answer{} — not applied",
                        a.name,
                        code.map(|c| format!(" (exit {c})")).unwrap_or_default()
                    ));
                }
            }

            // Recorded only when something was actually measured. `reconcile`
            // drops every `true` it is not told about, so writing an empty
            // answer would erase the repo's resolution rather than leave it be.
            if let Some(fired) = fired_from(candidates.len(), &answered) {
                let recorded = record_resolution(&paths, &fired)?;
                for key in recorded.iter().filter(|(_, on)| **on).map(|(k, _)| k) {
                    summary.provisioned.push(key.clone());
                }

                // The stack layer, through the same function every launch
                // reads — so what `init` reports built is what `omh run` runs,
                // by construction rather than by two implementations agreeing.
                //
                // Re-resolved from disk rather than reusing `recorded`, which
                // is the committed table alone: `record_resolution` has just
                // written it, and a `false` in `settings.local.toml` means *not
                // on this laptop*, which is the laptop building the image.
                let (own, repo) = resolved(&paths)?;
                let sandbox = sandbox(&paths, &adapter, &repo)?;
                image::ensure_stack(backend.program(), &adapter, &sandbox.recipe(), &paths.repo)?;
                if sandbox.tag != image::tag_for(&adapter) {
                    summary.stack_image = Some(sandbox.tag.clone());
                }

                // And what that image turned out to contain, measured once and
                // remembered: every launch afterwards reads `~/.omh/facts.json`
                // rather than starting a container to ask again.
                //
                // Two readings of one probe. A `needs` that did not resolve is
                // a **provisioning failure** — the recipe ran and the
                // environment still does not work, which is exactly what
                // shipping rustup with no `cc` looked like. The same
                // measurements hold back a hook whose program is missing, which
                // is a different question about the same fact.
                let hook_dirs = Profile::resolve(&paths).sources(adapter::Capability::Hooks)?;
                let mut sandbox = sandbox;
                sandbox.top_up(
                    &paths,
                    backend.program(),
                    &adapter,
                    &hook_dirs,
                    &own,
                    &repo,
                    ctx,
                )?;
                for name in &sandbox.owed {
                    if sandbox.resolves.get(name) == Some(&false) {
                        summary
                            .provision_problems
                            .push(format!("{name} did not resolve after installing"));
                    }
                }

                // And the other reading, through the launcher's own function so
                // `init` cannot report one thing and a launch do another.
                //
                // No question here any more, and that is the point of the whole
                // design. What stood here asked, for every program the sandbox
                // lacked, whether to switch its hook off — and recorded the
                // answer in a committed file. It was asking somebody to
                // configure around a broken environment, and the answer
                // outlived the breakage: a repo whose sandbox later gained
                // `cargo` still had `cargo = "skip"` on file, so the hook
                // stayed off for everybody who cloned it, with nothing to
                // re-ask. Now nothing is on file, because nothing had to be
                // decided.
                held_back = render::held_back(&hook_dirs, &own, &repo, &sandbox.resolves)?;
            }
        }
    }
    // No harness is no image, and no image is no sandbox to ask about. The
    // hooks are already written either way.

    // Report every decision, so `omh why` has something to explain. Printed as
    // each one is made rather than collected for the end, which is why the
    // image and graph lines below appear inside the summary.
    // The headline is a claim about this run, so it has to be able to stop
    // being true. omh derives what it can and asks only what nothing could
    // derive; printing "asked nothing" after putting a question on screen would
    // make the promise the tagline is selling into a thing the user just
    // watched it break.
    //
    // Counted from what was actually *put*, not from what was answered — a
    // question declined was still a question asked, and claiming otherwise
    // would let omh interrogate somebody and then deny it.
    summary.asked = asked;
    summary.adapters = adapters.clone();
    summary.editors = editors.clone();
    summary.harness_on_host = harness.as_deref().is_some_and(runtime::installed);
    summary.harness = harness.clone();
    summary.stacks = stacks
        .iter()
        .map(|s| (s.name.clone(), s.marker.clone()))
        .collect();
    // Named, with the evidence, because the alternative is the failure this
    // whole design replaces: a hook that runs on turn one and reports
    // `cargo: not found`, saying nothing about who decided to run cargo or
    // where it looked.
    //
    // "will not run", and it is safe to say so now. This list comes from
    // `render::held_back`, which is the function the launcher itself uses — so
    // a hook named here is a hook the session will not ship, rather than one
    // omh hoped somebody would go and disable.
    //
    // The hook file stays where it is either way. `.omh/hooks/` is the repo's
    // statement about itself and it is committed; whether a program exists is
    // a fact about one image, and it decides what runs here, never what the
    // repo contains.
    summary.held_back = held_back
        .iter()
        .map(|d| (d.name.clone(), d.wanted.clone()))
        .collect();

    // Hooks somebody already has, somewhere omh can see them. **Noticed, never
    // acted on**: importing writes executable content into the repo, and doing
    // that because `init` happened to find a file is not a decision omh gets to
    // make on somebody's behalf. It says what is there and what would bring it
    // across.
    summary.importable = importable(&paths, &adapters);

    // What the repo already documents becomes notes that *point* at it.
    // Printing the seeds instead would derive them every run, show them once,
    // and keep them nowhere.
    summary.memory = match seed_store(&paths) {
        Ok(report) => report,
        // Never fatal. A repo that cannot be ingested is still a repo omh set
        // up, and failing `init` over the note store would be the tail
        // wagging the dog.
        Err(e) => format!("not seeded: {e:#}"),
    };

    summary.catalogue_dir = paths.root.display().to_string();
    summary.repo_dir = repo_omh.display().to_string();
    // The index lives in a container volume, so it has to be built inside the
    // sandbox — one built on the host would land where no session can read it.
    if let Some(h) = &harness {
        let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
        let adapter = Adapter::find(&paths.adapters(), h)?;
        let args = base::index_args(
            &image::tag_for(&adapter),
            &paths.cache_volume(),
            &paths.repo,
            &paths.repo_name(),
        );
        match Command::new(backend.program())
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Backgrounded: init returns now and the first launch waits only if
            // this has not finished.
            Ok(_) => {
                summary.graph = Some(format!("indexing in background → {}", paths.cache_volume()))
            }
            Err(e) => summary.graph = Some(format!("could not start indexing: {e}")),
        }
    }

    summary.base_set = manifest.version.to_string();
    summary.rationale = manifest
        .rationale()
        .into_iter()
        .map(|(name, why)| (name.to_string(), why.to_string()))
        .collect();
    summary.next_command = harness.as_deref().unwrap_or("config").to_string();

    ctx.say(&summary);
    Ok(())
}

/// Adapters ship with omh but live in `~/.omh`. Without this a fresh install
/// cannot launch anything, which is the state the tool was in until now.
fn install_bundled_adapters(paths: &Paths, ctx: &out::Ctx) -> Result<Vec<String>> {
    install_bundled(&paths.adapters(), bundled::Shipped::Adapters, ctx)?;
    Ok(Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect())
}

/// Put the two questions of last resort, and write down what comes back.
///
/// **A terminal is a precondition, not a fallback.** With stdin closed — a CI
/// runner, a script — nothing is asked and nothing is written, which is the
/// same outcome as declining and is reached without printing a prompt nobody
/// can answer. `ask::prompt` reads EOF as a stop for the same reason.
///
/// Returns how many questions were actually put, so `init`'s headline can stop
/// claiming it asked nothing the moment it did.
///
/// `write_if_absent`, so an answer somebody has since edited is never
/// overwritten by a later `init` re-asking and getting a different reply.
fn questions(
    repo_omh: &std::path::Path,
    unclaimed: &[&stack::Marker],
    has_test: bool,
    ctx: &out::Ctx,
) -> Result<(usize, Vec<String>)> {
    if unclaimed.is_empty() && has_test {
        return Ok((0, Vec::new()));
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Ok((0, Vec::new()));
    }

    let stdin = std::io::stdin();
    let (asked, answers) = ask_all(
        unclaimed,
        has_test,
        &mut stdin.lock(),
        &mut std::io::stderr(),
    )?;

    let mut hooks = Vec::new();
    for a in answers {
        let path = repo_omh.join(&a.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_if_absent(&path, &a.body)?;
        // Confirmed as it happens rather than saved for the summary: the user
        // is sitting at a prompt they just answered, and the answer to "what
        // did that do" is owed now, not forty lines later.
        ctx.progress(&a.said);
        // Handed back so `init` can put it in `[use]`. A hook written into a
        // repo whose selection is already curated is one `merge_hooks` drops,
        // so an answered question would produce a file, a report line, and a
        // session that never runs it.
        if a.path.starts_with("hooks") {
            if let Some(stem) = a.path.file_stem() {
                hooks.push(stem.to_string_lossy().into_owned());
            }
        }
    }
    Ok((asked, hooks))
}

/// The exchange itself, with the terminal handed in.
///
/// Split from [`questions`] so its rules can be asserted at all — how many
/// questions were put, what a decline does to the ones after it, and that a
/// declined question is still a question asked.
fn ask_all(
    unclaimed: &[&stack::Marker],
    has_test: bool,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> Result<(usize, Vec<ask::Answer>)> {
    let mut asked = 0usize;
    let mut answers = Vec::new();

    for marker in unclaimed {
        asked += 1;
        match ask::how_is_it_installed(marker, input, out)? {
            Some(a) => answers.push(a),
            // **Stop the marker questions, rather than working through them.**
            // A decline and a closed pipe arrive here identically, and the one
            // that matters is the pipe: a polyglot repo with three unclaimed
            // markers would otherwise print three questions into a void and
            // count them. One "no" is answer enough to stop asking about the
            // rest — and the test question below is still put, because it is
            // the one most repos reach.
            None => break,
        }
    }
    // Asked last, because it is the question most repos reach and the one most
    // worth answering — putting it after an exchange somebody has already
    // declined would waste it.
    if !has_test {
        asked += 1;
        if let Some(a) = ask::what_tests_it(input, out)? {
            answers.push(a);
        }
    }
    Ok((asked, answers))
}

/// Copy definitions that ship with omh into `~/.omh`.
///
/// Bundled files are **managed**: they are refreshed on every `init`, because a
/// fix omh ships has to reach people who already ran it once. The one that
/// mattered was a wrong credential path, which made `omh auth` capture nothing
/// while reporting success. Definitions you add yourself are left alone.
///
/// The contents come from [`bundled`], embedded at compile time. Reading them
/// from the source tree instead is what made a released binary install nothing
/// at all — and say nothing, because the `read_dir` error was discarded.
fn install_bundled(
    dest: &std::path::Path,
    kind: bundled::Shipped,
    ctx: &out::Ctx,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("creating {} for the bundled {}", dest.display(), kind.dir()))?;
    for &bundled::File { name, contents } in kind.files() {
        let target = dest.join(name);

        // Bytes, not text. `read_to_string` fails on a single non-UTF-8 byte,
        // and treating that failure as "no file here" overwrote the file
        // without the backup promised below — the read failed, the write
        // succeeded, and somebody's edit was gone. Only "not found" means
        // absent; every other error is reported rather than assumed benign.
        let existing = match std::fs::read(&target) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", target.display())),
        };

        if !existing.is_empty() && existing != contents.as_bytes() {
            // Managed files are refreshed so shipped fixes land, but
            // silently discarding an edit is not acceptable.
            //
            // **Appended, never `with_extension`.** That replaces the
            // extension, so it produced the right name only while everything
            // omh shipped was TOML: an edited `rust-test.json` was saved as
            // `rust-test.toml.yours` while the line below said
            // `rust-test.json.yours`, and somebody looking where omh told them
            // to look would conclude their edit had been thrown away.
            let backup = target.with_file_name(format!("{name}.yours"));
            std::fs::write(&backup, &existing)
                .with_context(|| format!("saving your {name} as {}", backup.display()))?;
            // stderr: this is a warning about data, and stdout is the report.
            ctx.warn(&format!(
                "replaced {} — yours saved as {name}.yours",
                target.display()
            ));
        }
        std::fs::write(&target, contents)
            .with_context(|| format!("writing {}", target.display()))?;
    }

    // Not `.flatten()`. An unreadable entry here would be dropped from the
    // list omh then prints as `harnesses N (...)` and hands to
    // `detect::preferred_harness` — under-reporting and choosing from an
    // incomplete set, silently. That is the shape of bug this file just
    // finished removing.
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dest).with_context(|| format!("reading {}", dest.display()))? {
        let path = entry
            .with_context(|| format!("listing {}", dest.display()))?
            .path();
        if path.extension().is_some_and(|x| x == "toml") {
            names.push(path.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Append a line if absent. Rewriting the file would eat anything you added.
fn ensure_line(path: &std::path::Path, line: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    std::fs::write(path, out)?;
    Ok(())
}

fn write_if_absent(path: &std::path::Path, contents: &str) -> Result<()> {
    if !path.exists() {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Which provides applied, from what the predicates answered.
///
/// `None` when nothing was answered — the container never ran, the runtime
/// hiccuped, the image was missing. That case is not "nothing applies", and the
/// difference is destructive rather than academic: `stack::reconcile` drops
/// every `true` it is not told about, so recording an empty answer would erase
/// a repo's resolution and leave the next launch provisioning nothing.
///
/// A provide that could not answer is simply absent from the set, which is the
/// safe direction — it is not installed, so it is not recorded, so its `needs`
/// are not claimed and nothing reports a gap omh invented. Installing on a
/// coin-flip would be silent either way.
///
/// `asked` is how many provides there were to ask about, and it separates two
/// things an empty report cannot: **nothing to ask** is an answer, **nothing
/// answered** is silence. A repo that stops being a stack has no candidates and
/// runs no container, and that has to clear the resolution rather than preserve
/// it — otherwise `[provision]` keeps asserting `rust/toolchain = true` after
/// the `Cargo.toml` is gone, and the stack layer keeps installing a toolchain
/// nothing uses.
fn fired_from(asked: usize, answered: &[doctor::Outcome]) -> Option<BTreeSet<String>> {
    if asked == 0 {
        return Some(BTreeSet::new());
    }
    // One line per provide, so fewer lines than provides is a report that did
    // not finish — not a report saying "no". Accepting the prefix would make
    // `reconcile` drop every `true` it was not told about and rewrite a
    // committed file without them. The now-deleted `[toolchain]` question had
    // this same shape and had to be fixed for it, where it only cost a spurious
    // question; here it deletes.
    if answered.len() != asked {
        return None;
    }
    Some(
        answered
            .iter()
            .filter(|o| stack::verdict(o) == stack::Verdict::Applies)
            .map(|o| o.name.clone())
            .collect(),
    )
}

/// Write what fired into the repo's **shared**, committed settings, and hand
/// back what the file now says.
///
/// A function rather than four lines inline, because the layer it names on both
/// sides is the whole of its correctness and inline it is reachable only
/// through a container. Both halves are load-bearing in opposite directions:
///
/// - **Read `Shared`.** `reconcile` writes what it is given, so reading the
///   merge would take a `false` from `settings.local.toml` — one laptop's *not
///   here* — and commit it for everybody who clones.
/// - **Write `Shared`.** The resolution is the repo's, and a teammate cloning
///   it is the reason it lives in a committed file at all. Written to `Local`
///   it would be re-derived, and re-asked, on every machine.
fn record_resolution(paths: &Paths, fired: &BTreeSet<String>) -> Result<BTreeMap<String, bool>> {
    let recorded = stack::reconcile(
        &config::read_provision(paths, config::Layer::Shared)?,
        fired,
    );
    config::write_provision(paths, config::Layer::Shared, &recorded)?;
    Ok(recorded)
}

/// The recipes to run, in the order the stack files gave them.
///
/// File order is install order — `corepack enable pnpm` needs the node the
/// provide above it asserted — so this walks the definitions rather than the
/// resolution, which is a map sorted by name and would silently reorder them.
///
/// A provide with no `install` contributes nothing: it asserts the base image
/// already ships something, so it changes neither the recipe nor the tag.
///
/// **`resolved` is the only input**, and that is the point. It is the
/// `[provision]` table as all three settings layers resolve it: `init` writes
/// what its predicates found, a person may write `false` to opt out, and every
/// launch afterwards reads the same table. A launch that re-derived this from
/// anything else — the predicates it cannot run, a set of provides that
/// "fired" — would build a different image from the one `init` reported, and
/// the disagreement would be invisible because both are plausible.
///
/// Only `true` provisions. Absent is not `false` and does not need to be: an
/// entry nobody recorded is one no predicate has said applies here.
fn installs_for<'a>(
    detected: &[&'a stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> Vec<&'a str> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .filter_map(|(_, p)| p.install.as_deref())
        .collect()
}

/// What the stacks this repo provisions said must resolve once they had run.
///
/// Only provides the resolution recorded `true`. A provide nobody recorded was
/// never installed, and a provide somebody opted out of was deliberately not
/// installed — reporting either as a failure would be a gap omh invented. The
/// consequence of an opt-out is not silenced by that: if a hook names the
/// program, it is probed anyway through `render::hook_programs`, and a hook
/// that cannot run is dropped by name.
///
/// This includes provides with **no `install`**, which is the point of letting
/// them exist: `stacks/node.toml`'s `runtime` asserts the base image already
/// ships `node` and `npm`, and the only way that assertion is worth writing is
/// if something checks it.
fn needs_of(
    detected: &[&stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> BTreeSet<String> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .flat_map(|(_, p)| p.needs.iter().cloned())
        .collect()
}

/// Everything worth asking one image about: what the stacks promised, and what
/// the hooks will actually run.
///
/// **The union, and it has to be.** The two lists answer different questions
/// and neither contains the other. A stack's `needs` is what provisioning owes
/// — the reading that catches rustup installing a `cargo` that cannot link.
/// A hook's program is what will be handed to a shell — and a hand-written
/// `shellcheck` hook is in no `needs` list, so a probe built from `needs` alone
/// ships it into a sandbox that cannot run it. That is the original
/// `cargo: not found` with a different program in it.
fn probe_targets(
    hook_dirs: &[PathBuf],
    own: &base::Own,
    repo: &settings::RepoPolicy,
    owed: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut wanted = render::hook_programs(hook_dirs, own, repo)?;
    wanted.extend(owed.iter().cloned());
    Ok(wanted)
}

/// Ask the image about the programs nobody has asked it about yet, remember
/// the answers, and hand back everything known about it.
///
/// The cache is the reason a launch is not a container run: `Facts::unseen`
/// narrows the question to what has never been answered for this tag, and a
/// repo whose hooks and stacks have not changed asks nothing at all.
///
/// Never fatal. A runtime that will not start, an image that is not there, a
/// probe that says nothing — all of them leave the facts as they were, which
/// reads as *nobody has looked* and suppresses nothing. The alternative is a
/// diagnostic failure taking a launch down with it.
/// What a probe run amounts to: what it measured, or **why nobody could be
/// asked**.
///
/// Split out of `measure` because the reason is the whole value of the guard,
/// and a reason that only exists as an `eprintln!` inside a function that shells
/// out is a reason no test can see disappear.
///
/// A container that ran and **failed** is not an answer. Checking only the
/// `Err` arm — a runtime that would not start — was the shape `init`'s
/// predicate call already had to be fixed for: `docker run` failing because the
/// image is gone, the daemon is refusing, or the disk is full exits non-zero
/// with empty stdout, which parses to no outcomes and reads as *nothing was
/// measured*. Unmeasured suppresses nothing, so the direction is safe; the
/// silence is not. Without a reason the user gets a session with every hook
/// shipped into a sandbox nobody could ask about, and nothing said.
///
/// Stderr is trimmed to three lines. A runtime failing to pull or mount can
/// produce a page of it, and a diagnostic that buries the line above it in its
/// own output is one people learn to scroll past.
fn measured_or_reason(
    ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Vec<doctor::Outcome>, String> {
    if !ok {
        let mut reason = String::from("could not ask the sandbox what it has");
        for line in stderr.lines().filter(|l| !l.trim().is_empty()).take(3) {
            reason.push_str("\n     ");
            reason.push_str(line);
        }
        return Err(reason);
    }
    Ok(doctor::parse(stdout))
}

fn measure(
    program: &str,
    paths: &Paths,
    tag: &str,
    wanted: &BTreeSet<String>,
    ctx: &out::Ctx,
) -> Result<BTreeMap<String, bool>> {
    let mut facts = facts::Facts::load(paths);
    let unseen = facts.unseen(tag, wanted);
    if !unseen.is_empty() {
        let borrowed: Vec<&str> = unseen.iter().map(String::as_str).collect();
        let ran = Command::new(program)
            .args(image::probe_args(tag, &doctor::probe_programs(&borrowed)))
            .output();
        let outcomes = match ran {
            Ok(out) => measured_or_reason(
                out.status.success(),
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(e) => Err(format!("could not ask the sandbox what it has ({e})")),
        };
        let outcomes = outcomes.unwrap_or_else(|reason| {
            ctx.warn(&reason);
            Vec::new()
        });
        if !outcomes.is_empty() {
            facts.learn(tag, &outcomes);
            // Reported and swallowed, never fatal. This is a cache beside the
            // catalogue; a read-only home, a full disk or a `facts.json`
            // somebody replaced with a directory would otherwise abort every
            // `omh run`, `omh code` and `omh doctor` on the machine — a launch
            // killed by a file whose entire design premise is that losing it
            // degrades to "nobody has looked". `Facts::load` already treats the
            // read side this way and says why.
            if let Err(e) = facts.save(paths) {
                ctx.warn(&format!(
                    "measurements not cached ({e:#}) — the sandbox is asked again next time"
                ));
            }
        }
    }
    Ok(facts.about(tag))
}

/// What this repo's sandbox is: the recipe its stacks provision, the image that
/// recipe produces, and what that image has been measured to contain.
///
/// The four fields are **one answer**, and holding them together is what makes
/// a mismatch hard to write: `tag` is derived from `installs`, `resolves` is
/// keyed on `tag`, and `owed` is what `installs` promised. Nothing outside
/// [`sandbox`] constructs one.
struct Sandbox {
    /// Owned, because the definitions they are read from do not outlive this.
    installs: Vec<String>,
    tag: String,
    resolves: BTreeMap<String, bool>,
    /// What the provides this repo installed said must resolve once they had.
    /// Carried here rather than re-derived, so the caller that tops the
    /// measurements up asks about the same list `init` reported on.
    owed: BTreeSet<String>,
}

impl Sandbox {
    fn recipe(&self) -> Vec<&str> {
        self.installs.iter().map(String::as_str).collect()
    }

    /// Ask this image about anything nobody has asked it yet, and keep the
    /// answers.
    ///
    /// Launch does this too, not only `init`. A hook added after the last
    /// `init` names a program no measurement covers, and an unmeasured program
    /// suppresses nothing — so without this the hook ships into a sandbox that
    /// may not have it and fails at turn one with `not found`, which is the
    /// failure this whole design starts from. The cache is what makes it
    /// affordable: a repo whose hooks and stacks have not changed asks nothing
    /// and starts no container.
    ///
    /// **Builds the image first**, and that ordering is the method's reason for
    /// existing rather than a detail inside it.
    ///
    /// `init` had it right and all three launch paths had it backwards: they
    /// measured, then built inside `session_up`. So the first launch after a
    /// recipe changed — a `[provision]` opt-out, a fresh clone of a repo whose
    /// resolution is committed, anything after `docker image prune` — probed a
    /// tag with no image behind it, learned nothing, and shipped every hook
    /// unsuppressed into a sandbox that did not have their programs. It healed
    /// on the *second* launch, which is precisely the broken-first-turn this
    /// design exists to remove.
    ///
    /// Fixing it in three call sites would have left the fourth caller to get
    /// it right. Here it cannot be got wrong: asking an image a question and
    /// making sure there is an image to ask are one operation.
    ///
    /// Failures inside `measure` are reported and swallowed — a runtime that
    /// will not start leaves the facts as they were, which reads as *nobody has
    /// looked* and suppresses nothing. The build is **not** swallowed: an image
    /// that will not build is the session, not a diagnostic about it.
    //
    // Eight arguments, one over clippy's default. Every one is a distinct
    // input this cannot derive — the paths, the runtime, the adapter, the hook
    // directories, both halves of the resolved settings, and where to report.
    // Bundling them into a struct only to unpack it here would move the list
    // rather than shorten it.
    #[allow(clippy::too_many_arguments)]
    fn top_up(
        &mut self,
        paths: &Paths,
        program: &str,
        adapter: &Adapter,
        hook_dirs: &[PathBuf],
        own: &base::Own,
        repo: &settings::RepoPolicy,
        ctx: &out::Ctx,
    ) -> Result<()> {
        let recipe: Vec<String> = self.installs.clone();
        image::ensure_stack(
            program,
            adapter,
            &recipe.iter().map(String::as_str).collect::<Vec<_>>(),
            &paths.repo,
        )?;
        let wanted = probe_targets(hook_dirs, own, repo, &self.owed)?;
        self.resolves = measure(program, paths, &self.tag, &wanted, ctx)?;
        Ok(())
    }
}

/// Work out which image this repo runs, and what is already known about it.
///
/// **One function, because these are one answer.** For the whole of the first
/// milestone `init` built a stack layer and `container::plan` hardcoded
/// `image::tag_for(adapter)`, so the layer was built by one command and run by
/// none — and nothing was wrong with either half on its own. Two places
/// deciding which image a session runs is the shape of that bug, so there is
/// one place, and the measurements come back keyed on the tag it returned.
///
/// Fatal when the stacks will not load, which is the opposite of `say_hooks`'
/// answer to the same directory and is right for the opposite reason. There, an
/// unreadable directory costs a report. Here it decides *which sandbox you get*:
/// falling back to the harness image would launch a session with no toolchain
/// in it, silently, which is the failure this whole design starts from.
///
/// Reads the cache but never the container. Asking the image anything is
/// [`Sandbox::top_up`], which builds it first — so this stays cheap enough to
/// call on every launch path before anything has been decided.
fn sandbox(paths: &Paths, adapter: &Adapter, repo: &settings::RepoPolicy) -> Result<Sandbox> {
    let defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected = stack::detected(&defs, &paths.repo);
    let installs: Vec<String> = installs_for(&detected, &repo.provision)
        .into_iter()
        .map(str::to_string)
        .collect();
    let tag = image::stack_tag(
        adapter,
        &installs.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let resolves = facts::Facts::load(paths).about(&tag);
    let owed = needs_of(&detected, &repo.provision);
    Ok(Sandbox {
        installs,
        tag,
        resolves,
        owed,
    })
}

/// Run the harness's own login inside a sandbox, with this account's credential
/// files bind-mounted writable. There is no separate capture step: the login
/// writes straight through to the host.
fn auth_cmd(cwd: &std::path::Path, harness: &str, account: &str, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let adapter = Adapter::find(&paths.adapters(), harness)?;

    if adapter.creds.is_empty() {
        anyhow::bail!(
            "adapter {harness} declares no credential paths, so there is nothing to capture"
        );
    }

    auth::validate_name(account)?;
    let account_dir = auth::dir(&paths, harness, account);
    let already = auth::is_captured(&paths, &adapter, account);
    auth::prepare(&adapter, &account_dir, "/home/agent")?;

    let backend = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p))?;
    image::ensure(backend.program(), &adapter)?;

    // A throwaway: logging in must not leave a branch behind.
    let session = Session::scratch(paths.scratch("auth"), "auth".into());
    session.ensure(&paths.repo, "")?;
    let (own, repo) = resolved(&paths)?;

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &[],
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: true,
            account_dir: Some(account_dir.clone()),
            memory_bin: memory::deliver::available(&paths),
            // Empty, like the base this scratch session was created with at
            // `session.ensure(&paths.repo, "")`: a login is not work on the
            // project, so there are no project rules to look up.
            base: None,
            omh: own,
            repo,
            // The harness image, not this repo's stack layer, for the reason
            // `base` is `None`: a login is not work on the project. Building a
            // toolchain to type a password would spend minutes on a container
            // that is thrown away, and the credential paths a login writes are
            // the same in both images.
            image: image::tag_for(&adapter),
            // So nothing has been measured about it here, and nothing is
            // suppressed. That is the safe direction — a login session running
            // one hook too many costs nothing, and this container exists for
            // the length of an OAuth redirect.
            resolves: BTreeMap::new(),
        },
    )?;
    plan.validate(&backend.caps())?;
    image::ensure_network(backend.program(), &plan.network)?;

    // Progress, not the report: the login itself is what the user is here for,
    // and this is the sentence that tells them which window is about to open
    // and where the token will land. Under `--json` the same facts arrive as
    // fields on the outcome below.
    ctx.progress(&format!(
        "logging {harness} in as `{account}`{} — credentials → {}{}",
        if already { " (re-authenticating)" } else { "" },
        account_dir.display(),
        match &adapter.login {
            Some(hint) => format!("\nnext → {hint}"),
            None => String::new(),
        }
    ));
    let status = Command::new(backend.program())
        .args(backend.args(&plan))
        .status()?;
    if let Err(e) = session.remove(&paths.repo, "", &paths.shadows()) {
        // A leftover `auth` worktree wins `session::current()` and silently
        // becomes the session the next launch runs in.
        ctx.warn(&format!("could not remove the auth worktree: {e}"));
    }

    // Host paths, not guest ones: the guest path names a container that has
    // already been torn down and that the user cannot inspect.
    let unfilled: Vec<std::path::PathBuf> =
        auth::unfilled(&adapter, &account_dir, auth::GUEST_HOME)
            .iter()
            .map(|guest| {
                account_dir.join(
                    guest
                        .strip_prefix(auth::GUEST_HOME)
                        .unwrap_or(guest.as_path()),
                )
            })
            .collect();
    auth::login_outcome(status.success(), &unfilled)
        .map_err(|e| e.context(format!("run `omh auth {harness} {account}` again")))?;
    let all = auth::accounts(&paths, &adapter);
    // What the files can and cannot settle. For a harness naming `token` files
    // an empty `unfilled` *is* the login; for one that keeps credentials
    // somewhere omh cannot stat it means only that nothing is obviously
    // missing, and saying "captured" there announced a login to users who had
    // opened the harness, run nothing and quit.
    let decided = auth::decided_by_files(&adapter);
    let mut action = if decided {
        report::Action::new(
            "account-captured",
            format!("`{account}` captured for {harness}"),
        )
    } else {
        report::Action::new(
            "account-recorded",
            format!("`{account}` recorded for {harness} — login not confirmed"),
        )
        .note(format!(
            "{harness} keeps its credentials where omh cannot read them, so only \
             {harness} can say whether the login took"
        ))
        .next(format!("omh doctor {harness}"))
    };
    action = action.data(serde_json::json!({
        "harness": harness,
        "account": account,
        "reauthenticated": already,
        "credentials": account_dir.display().to_string(),
        "accounts": all,
    }));
    // Only once there is a choice to make. With one account the line is a
    // sentence about a decision nobody has.
    if all.len() > 1 {
        action = action
            .note(format!("accounts: {}", all.join(", ")))
            .next("omh repo set account <name>");
    }
    ctx.say(&action);
    Ok(())
}

fn ls(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let base = session::default_branch(&paths.repo);

    ctx.say(&report::Inventory {
        harnesses: Adapter::load_dir(&paths.adapters())?
            .iter()
            .map(|a| report::Harness {
                name: a.name.clone(),
                accounts: auth::accounts(&paths, a),
            })
            .collect(),
        adapters_dir: paths.adapters().display().to_string(),
        editors: editor::Editor::load_dir(&paths.editors())?
            .iter()
            .map(|e| report::Editor {
                name: e.name.clone(),
                installed: runtime::installed(&e.bin),
            })
            .collect(),
        sessions: session::list(&paths.worktrees())
            .into_iter()
            .map(|id| {
                let sess = Session::new(&paths.worktrees(), id.clone());
                report::Session {
                    label: sess.label().to_string(),
                    // `omh ls` is the wide view and does not ask git what state
                    // the work is in; `omh s` is the command for that, and
                    // asking here would cost a subprocess per session for a
                    // column this listing does not print. `None` says *not
                    // asked* — `Work::Clean` would be a claim, and a false one.
                    work: None,
                    // `None` for the same reason `work` is: this listing does
                    // not print the column and asking would cost a subprocess
                    // per session. `false` was a claim, and one omh had not
                    // checked.
                    running: None,
                    // Silently `.ok()` until #62 put a yellow question in
                    // this column: a surface that asks *how far behind?* and
                    // cannot say why is worse than one that never asked.
                    behind: match sess.behind(&paths.repo, &base) {
                        Ok(n) => Some(n),
                        Err(e) => {
                            ctx.warn(&format!(
                                "could not tell how far behind {base} {id} is: {e:#}"
                            ));
                            None
                        }
                    },
                    id,
                }
            })
            .collect(),
        base,
    });
    Ok(())
}

/// Bring a session up to date with its base, without letting a commit of
/// yours into the sandbox.
///
/// The order is the design, and every step of it is recoverable from the one
/// before:
///
/// 1. **Checkpoint** in the sandbox, so `omh sNN log` shows the point this can
///    be undone from and `git checkout` inside reaches it.
/// 2. **Take the session's tree** from the same throwaway index a review uses.
/// 3. **Merge on the host**, which writes a tree and touches nothing.
/// 4. **Materialise** it into the worktree.
/// 5. **Move the baseline**, replaying the branch's own commits first.
/// 6. **Record it in the sandbox** as a commit the agent can read.
///
/// Nothing is written anywhere until the merge has succeeded, so a failure in
/// steps 1-3 leaves the session exactly as it was.
fn sync(
    cwd: &std::path::Path,
    id: Option<&str>,
    base: Option<&str>,
    down: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| session::default_branch(&paths.repo));

    stop_before_syncing(&paths, &session, down, ctx)?;
    ctx.say(&sync_session(&paths, &session, &base)?);
    Ok(())
}

/// The sync itself, once it is known to be safe to run.
///
/// Split from `sync` so it can be asserted: the command around it needs a
/// container runtime to decide whether the sandbox is up, and nothing that
/// needs one is reachable from a test here. What is left outside is the
/// refusal and the printing.
fn sync_session(paths: &Paths, session: &Session, base: &str) -> Result<report::Synced> {
    let branch = session
        .branch
        .as_deref()
        .context("a scratch session has no base to move")?;
    let was = session::head_of(&paths.repo, branch)?;
    let onto = session::head_of(&paths.repo, base)?;
    anyhow::ensure!(
        was != onto,
        "{} is already on {base}. Nothing to bring over",
        session.id
    );

    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    let checkpoint = shadow.checkpoint(&session.worktree, "Before omh brought the base forward")?;

    // Named, so the conflict markers the agent opens say which side is which.
    // Measured: `merge-tree` labels each side with the string it was handed,
    // so a bare tree renders as forty hex characters in the middle of somebody's
    // source file.
    let ours = format!("refs/{}", session.id);
    let tree = session.tree(base)?;
    session::name_tree(&paths.repo, &ours, &tree)?;
    let merged = session::merge_three(&paths.repo, &was, base, &session.id);
    let _ = session::unname_tree(&paths.repo, &ours);
    let merged = merged?;

    session.materialise(&merged.tree)?;
    session.move_baseline(&paths.repo, &onto, &was)?;
    shadow.record_base_moved(&session.worktree, &onto, &merged.conflicted)?;
    let moved = session.commits_between(&paths.repo, &was, &onto)?;
    // Deliberately not `?`. The sync is done — the tree is merged, the baseline
    // has moved and the shadow has its commit — and a note that could not be
    // written is a worse outcome to report than to carry: the user would read a
    // failed command about work that landed.
    //
    // Carried out on the report rather than printed here, which is three things
    // at once. The failure becomes assertable, since nothing captures a write
    // to stderr from inside a function; it reaches `--json`, which a bare
    // `eprint` structurally never does; and this function goes back to being
    // the part with no side effects, which is the only reason it is split out
    // at all. The first draft printed it here and quietly broke that.
    let note = shadow
        .leave_note(&shadow::note_for(base, moved, merged.conflicted.len()))
        // `{e:#}` and not `{e}`: `Display` on an `anyhow::Error` prints the
        // outermost context only, so the reason — `Permission denied`, a full
        // disk — is exactly the part that would be dropped.
        .err()
        .map(|why| format!("{why:#}"));

    Ok(report::Synced {
        id: session.id.clone(),
        moved,
        base: base.to_string(),
        onto,
        conflicted: merged.conflicted,
        checkpoint: checkpoint.is_some(),
        note,
    })
}

/// A sync needs the sandbox stopped, and says why rather than assuming.
///
/// Not about the files — the checkpoint makes an overwrite recoverable. It is
/// about the agent's **context**: what it believes the tree contains lives in
/// its conversation, not on disk, and no checkpoint reaches that. It would then
/// edit a version that no longer exists, or write a whole file back from stale
/// content, and trunk's changes vanish inside a plausible-looking patch nobody
/// notices until review.
///
/// Stopping is the fix rather than the price: the harness restarts and reads
/// the tree as it now is. Whether an agent mid-turn may be interrupted is a
/// judgement only the user can make, which is why `--down` exists and why the
/// default is to refuse.
fn stop_before_syncing(paths: &Paths, session: &Session, down: bool, ctx: &out::Ctx) -> Result<()> {
    // Not a silent `return Ok(())`, which is what this was — and it sat one
    // line above the fix below, reaching the same outcome by a shorter path.
    // `runtime::installed` is itself `.unwrap_or(false)` over a `sh -c command
    // -v`, so a machine under fork pressure answers *no runtime installed*
    // when it means *could not ask*, and the sync then went ahead over a live
    // agent with nothing said at all.
    //
    // Refusing over a genuinely runtime-less machine is the cost, and it is
    // small: no runtime means no sandbox, so the session is not running, but a
    // sync is also not something that machine was going to do usefully. The
    // message says which it is and `--down` is not the way past — the way past
    // is a runtime.
    let backend = runtime::select(&runtime_preference(paths), &|p| runtime::installed(p))
        .with_context(|| {
            format!(
                "omh cannot tell whether {}'s sandbox is running, so it will not sync over it",
                session.id
            )
        })?;
    let name = paths.container(&session.id);
    // The reason this whole class was worth fixing. `.unwrap_or(false)` here
    // meant an unreachable runtime read as *nothing is running*, and a sync
    // then wrote over the files of a live agent — the outcome the paragraphs
    // above call the worst available, reached by the one path they do not
    // discuss.
    if !must_know(
        image::container_running(backend.as_ref(), &name),
        &session.id,
        "sync over it",
    )? {
        return Ok(());
    }
    anyhow::ensure!(
        down,
        "{id} is running, and a sync moves files underneath it. What the agent believes \
         the tree holds is in its conversation rather than on disk, so it would keep \
         editing a version that no longer exists:\n  \
         omh {id} down          stop it, then sync\n  \
         omh {id} sync --down   both, if the turn is safe to interrupt",
        id = session.id
    );
    ctx.progress(&format!("stopping {} first", session.id));
    image::container_remove(backend.program(), &name)?;
    Ok(())
}

/// The sandbox's own history, read from the host.
///
/// `log_cmd` rather than `log`, because `log` is what a reader of this file
/// expects to be a logging helper.
fn log_cmd(cwd: &std::path::Path, id: Option<&str>, turns: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    ctx.say(&log_report(&paths, &session, turns, ctx)?);
    Ok(())
}

/// The decision, separated from the printing so it can be asserted.
///
/// Split out because nothing reached the wiring otherwise: with the whole
/// command ending in `ctx.say`, replacing the read below with an empty list
/// left the entire suite green — `omh sNN log` would have reported *no
/// checkpoints* for every session on earth, with the shadow tests passing
/// (they call `checkpoints` directly) and the report tests passing (they build
/// the value by hand). `the_resolution_is_read_and_written_in_the_committed_
/// layer` in this file already argues the general case: a correct reader called
/// with the wrong argument is the same bug with a passing guard in front of it.
fn log_report(
    paths: &Paths,
    session: &Session,
    turns: bool,
    ctx: &out::Ctx,
) -> Result<report::Log> {
    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);

    // Three states, and only one of them is *nothing to show*.
    //
    // `Path::exists` was the first version of this and it answers `false` for
    // every failure, not only for absence — a root-owned `~/.omh`, an
    // unmounted volume, a stale handle — and each of those would have printed
    // *the agent has not committed anything* over an exit code of 0, about a
    // session whose repository omh could not open. `landed` in `shadow.rs`
    // states the rule this now follows and was written for this exact reason:
    // cannot tell must not spell the same as clean.
    //
    // The pairing with the gitdir is the part worth reading twice. `ensure`
    // writes the seed record and *then* renames the gitdir into place, so a
    // launch killed in between leaves a seed with no repository — the harmless
    // way round, rebuilt by the next launch. The reverse, a repository with no
    // seed, is what `reap` leaves when `remove_dir_all` fails on a live mount
    // and the seed file goes anyway. That one holds every checkpoint the agent
    // made and no way to say where they started, and it must not be reported
    // as an empty session: the user's next move would be `rm`.
    let read = match std::fs::metadata(&shadow.seed_record) {
        Ok(_) => shadow.checkpoints(&session.worktree)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !shadow.gitdir.exists() => {
            shadow::Checkpoints::default()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "{} has a sandbox repository at {} and no record of where it started. omh \
             cannot tell you what the agent committed, and `omh {} rm` would remove the \
             repository. Read it directly first:\n  git --git-dir={} log",
            session.id,
            shadow.gitdir.display(),
            session.id,
            shadow.gitdir.display()
        ),
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "reading where {} started, at {}",
                    session.id,
                    shadow.seed_record.display()
                )
            })
        }
    };

    let base = session::default_branch(&paths.repo);
    // Three answers, and the report renders each differently. A count omh could
    // not take must not print as zero — `0 behind main` is reassurance — and
    // the reason it could not be taken is worth saying out loud rather than
    // becoming an absence.
    let behind = match session.behind(&paths.repo, &base) {
        Ok(behind) => Some(behind),
        Err(e) => {
            ctx.warn(&format!(
                "could not tell how far behind {base} this is: {e}"
            ));
            None
        }
    };

    // Read only when asked. It is a second `git log` over a ref most sessions
    // do not have, and `omh sNN log` is a command people run often.
    let turns = match turns {
        false => None,
        true => Some(shadow.turn_log(&session.worktree)?),
    };

    Ok(report::Log {
        id: session.id.clone(),
        read,
        behind,
        base,
        turns,
    })
}

fn diff(
    cwd: &std::path::Path,
    id: Option<&str>,
    checkpoint: Option<usize>,
    base: Option<&str>,
    patch: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let report = diff_report(&paths, &session, checkpoint, base, patch, ctx)?;

    // Paging is for a person, and only when there is something to page. Under
    // `--json` the patch is a field in the answer — a pager between a script
    // and the object it asked for is a hang with no error — and an empty diff
    // goes to `say` so the reader gets the sentence `Diff::human` exists to
    // give them. Three states used to render as one blank screen: nothing
    // changed, the worktree had left its branch, and the pager was broken.
    let paged = patch && ctx.format == out::Format::Human && report.changed();
    if !paged {
        ctx.say(&report);
        return Ok(());
    }
    // What the palette resolved, not what git guesses. `NO_COLOR=1` and
    // `--color never` were both walked past by handing the terminal over —
    // measured: git honours neither, and prints colour into a file under
    // `--color always` because its own `auto` sees one.
    let colour = match ctx.palette.is_plain() {
        true => "never",
        false => "always",
    };
    match checkpoint {
        Some(number) => shadow::Shadow::new(&paths.shadows(), &session.id).stream_show(
            &paths.repo,
            &session.worktree,
            number,
            colour,
        ),
        None => session.stream_diff(&report.base, colour),
    }
}

/// The answer, separated from the printing so it can be asserted.
///
/// The same split `log_report` makes, for the reason it records: with the
/// wiring inline, hardcoding `What::Summary` at the call site left the whole
/// suite green while `omh sNN diff` dumped a full patch to stdout.
fn diff_report(
    paths: &Paths,
    session: &Session,
    checkpoint: Option<usize>,
    base: Option<&str>,
    patch: bool,
    ctx: &out::Ctx,
) -> Result<report::Diff> {
    let what = match patch {
        true => session::What::Patch,
        false => session::What::Summary,
    };
    let Some(number) = checkpoint else {
        let base = base
            .map(str::to_string)
            .unwrap_or_else(|| session::default_branch(&paths.repo));
        let body = session.diff(&base, what)?;
        return Ok(report::Diff {
            label: session.label().to_string(),
            session: session.id.clone(),
            checkpoint: None,
            base,
            what,
            body,
        });
    };

    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    // The same triage `log_report` does, so two commands one word apart answer
    // a never-launched session the same way. Without it, `omh sNN log` said
    // *no checkpoints* and exited 0 while `omh sNN diff 1` quoted the path of
    // a seed record at a user who has never heard of one.
    anyhow::ensure!(
        shadow.seed_record.exists() || shadow.gitdir.exists(),
        "there is no checkpoint {number} in this session. The agent has not committed \
         anything here yet — its sandbox has never run"
    );
    let _ = ctx;
    Ok(report::Diff {
        label: format!("{} checkpoint {number}", session.id),
        session: session.id.clone(),
        checkpoint: Some(number),
        // A checkpoint is measured against its own parent, not against a
        // branch — naming the base branch here would be naming something this
        // diff was never taken against.
        base: "its parent".to_string(),
        what,
        body: shadow.show(&session.worktree, number, what)?,
    })
}

/// Turn what the user typed into what the harvest takes.
///
/// Everything refusable *about the selection* is refused here, before
/// `harvest` makes a worktree, fetches, or touches the branch. That is not
/// everything `--keep` can refuse — `harvest` still turns away carried
/// secrets, a worktree that left its branch and a replay point it cannot
/// place, all after the fetch — but those need the sandbox read to answer.
/// A number, a merge and a missing terminal do not.
///
/// `terminal` is passed rather than probed, the rule `Cli::output` states in
/// this file: *resolved here and passed down rather than consulted where it is
/// used*. Probing inline made the decision untestable — no test process has a
/// terminal, so every test took the same arm and a mutation reopening the
/// editor for every real user left the suite green.
fn what_to_keep(
    shadow: &shadow::Shadow,
    session: &Session,
    selection: &str,
    edit: bool,
    terminal: bool,
    keeps_a_selection: &dyn Fn() -> Result<bool>,
) -> Result<shadow::Keep> {
    // Before the terminal check, so a line that names the same thing twice is
    // told so rather than being told about a terminal it also lacks. The
    // opposite order made this unreachable: every test, and every script, hit
    // the tty message first and this could be deleted without a test noticing.
    anyhow::ensure!(
        !edit || selection.is_empty(),
        "`--edit` opens the whole list for editing, so `--keep {selection} --edit` names \
         what to take twice. Use one: `--keep {selection}` takes those, `--keep --edit` \
         opens all of them"
    );
    if edit {
        // The measured hole this closes: with stdin not a terminal, `rebase -i`
        // runs the *unedited* todo, exits 0, and omh reports a curation that
        // never happened. `--edit` is the only path that needs a person, so it
        // is the only one that has to ask.
        anyhow::ensure!(
            terminal,
            "`--edit` opens the list in your editor and there is no terminal here. \
             git would run the list unedited and report success. Drop `--edit` to keep \
             everything, or name what you want: `omh {} commit --keep 1,3-4`",
            session.id
        );
        return Ok(shadow::Keep::Edit);
    }
    if selection.is_empty() {
        return Ok(shadow::Keep::All);
    }

    // The one thing a selection needs that `--keep` alone does not, asked
    // before anything is read. `cherry-pick --empty=` is newer than everything
    // else omh asks of git — #56 made it a dependency without being able to
    // name the release — and on a git without it the flagship command answers
    // with a usage dump the user has no way to read as *your git is too old*.
    // Asked here rather than at the call site, so `--keep` and `--keep --edit`
    // — which need nothing new of git — do not fork one to throw the answer
    // away. Both return above this line.
    //
    // Three answers. *Could not ask* is git's own failure and is reported as
    // itself: the first version collapsed it into "no", so a user with no git
    // on PATH was told their git was too old to name checkpoints, and the
    // fallback omh recommended failed differently a second later.
    match keeps_a_selection().with_context(|| {
        format!(
            "omh cannot tell whether this git can name checkpoints, so it will not guess. \
             `omh {} commit --keep` takes them all and asks nothing new of git",
            session.id
        )
    })? {
        true => {}
        false => anyhow::bail!(
            "naming checkpoints needs a newer git than this one: `git cherry-pick` here has \
             no `--empty`, which omh uses to drop a commit whose changes are already on the \
             branch. `omh {} commit --keep` takes them all and works on any git omh supports",
            session.id
        ),
    }

    // Resolved against the session's own list, so a number means the commit it
    // meant on screen.
    let read = shadow.checkpoints(&session.worktree)?;
    let mut ids = Vec::new();
    for number in shadow::chosen(selection, read.commits.len())? {
        // `chosen` bounds the number by the length of this list and
        // `checkpoints` numbers it contiguously from 1, so this cannot miss.
        let checkpoint = &read.commits[number - 1];
        debug_assert_eq!(checkpoint.number, number);
        // Already on the branch. Replaying it would apply it twice, and the
        // divider in `omh sNN log` is where the user read that it was theirs.
        anyhow::ensure!(
            !checkpoint.landed,
            "checkpoint {number} is already on {}. `omh {} log` draws the line: everything \
             below it has been handed over, and handing it over again applies it twice",
            session.branch.as_deref().unwrap_or("the branch"),
            session.id
        );
        // A merge, which a selection replays one commit at a time and git will
        // not do without being told which side to take. Refused here rather
        // than by git after the fetch, where the message is *"is a merge but no
        // -m option was given"* — advice about a flag `--keep` forbids and that
        // means something else entirely in omh.
        anyhow::ensure!(
            checkpoint.touched.is_some(),
            "checkpoint {number} is a merge, and omh will not choose which side of one to \
             take. `omh {} commit --keep` replays the whole range instead — note that git \
             flattens a merge when it does",
            session.id
        );
        ids.push(checkpoint.id.clone());
    }
    Ok(shadow::Keep::These(ids))
}

/// What the sandbox answered, and what the host answered, in one list.
///
/// The order is the point. The emptiness check is how omh notices a sandbox
/// that never ran the probe at all, and host checks always produce something —
/// folded in first, they would answer *yes, something ran* on behalf of a
/// container that did nothing, and `doctor` would pass on a probe that never
/// executed.
///
/// The host's side is **not** a parameter, and that is the guard. As two
/// arguments the order was a convention: swapping them, or passing an empty
/// list, silenced the emptiness check or dropped git from the report entirely,
/// and no test could reach either — `doctor_cmd` needs a container. Taken from
/// `doctor::git_checks` here, none of those three mistakes compiles.
fn every_check(from_the_sandbox: Vec<doctor::Outcome>) -> Result<Vec<doctor::Outcome>> {
    anyhow::ensure!(
        !from_the_sandbox.is_empty(),
        "the probe produced no output — the sandbox did not run it"
    );
    Ok(from_the_sandbox
        .into_iter()
        .chain(doctor::git_checks())
        .collect())
}

/// Whether a commit may go ahead over what `git diff --check` found.
///
/// Both ways of committing refuse, because both would land the markers: `-m`
/// stages the files as they are, and `--keep` replants commits the agent made
/// on top of them. A conflict half-resolved compiles in neither case, and the
/// person who finds out is whoever reviews the branch.
///
/// The lines are a parameter rather than something read here, so every branch
/// of this is a table test — and so that the caller cannot ask git twice.
///
/// `--force` exists because a conflict marker at the start of a line is not
/// always a conflict: a test fixture holds them on purpose, and this very
/// repository has files that would trip it. Refusing something a user can
/// mean is a nuisance; refusing it with no way past is a bug.
fn may_commit(id: &str, unresolved: &[String], force: bool) -> Result<()> {
    if unresolved.is_empty() || force {
        return Ok(());
    }
    // Enough to act on without becoming the output itself. A whole-file
    // conflict is one marker per hunk and there can be hundreds; the count is
    // the scale and the first lines are where to start.
    let shown: Vec<&str> = unresolved.iter().take(5).map(String::as_str).collect();
    let rest = unresolved.len().saturating_sub(shown.len());
    anyhow::bail!(
        "{id} still has {n} conflict marker{s} in its files:\n  {lines}{more}\n\
         Resolve them first, or:\n  \
         omh {id} commit --keep --force   commit them anyway",
        n = unresolved.len(),
        s = if unresolved.len() == 1 { "" } else { "s" },
        lines = shown.join("\n  "),
        more = match rest {
            0 => String::new(),
            n => format!("\n  …and {n} more"),
        }
    );
}

/// Whether the reaper may consider stopping this session.
///
/// The one place the safe direction is inverted, and a named function rather
/// than a `matches!` in a filter so that it can be asserted at all — the
/// mutation that matters (`Unknown` becoming reapable) left the whole suite
/// green while this was inline.
///
/// Not `must_know`, because this runs on a timer with nobody to refuse to.
/// *Could not tell* keeps a session **out** of the list: leaving a sandbox up
/// costs a container, and stopping a live one on a guess costs somebody's
/// turn.
fn reapable(running: &image::Running) -> bool {
    matches!(running, image::Running::Yes)
}

/// *Could not tell* at a point where omh is about to act on the answer.
///
/// Not a `no`, and at a decision point not a `yes` either — so it is a
/// refusal, carrying the runtime's own words. Every caller of this is about to
/// create, enter, stop or overwrite a container on the strength of the answer,
/// and each of those is worse done blind than not done.
///
/// The alternative it replaces was `.unwrap_or(false)`, which is how a Docker
/// daemon that is down made every sandbox look stopped. An earlier draft of
/// this sentence said it "shipped for a year"; the repository is nineteen days
/// old. Nobody would have checked that, which is the reason to say the
/// checkable thing instead: it was there from the first commit, 2026-08-05.
fn must_know(running: image::Running, what: &str, doing: &str) -> Result<bool> {
    match running {
        image::Running::Yes => Ok(true),
        image::Running::No => Ok(false),
        image::Running::Unknown(why) => anyhow::bail!(
            "omh could not tell whether {what} is running, so it will not {doing}: {why}"
        ),
    }
}

/// Whether this session may be removed, or what stands in the way.
///
/// Separate from `rm` so the decision is assertable: `rm` takes a container
/// down, and nothing that needs one can be reached by a test here. What is
/// left in `rm` is the single call — its absence is a line missing from a
/// diff rather than a behaviour hiding behind a runtime.
/// What omh knows about a session's turn snapshots when it is asked to remove
/// it.
///
/// Three answers rather than a count, for the reason `AtStake` next door has
/// three: *could not tell* is not *none*, and this is the last thing in front
/// of an irreversible delete. The first version of this was a `usize` reached
/// through `.unwrap_or(0)`, so an unreadable sandbox removed itself in
/// silence — no count, no warning, nothing to act on.
#[derive(Debug)]
enum Snapshots {
    /// No `refs/omh/turn` here — this session has never finished a turn with
    /// anything changed.
    None,
    Kept(usize),
    /// omh asked and could not tell, and why.
    Unreadable(String),
}

fn may_remove(
    paths: &Paths,
    session: &Session,
    snapshots: Snapshots,
    force: bool,
) -> Result<Option<String>> {
    let branch = format!("omh/{}", session.id);
    // Named, never the reason. A snapshot is a tree omh photographed at the
    // end of a turn, not work the agent chose to keep — and there is one for
    // nearly every session that ever ran, so refusing over them would make
    // this guard fire almost always. A guard that fires almost always is one
    // people learn to answer with `--force` without reading, which is exactly
    // how it would stop protecting the commits it was built for.
    //
    // They are still worth a sentence, because they go too, and because the
    // one time they matter is the one time the agent threw the tree away.
    // Part of the sentence, not a line of its own: every line below the first
    // is a command the user can paste, and there is a test that says so.
    let also = match &snapshots {
        Snapshots::None => String::new(),
        Snapshots::Kept(n) => format!(", and {n} turn snapshot{} omh took", plural(*n)),
        // Said as its own clause rather than folded into a number, because a
        // count omh could not take is the one answer a user might want to go
        // and look into before deleting.
        Snapshots::Unreadable(why) => {
            format!(", and omh could not tell how many turn snapshots go with it ({why})")
        }
    };
    let snapshots = match snapshots {
        Snapshots::Kept(n) => n,
        // An unreadable count is *something* at stake, so the line that reads
        // them is still offered — it is the command that would say what.
        Snapshots::Unreadable(_) => usize::MAX,
        Snapshots::None => 0,
    };
    // …and the way to read them is a command, so it goes where commands go.
    // Padded to the column the other lines use — the test that reads these
    // only checks each line is pasteable, so a misaligned one stays green.
    let reading = match snapshots {
        0 => String::new(),
        _ => format!(
            "\n  omh {} log --turns         read the snapshots",
            session.id
        ),
    };
    let (what, whether) = match at_stake(paths, session) {
        // Nothing the branch lacks. The snapshots still go, so they are still
        // said — returned rather than printed, because this function's whole
        // value is being a decision a table test can reach.
        // Nothing of the agent's own at stake. The snapshots still go, so they
        // are still said — returned rather than printed, because this
        // function's whole value is being a decision a table test can reach.
        AtStake::Nothing => {
            return Ok(non_empty(match snapshots {
                0 => String::new(),
                usize::MAX => format!(
                    "omh could not tell how many turn snapshots go with {id}{also_why}. \
                     `omh {id} log --turns` would say",
                    id = session.id,
                    also_why = also
                        .split_once('(')
                        .map(|(_, w)| format!(" — {}", w.trim_end_matches(')')))
                        .unwrap_or_default()
                ),
                n => format!(
                    "{n} turn snapshot{} omh took go with {id}. `omh {id} log --turns` reads them",
                    plural(n),
                    id = session.id
                ),
            }))
        }
        AtStake::Work(what) => (what, "that no branch has"),
        // Could not tell, which is not the same as nothing to lose. A user is
        // asked once and `--force` is right there; the alternative is deleting
        // the only copy of work omh could not count, in silence.
        AtStake::Unknown(why) => (why, "and omh cannot say what that removes"),
    };
    anyhow::ensure!(
        force,
        "{id} has {what} {whether}{also}. Removing it deletes the only copy:\n  \
         omh {id} log                 read what is there\n  \
         omh {id} commit --keep       put it on {branch}\n  \
         omh {id} commit -m \"…\"       or take the files as they stand{reading}\n  \
         omh {id} rm --force          remove it anyway",
        id = session.id
    );
    Ok(None)
}

/// `None` for the empty string, so "nothing to add" is a state rather than a
/// blank line somebody has to remember not to print.
fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// What a removal would destroy that exists nowhere else.
///
/// Three answers, because two would make *omh could not tell* spell the same
/// as *there is nothing here* — in the one command where that spelling is
/// unrecoverable. `Shadow::landed` states the rule one layer down and the
/// first version of this discarded it: `.exists()` and `.ok()` turned a
/// permissions error, a truncated replay record and a repository with no seed
/// into "go ahead and delete".
#[derive(Debug)]
enum AtStake {
    /// No sandbox was ever built here.
    Nothing,
    /// This much, counted.
    Work(String),
    /// A repository is there and omh could not read it. Why, in git's words.
    Unknown(String),
}

/// What the seed record alone settles, if anything.
///
/// The same triage `log_cmd` does, and for the same reason — `Path::exists`
/// answers `false` for every failure, not only for absence, so an unreadable
/// `~/.omh` read as *nothing to lose*. The one difference is what the third
/// arm does: `log` cannot show you the repository and says so; `rm` would
/// delete it, so it asks first.
///
/// Over the metadata result rather than the path, so all four answers are a
/// table. The permission arm cannot be produced by a test that might run as
/// root — `chmod 000` does not stop uid 0 — which is exactly the arm that
/// silently deleted a sandbox before this.
fn from_the_seed_record(
    metadata: std::io::Result<()>,
    gitdir_exists: bool,
    shadow: &shadow::Shadow,
) -> Option<AtStake> {
    match metadata {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !gitdir_exists => {
            Some(AtStake::Nothing)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(AtStake::Unknown(format!(
            "a sandbox repository at {} and no record of where it started",
            shadow.gitdir.display()
        ))),
        Err(e) => Some(AtStake::Unknown(format!(
            "a sandbox omh could not read at {}: {e}",
            shadow.seed_record.display()
        ))),
    }
}

fn at_stake(paths: &Paths, session: &Session) -> AtStake {
    let shadow = shadow::Shadow::new(&paths.shadows(), &session.id);
    if let Some(answer) = from_the_seed_record(
        std::fs::metadata(&shadow.seed_record).map(|_| ()),
        shadow.gitdir.exists(),
        &shadow,
    ) {
        return answer;
    }

    match shadow.unkept(&session.worktree) {
        Err(e) => AtStake::Unknown(format!("a sandbox omh could not read: {e}")),
        Ok((0, 0)) => AtStake::Nothing,
        Ok((0, files)) => AtStake::Work(format!("{files} uncommitted path{}", plural(files))),
        Ok((commits, 0)) => AtStake::Work(format!("{commits} commit{}", plural(commits))),
        Ok((commits, files)) => AtStake::Work(format!(
            "{commits} commit{} and {files} uncommitted path{}",
            plural(commits),
            plural(files)
        )),
    }
}

/// The `s` on a count, in the one place that decides what that looks like.
fn plural(n: usize) -> &'static str {
    match n {
        1 => "",
        _ => "s",
    }
}

/// The session a command acts on when it acts on work already done.
///
/// Deliberately not `session::pick`: that invents the *next* id when none
/// exists, which is right for a launch — it is about to create that worktree —
/// and wrong for every command that operates on a session that must already be
/// there. Committing into a fabricated id would fail somewhere further down,
/// about a path nobody named.
fn existing_session(paths: &Paths, explicit: Option<&str>) -> Result<Session> {
    let id = match explicit {
        Some(id) => {
            session::validate_id(id)?;
            id.to_string()
        }
        None => session::current(&paths.worktrees())
            .context("no sessions yet — start one with `omh claude`")?,
    };
    let session = Session::new(&paths.worktrees(), id);
    anyhow::ensure!(
        session.worktree.exists(),
        "no session {} — `omh s` lists them",
        session.id
    );
    Ok(session)
}

/// The two ways to land a session's work, as one value.
///
/// They are mutually exclusive and clap already enforces that — this is what
/// stops the *rest* of the code from having to know. As four loose parameters
/// (`message`, `keep`, `edit`, and their combinations) a caller could pass a
/// message and a selection together, and what happened then was decided by
/// which `if` came first. The commit body says out loud why doing both is
/// wrong: the squash lands first and git's patch-id then drops every replanted
/// commit as already applied, so the granular history disappears with nothing
/// said. A pair that cannot be constructed cannot do that.
enum Landing<'a> {
    /// One commit of the files as they stand. `None` opens the editor.
    Squash(Option<&'a str>),
    /// The agent's own commits, replanted.
    Keep {
        /// Empty means everything since the last handover.
        selection: &'a str,
        /// The todo, in the user's editor.
        edit: bool,
    },
}

fn commit(
    cwd: &std::path::Path,
    id: Option<&str>,
    landing: Landing,
    skip_carried: bool,
    force: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let base = session::default_branch(&paths.repo);
    may_commit(&session.id, &session.unresolved(&base)?, force)?;

    // Two ways to land the same work, and the user picks which at the moment
    // they land it. `-m` squashes the files into one commit of their own and
    // never reads the sandbox's repository at all — it is the same `git add -A`
    // it always was. `--keep` replants the agent's commits, messages included.
    //
    // One writer either way. Doing both puts the squashed content on the branch
    // first, and git's patch-id then drops every replanted commit as already
    // applied — the granular history disappears with nothing said, which is the
    // whole thing `--keep` exists to deliver.
    // The `--keep` arm returns rather than yielding a message: it is a whole
    // command, and the squash below is the other one. Written as a match so
    // adding a third way to land is a compile error here rather than a
    // fall-through into the squash.
    let message = match landing {
        Landing::Squash(message) => message,
        Landing::Keep { selection, edit } => {
            let branch = session
                .branch
                .as_deref()
                .context("a scratch session has no branch to commit to")?;
            let shadow = crate::shadow::Shadow::new(&paths.shadows(), &session.id);
            let carried = config::policy_list(&paths, "carry_in");
            let keep = what_to_keep(
                &shadow,
                &session,
                selection,
                edit,
                std::io::IsTerminal::is_terminal(&std::io::stdin()),
                &|| shadow::git_supports("cherry-pick", "--empty"),
            )?;
            let named = match &keep {
                shadow::Keep::These(ids) => Some(ids.len()),
                _ => None,
            };
            let landed = shadow.harvest(&paths.repo, &session.worktree, branch, &carried, keep)?;
            // Named four, landed three. git drops a commit whose patch is already
            // on the branch — measured, it says `patch contents already upstream`
            // on stderr and exits 0, and the helper that runs it keeps only stdout
            // on success. Without this the user reads `kept 3` after asking for
            // four and is left to wonder which one, or whether they miscounted.
            if let Some(named) = named.filter(|named| *named > landed) {
                ctx.warn(&format!(
                    "you named {named} checkpoint{}, and {landed} reached {branch} — git drops a \
                 commit whose changes are already there. `omh {} log` shows what is left",
                    if named == 1 { "" } else { "s" },
                    session.id
                ));
            }
            let n = session.commits(&paths.repo, &base);
            warn_uncounted(&n, ctx, &base);
            ctx.say(
                &report::Action::new(
                    "committed",
                    match landed {
                        // Two ways to keep nothing, and they are different news. A
                        // session that has never handed anything over has made no
                        // commits; one that has is simply up to date, and telling
                        // that user their agent "has made no commits" contradicts
                        // the branch they are looking at.
                        // Swallowed rather than propagated, because this only
                        // chooses between two ways of saying nothing happened and
                        // the harvest above already succeeded — failing here would
                        // report a command that worked as a command that did not.
                        //
                        // `true` on error, and the direction matters. `landed`
                        // fails only for a record that *exists* and could not be
                        // read, so the session has handed something over before:
                        // "nothing new" is then the true sentence and "has made no
                        // commits" is a stronger claim than omh can make, about a
                        // branch the user can see. An earlier version defaulted the
                        // other way and called it the vaguer answer. It is not.
                        0 if shadow.landed().map(|l| l.is_some()).unwrap_or(true) => format!(
                            "nothing new to keep — everything {} has committed is already on \
                         the branch",
                            session.label()
                        ),
                        0 => format!("nothing to keep — {} has made no commits", session.label()),
                        _ => format!(
                            "kept {landed} of {}'s own commits{}",
                            session.label(),
                            branch_tally(&n)
                        ),
                    },
                )
                .data(serde_json::json!({
                    "session": session.id,
                    "branch": session.label(),
                    "kept": landed,
                    "commits": n.as_ref().ok(),
                    "base": base,
                })),
            );
            return Ok(());
        }
    };

    // The same list the launcher copies from, so what `commit` refuses to
    // publish and what omh put there cannot disagree.
    let carried = config::policy_list(&paths, "carry_in");
    let policy = if skip_carried {
        session::Carried::skipping(&carried)
    } else {
        session::Carried::refusing(&carried)
    };
    session.commit(message, policy)?;

    // Counted against the base rather than reported as "committed", because the
    // number is what tells you whether the branch is worth pushing — and it is
    // the same number `omh s rm` will use to decide the branch survives.
    let base = session::default_branch(&paths.repo);
    let n = session.commits(&paths.repo, &base);
    warn_uncounted(&n, ctx, &base);
    ctx.say(
        &report::Action::new(
            "committed",
            format!("committed to {}{}", session.label(), branch_tally(&n)),
        )
        .data(serde_json::json!({
            "session": session.id,
            "branch": session.label(),
            "commits": n.as_ref().ok(),
            "base": base,
        })),
    );
    Ok(())
}

/// Say that the count could not be taken, where the answer merely omits it.
///
/// `branch_tally` going quiet is right for the answer — what omh did is true
/// whether or not it can count afterwards — but quiet is not the same as
/// unsaid. This is the same failure `omh s rm` will meet later, and meeting it
/// there for the first time, over a branch, is worse than hearing about it now
/// over a commit that already succeeded. On stderr, like every other warning,
/// so it stays out of anything being redirected.
fn warn_uncounted(n: &Result<usize>, ctx: &out::Ctx, base: &str) {
    if let Err(e) = n {
        ctx.warn(&format!(
            "could not count this branch against {base} — {e:#}"
        ));
    }
}

/// What a session's branch holds, appended to an answer that is true without it.
///
/// Empty when git could not take the count — `commits` returns a `Result`
/// precisely because a base that does not resolve is a question with no answer,
/// and *"(0 commits on the branch)"* is the wrong one. The sentence in front of
/// this reports what omh just did, which is true either way.
fn branch_tally(n: &Result<usize>) -> String {
    match n {
        Ok(n) => format!(
            " ({n} {} on the branch)",
            if *n == 1 { "commit" } else { "commits" }
        ),
        Err(_) => String::new(),
    }
}

fn push(
    cwd: &std::path::Path,
    id: Option<&str>,
    name: Option<&str>,
    pr: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let session = existing_session(&paths, id)?;
    let target = session.push(name)?;
    ctx.say(
        &report::Action::new("pushed", format!("{} → origin/{target}", session.label())).data(
            serde_json::json!({
                "session": session.id,
                "branch": session.label(),
                "target": target,
            }),
        ),
    );

    if !pr {
        return Ok(());
    }

    // Optional accelerant, never a dependency: a repo on a non-GitHub remote is
    // a normal repo, and a box without `gh` still has to be able to push. Saying
    // what to run beats half-succeeding and leaving the user to guess whether
    // the PR exists.
    anyhow::ensure!(
        runtime::installed("gh"),
        "gh is not installed; open it with\n  gh pr create --head {target}"
    );
    let status = Command::new("gh")
        .current_dir(&session.worktree)
        .args(["pr", "create", "--head", &target])
        .status()
        .context("running gh pr create")?;
    anyhow::ensure!(status.success(), "gh pr create did not open a pull request");
    Ok(())
}

fn rm(cwd: &std::path::Path, id: &str, force: bool, ctx: &out::Ctx) -> Result<()> {
    session::validate_id(id)?;
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());

    // Before anything is taken down, because everything below this line is
    // irreversible and the first of them is the container.
    //
    // The last piece of [risks](docs/design/risks.md) 2c. The branch survives a
    // removal and the worktree's files were on disk until this ran — but the
    // agent's own commits live only in the sandbox's repository, and `reap`
    // deletes it. After a `reset --hard` in the sandbox those were the only
    // copies there ever were. omh could not ask this question until `log`
    // learned to count them.
    // Not `.unwrap_or(0)`, and not `?` either. A count omh could not take is
    // not a count of none — but it is also not a reason `--force` cannot get
    // past, which is what `?` made it: an orphaned directory that is not a
    // repository at all could no longer be cleaned up by the one command that
    // exists to clean it up.
    //
    // So it joins `AtStake::Unknown`, which has had exactly this shape and
    // exactly this escape since #58.
    let snapshots =
        match shadow::Shadow::new(&paths.shadows(), &session.id).turns(&session.worktree) {
            Ok(None) => Snapshots::None,
            Ok(Some(n)) => Snapshots::Kept(n),
            Err(e) => Snapshots::Unreadable(format!("{e:#}")),
        };
    if let Some(note) = may_remove(&paths, &session, snapshots, force)? {
        ctx.warn(note.trim());
    }

    // Drop the graph with the code it describes, while the container is still
    // around to do it. Otherwise the index outlives the worktree forever.
    //
    // Then take the container itself down. A session is the container *and* the
    // worktree, and removing only the worktree leaves a half that can never be
    // reached again: the bind mount still points at the deleted directory, the
    // next launch recreates it at a new inode the mount does not follow, and
    // `session_up` — seeing a container that is up — execs into it and gets
    // "current working directory is outside of container mount namespace root"
    // for every command from then on. Nothing else ever removes it.
    if let Ok(backend) = runtime::select(&runtime_preference(&paths), &|p| runtime::installed(p)) {
        let name = paths.container(id);
        // Best-effort already — `let _` below says so — and *could not tell*
        // joins *not running* because the exec would fail against a runtime
        // that will not answer anyway. Failing a removal over a tidy-up nobody
        // asked for would be the tail wagging the dog.
        //
        // An earlier version of this comment claimed the graph entry is
        // dropped on the next launch that reuses the id. It is not:
        // `drop_graph_command` has exactly one caller, this one. What is
        // skipped here is skipped for good, so it is said rather than
        // swallowed.
        let up = image::container_running(backend.as_ref(), &name);
        if let image::Running::Unknown(why) = &up {
            ctx.warn(&format!(
                "could not tell whether {id}'s sandbox was up, so its graph entry \
                 was left behind: {why}"
            ));
        }
        if matches!(up, image::Running::Yes) {
            let project = base::project_name(&paths.repo_name(), id);
            let _ = Command::new(backend.program())
                .args(backend.exec_args(&name, &base::drop_graph_command(&project), false))
                .output();
        }
        // Best-effort: a container that was never started has nothing to
        // remove, and that must not stop the worktree from going.
        // Warned rather than swallowed, and the line above already warns
        // about the weaker failure — not being able to *tell* whether it was
        // up. A removal that fails leaves a live container bound to a worktree
        // this function is about to delete, which manufactures exactly the
        // unenterable state `Probe::NotEnterable` exists for. This function's
        // own doc names `omh s rm` as the historical cause of it.
        if let Err(e) = image::container_remove(backend.program(), &name) {
            ctx.warn(&format!(
                "{id}'s container would not stop, and its worktree is going: it is left \
                 running against a directory that will not be there. `docker rm -f {name}` \
                 clears it — {e:#}"
            ));
        }
    }

    // The third thing a session owns. Staging is re-rendered on every launch so
    // leaving it costs nothing that breaks — but the `last-used` marker beside
    // it is what says a session ran here, and a marker with no session behind it
    // is how `omh s` learns to report a leftover that is not there any more.
    let _ = std::fs::remove_dir_all(paths.runs().join(id));

    // The branch is reported honestly rather than always claimed as kept: one
    // that never received a commit preserves nothing, and saying otherwise
    // trains people to ignore a namespace filling with dead refs.
    let base = session::default_branch(&paths.repo);
    let action = match session.remove(&paths.repo, &base, &paths.shadows())? {
        session::Removed::BranchKept(n) => {
            // Two ways to be kept, and they are not the same news. A branch
            // kept because it holds three commits is an invitation to review
            // them; one kept because omh could not tell what it holds is a
            // question, and saying "3 commits" for it would be an invention.
            //
            // The count comes back from `remove` rather than being asked
            // again: it is the number that *made* the decision, and a second
            // call could answer differently and narrate a decision nobody took.
            //
            // The review command changes with it. What stops omh counting is a
            // range end that does not resolve, and for a branch this session is
            // standing on that is the base — so a line beginning `<base>..`
            // would fail in the user's hands for the reason they are being
            // shown it.
            let (kept, review) = match n {
                Some(n) => (
                    format!(
                        "kept ({n} {} to review)",
                        if n == 1 { "commit" } else { "commits" }
                    ),
                    format!("git log {base}..omh/{id}"),
                ),
                None => (
                    format!("kept — omh could not count it against {base}"),
                    format!("git log omh/{id}"),
                ),
            };
            report::Action::new(
                "session-removed",
                format!("removed session {id}; branch omh/{id} {kept}"),
            )
            .next(review)
            .next(format!("git branch -D omh/{id}"))
            .data(serde_json::json!({
                "session": id,
                "branch": format!("omh/{id}"),
                "branch_kept": true,
                "commits": n,
            }))
        }
        session::Removed::BranchDropped => report::Action::new(
            "session-removed",
            format!("removed session {id}; branch omh/{id} dropped (no commits)"),
        )
        .data(serde_json::json!({
            "session": id,
            "branch": format!("omh/{id}"),
            "branch_kept": false,
            "commits": 0,
        })),
        session::Removed::NoBranch => {
            report::Action::new("session-removed", format!("removed session {id}"))
                .data(serde_json::json!({ "session": id, "branch_kept": false }))
        }
    };
    ctx.say(&action);

    // The review moment rides on something already happening rather than a
    // ritual nobody performs. Best-effort on purpose: a store omh cannot read
    // is a reason to say nothing, never a reason to leave a session that
    // cannot be removed.
    //
    // A nudge is advice, so it goes through `hint`: on stderr, and absent
    // under `--json`, where a sentence about reviewing notes is noise in a
    // stream something else is parsing.
    if let Ok(notes) = memory::load(&paths) {
        if let Some(line) = memory::session_nudge(&notes, id) {
            ctx.hint(&line);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The tally omits itself rather than saying zero.
    ///
    /// `omh s rm` deleted a branch because a count it could not take read as
    /// `0`; this is the same mistake one layer out, where it would be printed
    /// rather than acted on. A pure function over the `Result`, so the guard
    /// needs no repository and cannot be defeated by a fixture.
    #[test]
    fn a_tally_omh_could_not_take_is_absent_rather_than_zero() {
        assert_eq!(branch_tally(&Ok(1)), " (1 commit on the branch)");
        assert_eq!(branch_tally(&Ok(3)), " (3 commits on the branch)");
        assert_eq!(
            branch_tally(&Ok(0)),
            " (0 commits on the branch)",
            "a real zero is still an answer and still gets said"
        );
        assert_eq!(
            branch_tally(&Err(anyhow::anyhow!("bad revision"))),
            "",
            "and a count nobody took says nothing at all"
        );
    }

    const BUNDLED_ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BUNDLED_EDITORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/editors");

    /// A full command line, program name included, the way `main` sees it.
    fn cli_argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("omh")
            .chain(parts.iter().copied())
            .map(str::to_string)
            .collect()
    }

    /// The session goes first, and everything after it is what you would have
    /// typed anyway.
    ///
    /// Four spellings meant the same thing before this — `s diff`, `s diff s01`,
    /// `s -s s01 diff`, `-s s01 s diff` — over two mechanisms applied unevenly:
    /// `rm` required the positional, `commit` and `push` ignored it, and `push`
    /// could not have one because that slot is the branch name. This is the one
    /// form they collapse into.
    #[test]
    fn a_session_named_first_is_the_session_the_command_acts_on() {
        // a session verb: the prefix desugars to the sessions namespace
        assert_eq!(
            session_prefix(cli_argv(&["s01", "diff"])),
            (Some("s01".to_string()), cli_argv(&["s", "diff"]))
        );
        // The verbs come from the parser, not from a list here, so `log` joins
        // this the moment `SessionsCmd` has it — which is the next step in the
        // spec and exactly why the list is derived.
        assert!(
            !session_prefix(cli_argv(&["s01", "push", "fix/x"]))
                .1
                .contains(&"s01".to_string()),
            "the id is lifted out, never left in the arguments"
        );
        // …carrying its own flags untouched
        assert_eq!(
            session_prefix(cli_argv(&["s02", "commit", "--keep", "1,3"])),
            (
                Some("s02".to_string()),
                cli_argv(&["s", "commit", "--keep", "1,3"])
            )
        );
        // A session verb whose flags do not parse stays a session verb. This
        // is the case `--keep 1,3` used to cover, before #56 gave `--keep` a
        // value and both readings started parsing: without the verb check, the
        // fallback would offer to launch a harness called `commit`.
        assert_eq!(
            session_prefix(cli_argv(&["s02", "commit", "--whatever"])),
            (
                Some("s02".to_string()),
                cli_argv(&["s", "commit", "--whatever"])
            ),
            "a session verb omh cannot parse is not a harness"
        );
    }

    /// The one place the desugaring is not a pure alias: a launch.
    ///
    /// `sessions` has no verb for starting a harness, so when what follows is
    /// not a session verb the prefix still names the session and the command
    /// runs where it lives. Everything after a harness name is still the
    /// harness's argv.
    #[test]
    fn a_session_named_first_also_works_for_what_sessions_has_no_verb_for() {
        assert_eq!(
            session_prefix(cli_argv(&["s01", "claude", "--resume", "x"])),
            (
                Some("s01".to_string()),
                cli_argv(&["claude", "--resume", "x"])
            )
        );
        assert_eq!(
            session_prefix(cli_argv(&["s01", "attach", "zed"])),
            (Some("s01".to_string()), cli_argv(&["attach", "zed"]))
        );
        // `graph` had a positional of its own until the prefix landed, and for
        // one commit it had both — the prefix set the session and `graph` read
        // the positional, so the browser opened on whichever session `pick`
        // chose. This asserts the *lifting*, which is all this function does.
        // What happens next is a refusal: the graph is one server per repo, so
        // `omh s01 graph` names a scope nothing can honour and `dispatch` says
        // so rather than opening on a session the id had no part in choosing.
        let (named, argv) = session_prefix(cli_argv(&["s01", "graph"]));
        assert_eq!(
            the_one_session(named, Cli::try_parse_from(&argv).unwrap().session).unwrap(),
            Some("s01".to_string()),
            "the prefix is lifted whatever follows it — `consumes_session` is \
             what decides whether the command may have it"
        );
    }

    /// A session and nothing to do with it is a question, and `s` asks it.
    ///
    /// Neither reading parses, and which error surfaces is a choice: as
    /// written, `omh s01` is a request to launch a harness called `s01` and the
    /// answer would be ``unknown harness `s01` `` — true, useless, and about
    /// the wrong thing.
    #[test]
    fn a_session_with_nothing_to_do_is_asked_what_to_do() {
        assert_eq!(
            session_prefix(cli_argv(&["s01"])),
            (Some("s01".to_string()), cli_argv(&["s"]))
        );
    }

    /// A harness's own `-s` is not omh naming the session twice.
    ///
    /// `passthrough` already records why, in the same file: "`-s` is omh's
    /// session flag and is also a flag plenty of harnesses have; refusing
    /// shorts would break launches that work today to guard a mistake nobody
    /// has made." Everything after a harness name belongs to the harness, and
    /// the duplicate check has to respect the same boundary — a check that
    /// reads the whole line refuses a launch that has always worked.
    #[test]
    fn a_harness_flag_is_not_omh_naming_the_session_twice() {
        assert_eq!(
            session_prefix(cli_argv(&["s01", "claude", "-s", "some-session"])),
            (
                Some("s01".to_string()),
                cli_argv(&["claude", "-s", "some-session"])
            ),
            "the harness keeps its own flags"
        );
    }

    /// omh's own flags may sit between the session and the verb.
    ///
    /// `omh s01 --json diff` is an ordinary thing to type, and the first
    /// version of this could not read it: it classified `argv[2]` and nothing
    /// else, so a flag there was neither a verb nor a harness name and the line
    /// went to the harness dispatcher as `unknown harness \`diff\``.
    ///
    /// Rather than teach this function which flags take values — knowledge that
    /// already lives in the parser and would rot here — the sessions reading is
    /// tried and kept when it parses.
    #[test]
    fn omhs_own_flags_may_sit_between_the_session_and_the_verb() {
        assert_eq!(
            session_prefix(cli_argv(&["s01", "--json", "diff"])),
            (Some("s01".to_string()), cli_argv(&["s", "--json", "diff"]))
        );
        assert_eq!(
            session_prefix(cli_argv(&["s01", "--dry-run", "claude"])),
            (Some("s01".to_string()), cli_argv(&["--dry-run", "claude"])),
            "and a harness is still a harness"
        );
    }

    /// Every session command line omh prints is one omh accepts.
    ///
    /// Deleting the positionals broke three printed suggestions at once, in
    /// three files — `omh s down {id}`, `omh s diff {id}` and *clear each with
    /// `omh s rm <id>`* — and every test stayed green, because a suggestion is
    /// a string until someone types it. One of them had been wrong since it was
    /// written for an unrelated reason, which is what advice nobody runs looks
    /// like.
    ///
    /// Scoped to lines naming a session, not every `omh …` in the tree: the
    /// rest are wrapped in prose that no cutting rule separates cleanly, and a
    /// guard needing an exception list is one that gets an exception added
    /// instead of a bug fixed. This is the class the prefix put at risk.
    ///
    /// What it does not catch is a verb spelled wrong — a line has to name a
    /// real one to be recognised as a session line at all. The class here is
    /// *where the id goes*, which is what changed.
    ///
    /// The line goes through `session_prefix` before the parser, because that
    /// is the path a typed line takes — checking it against `Cli` alone would
    /// call `omh s01 diff` a failure and `omh s diff s01` a success, both
    /// backwards.
    /// No attribute has been separated from the item it applies to.
    ///
    /// The other half of the same accident, and the half that keeps happening:
    /// inserting before an anchor without reading what precedes it walks back
    /// over the *next* item's `#[test]` and strands it above the new one. The
    /// stranded attribute then applies to whatever the insertion brought, and
    /// the test it came from silently stops being a test — which is how a
    /// helper it alone called became dead code and took the lint run down.
    ///
    /// Three times in two changes. Clippy refuses `duplicated attribute`, but
    /// clippy does not run on this machine — a 1.81 shim against a 1.85 crate
    /// — so CI is the only place it is caught, one push and four minutes
    /// later. This is the same guard, here.
    ///
    /// Two shapes: a `#[test]` that no longer sits on a function, and **any**
    /// attribute that sits above a doc comment. The second was added after the
    /// first failed to catch the same mistake made to a `#[derive(…)]` — this
    /// paragraph used to say derives "legitimately stack and sit above doc
    /// comments in this tree", which was asserted rather than checked. There
    /// were zero such instances; a doc comment always comes first here.
    #[test]
    fn no_test_attribute_was_stranded_from_its_function() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut stranded = Vec::new();
        let mut checked = 0;
        for dir in ["src", "tests"] {
            for file in std::fs::read_dir(root.join(dir)).unwrap() {
                let file = file.unwrap().path();
                if file.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                checked += 1;
                let body = std::fs::read_to_string(&file).unwrap();
                let lines: Vec<&str> = body.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    if line.trim() != "#[test]" {
                        continue;
                    }
                    // What may follow: the function, or more attributes
                    // (`#[should_panic]`, `#[ignore]`). Anything else — a doc
                    // comment, a blank line, another `#[test]` — means this one
                    // is no longer attached to what it was written for.
                    let next = lines.get(n + 1).map(|l| l.trim()).unwrap_or("");
                    let attached = next.starts_with("fn ")
                        || next.starts_with("async fn ")
                        || (next.starts_with("#[") && next != "#[test]");
                    if !attached {
                        stranded.push(format!(
                            "{}:{}: followed by `{next}`",
                            file.display(),
                            n + 1
                        ));
                    }
                }
                // Any attribute above a doc comment — the same accident
                // wearing a different hat. An insertion before an anchor
                // swallowed `report::Down`'s `#[derive(…)]` and left it
                // heading the new block, after the `#[test]` half of this was
                // already written, so the rule is the general one now.
                for (n, line) in lines.iter().enumerate() {
                    if line.trim().starts_with("#[")
                        && lines
                            .get(n + 1)
                            .is_some_and(|l| l.trim().starts_with("///"))
                    {
                        stranded.push(format!(
                            "{}:{}: `{}` sits above a doc comment",
                            file.display(),
                            n + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
        assert!(checked > 1, "the scan found no sources to read");
        assert!(
            stranded.is_empty(),
            "`#[test]` separated from its function: {stranded:#?}"
        );
    }

    /// No source or document still tells anyone to type a verb that is gone.
    ///
    /// `the_session_lines_omh_prints_are_lines_omh_accepts` cannot do this,
    /// and the reason is worth writing down: it only checks a line whose
    /// second word is a **known** session verb. Retiring `ls` therefore did
    /// not make those lines fail — it quietly removed them from the scan, and
    /// two user-facing messages went on naming a command that no longer
    /// parses. A guard keyed on the current vocabulary cannot see a word
    /// leaving it.
    ///
    /// Docs are included because nothing else reads them for command
    /// spellings: `tests/docs.rs` checks links, anchors and reachability.
    ///
    /// The whole tree is walked rather than an allowlist of directories. The
    /// first version read `src`, `tests` and `docs`, and stayed green while
    /// `README.md` — the front page, and the file most people read first —
    /// still printed the retired verb. An allowlist is the same shape of
    /// mistake as keying on the current vocabulary: both go quiet about the
    /// place nobody thought of.
    ///
    /// It cannot tell *offering* a spelling from *saying it is gone*, so prose
    /// explaining the removal has to name the verb rather than the whole
    /// invocation — "there is no `ls` verb", not the line somebody used to
    /// type. That is a real limitation and the cheaper side of it: a scan that
    /// tried to judge intent would let the next one through.
    #[test]
    fn nothing_still_offers_a_verb_that_was_retired() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // Spellings omh used to accept and does not. `ls` became the absence
        // of a verb in 2026.08 — `omh s` is the listing and `omh sNN` is one
        // row of it.
        //
        // Assembled rather than written whole, and that is not decoration: the
        // sweep that retired the verb was a search-and-replace over `src` and
        // `docs`, and it rewrote this very constant, turning the guard into
        // one that matched every file in the tree. A guard against a spelling
        // cannot spell it.
        // Both the written spelling and the argv form the tests use. The ~15
        // call sites the sweep rewrote were argv, so the shape most likely to
        // be left behind was the one a prose-only needle cannot see — and one
        // was: the JSON guard went on invoking a line that no longer parsed,
        // and passed, because its empty stdout read as nothing to say.
        const ON_PURPOSE: &str = "types the retired verb on purpose";
        let gone: [String; 4] = [
            format!("omh s {}", "ls"),
            format!("omh sessions {}", "ls"),
            format!("{:?}, {:?}", "s", "ls"), // types the retired verb on purpose
            format!("{:?}, {:?}", "sessions", "ls"), // types the retired verb on purpose
        ];
        let mut found = Vec::new();
        let mut read = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Build output and git's own storage are not ours to read,
                    // and `target` alone is large enough to matter.
                    if !matches!(
                        path.file_name().and_then(|n| n.to_str()),
                        Some("target") | Some(".git")
                    ) {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs" && e != "md") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).unwrap();
                read.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
                for (n, line) in body.lines().enumerate() {
                    // Declared, not inferred. Two lines have to type the verb
                    // to do their job — the needles just above, and the test
                    // that checks typing it is refused — and a scan that tried
                    // to work out which those were would be a scan that let
                    // the next one through. Saying so on the line is cheap and
                    // greppable; guessing is neither.
                    if line.contains(ON_PURPOSE) {
                        continue;
                    }
                    for spelling in &gone {
                        if line.contains(spelling.as_str()) {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        // Named files rather than a count. A count cannot tell a walk that
        // stopped early from one that read everything, and reading everything
        // is the only claim this guard makes that is worth anything.
        for must in ["README.md", "src/main.rs", "docs/commands.md"] {
            assert!(
                read.iter().any(|p| p == std::path::Path::new(must)),
                "the scan never read {must}, so its silence says nothing"
            );
        }
        assert!(
            found.is_empty(),
            "these still offer a command omh no longer accepts: {found:#?}"
        );
    }

    /// No doc comment has been spliced onto itself.
    ///
    /// A specific accident with a specific cause: these files are edited by
    /// matching an anchor and inserting before it, and an anchor chosen
    /// without reading the line above it lands the new text *inside* the
    /// previous item's doc block, and the sentence that was there ends up
    /// carrying a second doc marker partway along. It compiles, it renders as
    /// one run-on sentence, and it reads as deliberate.
    ///
    /// It shipped five times before anything looked for it, across four files
    /// and four separate changes. Nothing else in the suite can see it: the
    /// text is a comment, so no behaviour changes and no assertion fails.
    ///
    /// A doc comment that quotes the shape trips this — the first draft of
    /// this one did. Reword it; the check has no way to tell an example from
    /// the thing, and the thing is worth more than the example.
    ///
    /// It does not catch every splice, and this test was itself installed by
    /// one it cannot see: the insertion walked back over the *next* test's
    /// `#[test]` and left it stranded above this one. That shape is a
    /// duplicated attribute, which clippy already refuses — so the two
    /// together cover the accident, and only the comment half needed
    /// something new.
    #[test]
    fn no_doc_comment_was_spliced_onto_itself() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0;
        let mut doubled = Vec::new();
        for dir in ["src", "tests"] {
            for file in std::fs::read_dir(root.join(dir)).unwrap() {
                let file = file.unwrap().path();
                if file.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                checked += 1;
                for (n, line) in std::fs::read_to_string(&file).unwrap().lines().enumerate() {
                    // Two on one line. A doc comment is the whole line by
                    // construction, so a second one is always a splice — and
                    // `///` inside a string or a URL is a different shape
                    // (`http://`), which has two slashes and not three.
                    if line.matches("///").count() > 1 {
                        doubled.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(checked > 1, "the scan found no sources to read");
        assert!(
            doubled.is_empty(),
            "doc comments spliced together: {doubled:#?}"
        );
    }
    #[test]
    fn the_session_lines_omh_prints_are_lines_omh_accepts() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for file in std::fs::read_dir(&src).unwrap() {
            let file = file.unwrap().path();
            if file.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // A message wide enough to wrap is written with Rust's string
            // continuation, which eats the newline and the indent that follows
            // it. Read one line at a time, `omh s commit --keep` looks like
            // ``omh s commit \``, so the scan has to join what the compiler
            // joins before it reads anything.
            let body = std::fs::read_to_string(&file).unwrap();
            let mut joined = String::new();
            let mut continued = false;
            for line in body.lines() {
                // What the compiler drops: the newline, and the indent the
                // next line begins with.
                let line = if continued { line.trim_start() } else { line };
                match line.strip_suffix('\\') {
                    // `\\\\` is an escaped backslash, not a continuation, and
                    // stripping one leaves the other — which still ends in a
                    // backslash, so this asks again rather than assuming.
                    Some(head) if !head.ends_with('\\') => {
                        joined.push_str(head);
                        continued = true;
                    }
                    _ => {
                        joined.push_str(line);
                        joined.push('\n');
                        continued = false;
                    }
                }
            }
            let body = joined;
            for raw in body.lines() {
                // Comments describe old spellings on purpose — `session_prefix`
                // documents all four it replaced.
                if raw.trim_start().starts_with("//") {
                    continue;
                }
                for (at, _) in raw.match_indices("omh ") {
                    let rest = &raw[at + "omh ".len()..];
                    // A printed line ends where the message resumes: a newline
                    // escape, the end of the literal, backticked prose, the
                    // separator `omh s` uses, or the column padding that lines
                    // an explanation up beside it.
                    let end = ["\\n", "\"", "`", "·", "  ", ","]
                        .iter()
                        .filter_map(|stop| rest.find(stop))
                        .min()
                        .unwrap_or(rest.len());
                    let line = rest[..end].trim();
                    // Placeholders stand for an id or a name; either way a
                    // session id is the value that makes the line whole.
                    let filled = regex_lite_fill(line);
                    let words: Vec<&str> = filled.split_whitespace().collect();
                    // A session and something to do with it. The verb is part
                    // of the test rather than assumed, because `omh {}` — a
                    // format string whose whole command is a runtime value —
                    // fills to a bare id and is not a line anyone printed.
                    let names_a_session = matches!(words.first(), Some(&"s" | &"sessions" | &"s01"))
                        && words.get(1).is_some_and(|w| is_a_session_verb(w))
                        // A line ending in a flag is naming the flag, not
                        // showing a command: five messages in `shadow.rs` say
                        // *take the files as they stand with `omh s commit
                        // -m`*, and the value the reader supplies is the point.
                        && words.last().is_some_and(|w| !w.starts_with('-'));
                    if !names_a_session {
                        continue;
                    }
                    let argv: Vec<String> = std::iter::once("omh")
                        .chain(words)
                        .map(str::to_string)
                        .collect();
                    let (_, argv) = session_prefix(argv);
                    assert!(
                        Cli::try_parse_from(&argv).is_ok(),
                        "{} prints `omh {line}`, which omh does not accept",
                        file.file_name().unwrap().to_string_lossy()
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked >= 4,
            "the scan found only {checked} session lines — it stopped reading, \
             which is how this passes while saying nothing"
        );
    }

    /// `{id}`, `{}`, `<id>`: whichever a message uses, fill it.
    ///
    /// The first placeholder is the session — that is the shape of every line
    /// this scan reads, since the session goes first. A later one is a value
    /// the verb takes, and `1` is the fill because it is valid where a number
    /// is required and harmless where a word is. Filling every slot with a
    /// session id was the first version, and it turned the real hint
    /// `omh {} diff {}` into `omh s01 diff s01` — refused for a reason that was
    /// the test's own doing rather than the line's.
    fn regex_lite_fill(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        let mut first = true;
        while let Some(open) = rest.find(['{', '<']) {
            let close = if rest.as_bytes()[open] == b'{' {
                '}'
            } else {
                '>'
            };
            let Some(shut) = rest[open..].find(close) else {
                break;
            };
            out.push_str(&rest[..open]);
            out.push_str(if first { "s01" } else { "1" });
            first = false;
            rest = &rest[open + shut + 1..];
        }
        out.push_str(rest);
        out
    }

    /// Anything that is not `sNN` is left exactly as it was.
    #[test]
    fn a_command_that_is_not_a_session_is_not_read_as_one() {
        for line in [
            vec!["s", "diff"],
            vec!["init"],
            vec!["claude"],
            vec!["sessions", "log"],
            // a harness whose name merely starts with s
            vec!["sourcegraph"],
        ] {
            assert_eq!(
                session_prefix(cli_argv(&line)),
                (None, cli_argv(&line)),
                "{line:?} is not a session prefix"
            );
        }
    }

    /// Naming the session twice is a question, not something to resolve.
    ///
    /// Every spelling of the flag, because the guard this replaces scanned argv
    /// for the two whole tokens `--session` and `-s` and let `--session=s02`
    /// and `-ss02` through — and letting one through is worse than having no
    /// guard, since the line then runs against whichever id won.
    #[test]
    fn a_session_named_twice_is_refused_rather_than_picked() {
        for flag in [
            vec!["--session", "s02"],
            vec!["--session=s02"],
            vec!["-s", "s02"],
            vec!["-ss02"],
        ] {
            let line: Vec<&str> = std::iter::once("s01")
                .chain(flag.iter().copied())
                .chain(std::iter::once("diff"))
                .collect();
            let (prefix, argv) = session_prefix(cli_argv(&line));
            let parsed = Cli::try_parse_from(&argv)
                .unwrap_or_else(|e| panic!("{line:?} has to reach the parser: {e}"));
            let err = the_one_session(prefix, parsed.session)
                .expect_err("two names for one session is not something to guess at");
            assert!(
                err.to_string().contains("s01") && err.to_string().contains("s02"),
                "the refusal has to name both, for {line:?}: {err}"
            );
        }
    }

    /// A harness's own `-s` still is not omh naming the session twice.
    ///
    /// The pair above and this one are the two halves of one boundary, and a
    /// guard that satisfies either alone is easy to write: refusing the whole
    /// line refuses this launch, and refusing nothing lets the pair above
    /// through. Read through the parser they separate themselves — everything
    /// after a harness name is the harness's argv, so the flag never becomes
    /// omh's `session`.
    #[test]
    fn a_harness_flag_is_still_not_omh_naming_the_session_twice() {
        let (prefix, argv) = session_prefix(cli_argv(&["s01", "claude", "-s", "some-session"]));
        let parsed = Cli::try_parse_from(&argv).expect("a launch is a valid line");
        assert_eq!(
            the_one_session(prefix, parsed.session).unwrap(),
            Some("s01".to_string()),
            "the harness keeps its own flags and the session stays the prefix's"
        );
    }

    /// The log reports the session it was asked about, from that session's own
    /// sandbox repository.
    ///
    /// Nothing reached this wiring before it was split out of `log_cmd`:
    /// replacing the read with an empty list left the whole suite green, and
    /// `omh sNN log` would have answered *the agent has not committed
    /// anything* for every session on earth. The shadow tests call
    /// `checkpoints` directly and the report tests build the value by hand, so
    /// both halves stayed correct while nothing joined them.
    ///
    /// Two sessions with different work in them, because with one the wiring
    /// cannot be wrong in a way this notices.
    #[test]
    fn the_log_reads_the_named_sessions_own_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .expect("git must be installed to run this test");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "root"]);
        std::fs::create_dir_all(paths.shadows()).unwrap();

        let mut sessions = Vec::new();
        for (id, subject) in [("s01", "Only in s01"), ("s02", "Only in s02")] {
            let session = Session::new(&paths.worktrees().join(id), id.to_string());
            session.ensure(&paths.repo, "main").unwrap();
            let shadow = shadow::Shadow::new(&paths.shadows(), id);
            shadow.ensure(&session.worktree, &[]).unwrap();
            std::fs::write(session.worktree.join("work.rs"), format!("// {subject}\n")).unwrap();
            // Through the sandbox's own repository, the way the agent commits.
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", subject],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
            sessions.push(session);
        }

        for (session, mine, theirs) in [
            (&sessions[0], "Only in s01", "Only in s02"),
            (&sessions[1], "Only in s02", "Only in s01"),
        ] {
            let log = log_report(&paths, session, false, &out::Ctx::plain()).unwrap();
            let subjects: Vec<&str> = log
                .read
                .commits
                .iter()
                .map(|c| c.subject.as_str())
                .collect();
            assert!(
                subjects.contains(&mine) && !subjects.contains(&theirs),
                "{}'s log is {}'s work: {subjects:?}",
                session.id,
                session.id
            );
        }
    }

    /// `--keep` binds a value only when one is given.
    ///
    /// `num_args = 0..=1` with `default_missing_value` is the clap shape most
    /// prone to silently binding the wrong thing — a following flag, or a
    /// following verb — and the three forms are one assertion each.
    #[test]
    fn keep_takes_a_selection_or_nothing_and_never_the_next_word() {
        let parse = |args: &[&str]| {
            let argv: Vec<String> = std::iter::once("omh")
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect();
            match Cli::try_parse_from(&argv).map(|cli| cli.cmd) {
                Ok(Cmd::Sessions {
                    cmd: Some(SessionsCmd::Commit { keep, edit, .. }),
                }) => Ok((keep, edit)),
                Ok(_) => panic!("{args:?} did not parse as a commit"),
                Err(e) => Err(e.to_string()),
            }
        };

        assert_eq!(parse(&["s", "commit"]).unwrap(), (None, false));
        assert_eq!(
            parse(&["s", "commit", "--keep"]).unwrap(),
            (Some(String::new()), false),
            "a bare --keep is not a missing value"
        );
        assert_eq!(
            parse(&["s", "commit", "--keep", "1,3-4"]).unwrap(),
            (Some("1,3-4".to_string()), false)
        );
        assert_eq!(
            parse(&["s", "commit", "--keep", "--edit"]).unwrap(),
            (Some(String::new()), true),
            "--keep does not swallow the flag after it"
        );
        assert!(
            parse(&["s", "commit", "--edit"])
                .unwrap_err()
                .contains("--keep"),
            "--edit is about the list --keep takes"
        );
        assert!(
            parse(&["s", "commit", "--keep", "-m", "x"])
                .unwrap_err()
                .contains("cannot be used with"),
            "and squashing is the other way to land work, not a modifier of this one"
        );
    }

    /// A commit refuses over markers a sync left behind, says where they are,
    /// and can be meant anyway.
    #[test]
    fn a_commit_will_not_land_an_unresolved_conflict_unless_it_is_meant() {
        let none: Vec<String> = vec![];
        assert!(
            may_commit("s01", &none, false).is_ok(),
            "a clean tree commits"
        );

        let one = vec!["src/tap.rs:12: leftover conflict marker".to_string()];
        let said = may_commit("s01", &one, false).unwrap_err().to_string();
        assert!(
            said.contains("src/tap.rs:12"),
            "it says where, so the user is not sent hunting: {said}"
        );
        assert!(
            said.contains("1 conflict marker in"),
            "and one is not `1 conflict markers`: {said}"
        );
        assert!(
            said.contains("--force"),
            "and the way past is in the refusal: {said}"
        );
        assert!(may_commit("s01", &one, true).is_ok(), "`--force` means it");

        // A whole-file conflict is one line per hunk. The refusal has to stay
        // readable at that size, or the way past scrolls off the screen.
        let many: Vec<String> = (1..=40)
            .map(|n| format!("src/big.rs:{n}: leftover conflict marker"))
            .collect();
        let said = may_commit("s01", &many, false).unwrap_err().to_string();
        assert_eq!(
            said.matches("leftover conflict marker").count(),
            5,
            "five lines, not forty: {said}"
        );
        assert!(
            said.contains("40 conflict markers") && said.contains("…and 35 more"),
            "and the count is still the whole truth: {said}"
        );
    }

    /// omh does not act on a container question it could not answer.
    ///
    /// The decision points all read the same way — start it, enter it, stop
    /// it, sync over it — and each is worse done blind than not done. The one
    /// that made this worth changing is `sync`: it asks whether the sandbox is
    /// up to decide whether writing files would land under a live agent, and
    /// an unreachable runtime used to answer *nothing is running*.
    #[test]
    fn a_container_question_omh_could_not_answer_stops_the_command() {
        assert!(must_know(image::Running::Yes, "s01", "sync over it").unwrap());
        assert!(!must_know(image::Running::No, "s01", "sync over it").unwrap());

        let refused = must_know(
            image::Running::Unknown("daemon not reachable".into()),
            "s01",
            "sync over it",
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("sync over it"),
            "it says what it declined to do: {refused}"
        );
        assert!(
            refused.contains("daemon not reachable"),
            "and the runtime's reason, which is the only actionable part: {refused}"
        );
        // The subject is a parameter because two of the callers are `graph`,
        // which asks about a per-repo UI container and not a session. The
        // sentence used to say "the sandbox" for those, sending the reader to
        // look at sessions.
        assert!(
            refused.contains("s01") && !refused.contains("the sandbox"),
            "and names what it asked about: {refused}"
        );
        let graph = must_know(
            image::Running::Unknown("daemon not reachable".into()),
            "the graph",
            "stop it",
        )
        .unwrap_err()
        .to_string();
        assert!(graph.contains("the graph is running"), "got: {graph}");
    }

    /// The reaper's inverted safe direction, which is the one place *could not
    /// tell* must not lead to a refusal — there is nobody to refuse to.
    ///
    /// A named predicate rather than a `matches!` inside a filter, because the
    /// mutation that matters — `Unknown` becoming reapable, so a flapping
    /// daemon stops live agents' sandboxes on a guess — left the whole suite
    /// green while it was inline.
    #[test]
    fn a_session_omh_cannot_ask_about_is_never_reaped() {
        assert!(reapable(&image::Running::Yes), "a live one may be reaped");
        assert!(
            !reapable(&image::Running::No),
            "one already down has nothing to reap"
        );
        assert!(
            !reapable(&image::Running::Unknown("daemon down".into())),
            "and one omh could not ask about is left alone — stopping a live \
             session on a guess costs somebody's turn"
        );
    }

    /// Turn snapshots are named when a session is removed, and are never the
    /// reason it is refused.
    ///
    /// The judgement this encodes, since it is not the only defensible one. A
    /// snapshot is a tree omh photographed at the end of a turn, and there is
    /// one for nearly every session that ever ran — so refusing over them
    /// would make this guard fire almost always, and a guard that fires almost
    /// always is answered with `--force` unread. That is how it would stop
    /// protecting the agent's own commits, which is what it was built for.
    ///
    /// They are still said, because they do go, and because the one time they
    /// matter is the one time the agent threw the tree away.
    #[test]
    fn a_removal_names_the_turn_snapshots_it_takes_without_refusing_over_them() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        // Harvested first, so nothing of the agent's own is at stake — which
        // is the branch this decision is really about, and the one `force:
        // true` would have skipped straight past.
        shadow
            .harvest(
                &paths.repo,
                &session.worktree,
                "omh/s01",
                &[],
                shadow::Keep::All,
            )
            .unwrap();

        let note = may_remove(&paths, &session, Snapshots::Kept(12), false)
            .expect("snapshots alone never stop a removal")
            .expect("but they are said");
        assert!(
            note.contains("12 turn snapshots") && note.contains("log --turns"),
            "named, with the way to read them: {note}"
        );

        assert_eq!(
            may_remove(&paths, &session, Snapshots::None, false).unwrap(),
            None,
            "and a session that has none says nothing about them"
        );

        // With the agent's own commits at stake it still refuses — that is
        // the guard this must not have weakened — and the snapshots ride along
        // in the same message rather than displacing it.
        let (paths, unkept, _shadow) = a_session_with_two_checkpoints();
        let refused = may_remove(&paths, &unkept, Snapshots::Kept(3), false)
            .expect_err("unharvested commits still refuse")
            .to_string();
        assert!(
            refused.contains("commit --keep"),
            "the refusal is unchanged: {refused}"
        );
        assert!(
            refused.contains("and 3 turn snapshots"),
            "and says what else goes: {refused}"
        );
        // The rule the rest of this message keeps: everything under the first
        // line is a command, so the snapshots are mentioned in the sentence
        // and the way to read them is offered as one.
        assert!(
            refused.contains("omh s01 log --turns"),
            "with a command for it: {refused}"
        );
        for line in refused
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            assert!(
                line.starts_with("omh s01 "),
                "a line the user cannot paste: {line}"
            );
        }
    }

    /// A sync that cannot leave its note is still a sync, and says which.
    ///
    /// The design decision the call site argues for, asserted rather than
    /// commented: the merge has landed, the baseline has moved and the shadow
    /// has its commit by the time the note is written, so failing here would
    /// report finished work as a failed command. The other half — that the
    /// user hears about it — is what carrying it on the report rather than
    /// printing it inside makes checkable at all.
    #[test]
    fn a_sync_whose_note_cannot_be_written_still_succeeds_and_says_so() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let repo_git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        repo_git(&["checkout", "-q", "main"]);
        std::fs::write(paths.repo.join("from-trunk.rs"), "fn trunk() {}\n").unwrap();
        repo_git(&["add", "-A"]);
        repo_git(&["commit", "-qm", "trunk moved"]);

        // A directory where the note goes. Contrived, and the reachable
        // versions — a full disk, a permission the host process does not have
        // — are not things a test can arrange on demand.
        std::fs::create_dir(shadow::note_file(&shadow.gitdir)).unwrap();

        let synced = sync_session(&paths, &session, "main").expect("the sync itself is fine");
        assert_eq!(synced.moved, 1, "the work still arrived");
        assert!(
            session.worktree.join("from-trunk.rs").exists(),
            "and is on disk, which is what a failed command would deny"
        );
        let why = synced.note.expect("the failure is carried, not swallowed");
        assert!(
            why.contains("omh-note"),
            "naming the file it could not write: {why}"
        );
    }

    /// A sync brings trunk over, explains itself in the sandbox, and leaves
    /// the agent's work where a harvest can still take it.
    ///
    /// The whole mechanism end to end, because every part of it is only worth
    /// anything in combination: a merged file the agent never asked for, a
    /// commit that says what moved, a baseline that makes `diff` mean the
    /// agent's work again — and the checkpoint that makes all of it
    /// recoverable.
    ///
    /// The last assertion is the one that decides a design question. The spec
    /// says to advance the replay point past the commit omh writes, so a
    /// harvest never replays trunk's changes as the agent's. That cannot be
    /// done with a single ancestor pointer without also marking the checkpoint
    /// *below* it as handed over — and that checkpoint is the agent's own
    /// uncommitted work. So the replay point stays where it was, and this
    /// asserts what the spec was protecting: after a sync, `--keep` puts the
    /// agent's work on the branch and does not land trunk's twice.
    #[test]
    fn a_sync_brings_trunk_over_and_leaves_the_agents_work_harvestable() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let repo_git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        // The agent leaves something uncommitted, so the checkpoint has work.
        std::fs::write(session.worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();

        // Trunk moves: one file the session never touched.
        let on_session = repo_git(&["rev-parse", "HEAD"]);
        repo_git(&["checkout", "-q", "main"]);
        std::fs::write(paths.repo.join("from-trunk.rs"), "fn trunk() {}\n").unwrap();
        repo_git(&["add", "-A"]);
        repo_git(&["commit", "-qm", "trunk moved"]);
        let onto = repo_git(&["rev-parse", "HEAD"]);
        let _ = on_session;

        let synced = sync_session(&paths, &session, "main").unwrap();
        assert_eq!(synced.moved, 1, "one commit arrived");
        assert!(synced.conflicted.is_empty(), "and it merged cleanly");
        assert!(synced.checkpoint, "the uncommitted work was checkpointed");

        // 1. The file trunk added is in the session now.
        assert!(
            session.worktree.join("from-trunk.rs").exists(),
            "trunk's file reached the session"
        );
        // 2. The branch sits on the new base.
        assert_eq!(
            repo_git(&["rev-parse", "omh/s01"]),
            onto,
            "the baseline moved, so `diff` measures the agent's work and not trunk's"
        );
        // 3. The sandbox can read what happened.
        let sandbox_log = shadow.checkpoints(&session.worktree).unwrap();
        let subjects: Vec<&str> = sandbox_log
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert!(
            subjects.iter().any(|s| s.starts_with("base moved to")),
            "a commit the agent can read: {subjects:?}"
        );
        assert!(
            subjects.iter().any(|s| s.contains("Before omh brought")),
            "and the point it can be undone from: {subjects:?}"
        );

        // 4. The sentence the agent is given when it starts again — at the
        //    host path that is the mount's other end, since a note written
        //    anywhere else is delivered to nobody, in silence, forever.
        let note = std::fs::read_to_string(shadow::note_file(&shadow.gitdir))
            .expect("a note was left where the hook reads");
        assert!(
            note.contains("main moved 1 commit") && note.contains("git show HEAD"),
            "what moved, and where to read it: {note}"
        );
        assert_eq!(
            synced.note, None,
            "and nothing to report about leaving it: {synced:?}"
        );

        // 5. …and the agent's work is still there to be taken.
        let landed = shadow
            .harvest(
                &paths.repo,
                &session.worktree,
                "omh/s01",
                &[],
                shadow::Keep::All,
            )
            .unwrap();
        let on_branch = repo_git(&["log", "--format=%s", &format!("{onto}..omh/s01")]);
        assert!(landed > 0, "the harvest took something: {on_branch}");
        assert!(
            on_branch.contains("one") && on_branch.contains("two"),
            "the agent's own commits reached the branch: {on_branch}"
        );
        assert_eq!(
            on_branch.matches("base moved to").count(),
            0,
            "and trunk's changes did not arrive a second time as the agent's: {on_branch}"
        );
    }

    /// A session holding work no branch has is not removed by accident.
    ///
    /// The last piece of [risks](../docs/design/risks.md) 2c. The branch
    /// survives a removal and the worktree's files were on disk until it ran,
    /// but the agent's own commits live only in the sandbox's repository and
    /// `reap` deletes it. omh could not ask this until `log` learned to count.
    #[test]
    fn a_session_holding_unkept_work_is_not_removed_without_being_asked() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        let err = may_remove(&paths, &session, Snapshots::None, false)
            .expect_err("two commits are on no branch anywhere");
        let said = err.to_string();
        assert!(
            said.contains("s01 has 2 commits that no branch has"),
            "it says how much is at stake, and agrees with itself about the number: {said}"
        );
        // Every line offered is a command, and every one names this session —
        // a refusal that walks the user to `--force` by elimination is worse
        // than no refusal.
        for line in said
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            assert!(
                line.starts_with("omh s01 "),
                "a line the user cannot paste: {line}"
            );
        }
        assert!(
            said.contains("--keep") && said.contains("commit -m") && said.contains("--force"),
            "put it on the branch, take the files as they stand, or mean it: {said}"
        );
        assert!(
            said.contains("omh/s01"),
            "and names the branch it would go on: {said}"
        );
        assert!(
            may_remove(&paths, &session, Snapshots::None, true).is_ok(),
            "`--force` means it"
        );

        // Everything handed over: nothing to warn about.
        let head = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.gitdir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        std::fs::write(
            &shadow.landed_record,
            String::from_utf8_lossy(&head.stdout).trim(),
        )
        .unwrap();
        assert!(
            may_remove(&paths, &session, Snapshots::None, false).is_ok(),
            "a session whose work is all on the branch removes quietly"
        );
    }

    /// Work the agent threw away is still work only this repository has.
    ///
    /// `reset --hard` is one of the four commands the sandbox's own git exists
    /// to give back, and the first version of this guard was blind to it:
    /// `seed..HEAD` counts 0 afterwards, so `rm` removed three commits without
    /// a word — the exact scenario `risks.md` cites as the reason the guard
    /// exists. Measured: `--all --reflog` still finds them.
    ///
    /// The same read covers a side branch the agent wandered off, which
    /// `preflight` refuses a *harvest* over while `rm` was dropping it for
    /// good.
    #[test]
    fn work_the_sandbox_can_still_reach_counts_however_the_agent_left_it() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let sandbox_git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let seed = shadow.seed().unwrap();

        // The agent throws its own work away.
        sandbox_git(&["reset", "-q", "--hard", &seed]);
        assert!(
            shadow
                .checkpoints(&session.worktree)
                .unwrap()
                .commits
                .is_empty(),
            "the numbered list cannot see them — that is why this guard reads wider"
        );
        let err = may_remove(&paths, &session, Snapshots::None, false)
            .expect_err("two commits are still in there and on no branch");
        assert!(err.to_string().contains("2 commits"), "{err}");

        // …and when the replay point no longer reaches, the count widens back
        // to the seed rather than trusting a record the history has left
        // behind. Narrower would mean counting from a commit this repository
        // cannot place, which is how work goes missing from a number someone
        // is about to act on.
        std::fs::write(&shadow.landed_record, "0".repeat(40)).unwrap();
        let err = may_remove(&paths, &session, Snapshots::None, false)
            .expect_err("a record naming nothing this repository has is not an answer");
        assert!(
            err.to_string().contains("cannot say what that removes"),
            "omh says it cannot tell rather than counting from a point it cannot place: {err}"
        );
        std::fs::remove_file(&shadow.landed_record).unwrap();

        // …and the same for a branch it wandered off.
        sandbox_git(&["checkout", "-q", "-b", "spike"]);
        std::fs::write(session.worktree.join("spike.rs"), "fn spike() {}\n").unwrap();
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "a spike"]);
        sandbox_git(&["checkout", "-q", "-"]);
        let err = may_remove(&paths, &session, Snapshots::None, false).expect_err("three now");
        assert!(err.to_string().contains("3 commits"), "{err}");
    }

    /// What the seed record alone settles — including the arm a filesystem
    /// test cannot reach.
    ///
    /// `chmod 000` does not stop uid 0, so a permissions arm asserted through
    /// a real file passes vacuously wherever the suite runs as root. It is
    /// also the arm that silently deleted a sandbox: `Path::exists` answers
    /// `false` for a permissions error exactly as it does for absence.
    #[test]
    fn what_the_seed_record_settles_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let shadow = shadow::Shadow::new(dir.path(), "s01");
        let missing = || std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");

        assert!(
            from_the_seed_record(Ok(()), true, &shadow).is_none(),
            "a record omh can read settles nothing on its own — the repository decides"
        );
        assert!(
            matches!(
                from_the_seed_record(Err(missing()), false, &shadow),
                Some(AtStake::Nothing)
            ),
            "no record and no repository: nothing ever ran here"
        );
        // What `reap` leaves when `remove_dir_all` fails on a live mount and
        // the seed file goes anyway. `log` refuses to show this one.
        assert!(
            matches!(
                from_the_seed_record(Err(missing()), true, &shadow),
                Some(AtStake::Unknown(_))
            ),
            "a repository with no record of its start is not an empty session"
        );
        // The arm that mattered.
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        match from_the_seed_record(Err(denied), true, &shadow) {
            Some(AtStake::Unknown(why)) => assert!(
                why.contains("denied"),
                "the reason reaches the user rather than being read as absence: {why}"
            ),
            other => panic!("a record omh could not read is not an empty session: {other:?}"),
        }
    }

    /// A sandbox omh cannot read is asked about, not assumed empty.
    ///
    /// Three states, each of which the first version deleted in silence. The
    /// test it replaces asserted the opposite — it took the one harmless
    /// member of the class (the repository is *gone*, so nothing is left to
    /// lose) and generalised it to the whole class, which locked the collapse
    /// in.
    #[test]
    fn a_sandbox_omh_cannot_read_is_asked_about_rather_than_assumed_empty() {
        // A truncated replay record. `landed` bails on it deliberately: the
        // write truncates before it writes, so a process killed in that window
        // leaves zero bytes — and the session has demonstrably been harvested,
        // so the repository demonstrably holds commits.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::write(&shadow.landed_record, "").unwrap();
        let err = may_remove(&paths, &session, Snapshots::None, false)
            .expect_err("omh cannot tell what landed — that is a reason to ask");
        assert!(
            err.to_string().contains("cannot say what that removes"),
            "it says it cannot tell, rather than naming a count it does not have: {err}"
        );
        assert!(
            may_remove(&paths, &session, Snapshots::None, true).is_ok(),
            "and `--force` is still the way past, so nobody is trapped"
        );

        // A repository with no record of where it started — what `reap` leaves
        // when `remove_dir_all` fails on a live mount and the seed file goes
        // anyway. `log` refuses to *show* this one; `rm` would have deleted it.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        assert!(
            may_remove(&paths, &session, Snapshots::None, false).is_err(),
            "a repository omh cannot place is not an empty session"
        );

        // Gone entirely: nothing is left to lose, and `rm` must not stand in
        // the way of clearing up.
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        std::fs::remove_dir_all(&shadow.gitdir).unwrap();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        assert!(
            may_remove(&paths, &session, Snapshots::None, false).is_ok(),
            "nothing there is nothing to lose"
        );

        // And a session whose sandbox never ran at all.
        let never_ran = Session::new(&paths.worktrees().join("s02"), "s02".to_string());
        assert!(may_remove(&paths, &never_ran, Snapshots::None, false).is_ok());
    }

    /// The count says what it counted, and says it in the singular when there
    /// is one of it.
    ///
    /// Asserted on the value rather than the sentence: `contains("1 commit")`
    /// is satisfied by `"1 commits"`, so a hardcoded plural survives every
    /// string assertion in this file.
    #[test]
    fn one_of_a_thing_is_said_in_the_singular() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let landed = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.gitdir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        std::fs::write(
            &shadow.landed_record,
            String::from_utf8_lossy(&landed.stdout).trim(),
        )
        .unwrap();

        std::fs::write(session.worktree.join("in-flight.rs"), "fn later() {}\n").unwrap();
        assert!(
            matches!(at_stake(&paths, &session), AtStake::Work(what) if what == "1 uncommitted path"),
            "one path, said once"
        );

        let sandbox_git = |args: &[&str]| {
            Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap()
        };
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "one more"]);
        assert!(
            matches!(at_stake(&paths, &session), AtStake::Work(what) if what == "1 commit"),
            "and one commit, said once"
        );
    }

    /// Naming checkpoints needs a git that can drop an already-applied
    /// commit, and says so rather than handing over a usage dump.
    ///
    /// Injected, because this machine's git *has* the option: probed inline,
    /// deleting the guard changed nothing any test could see.
    #[test]
    fn a_selection_on_a_git_that_cannot_do_it_says_which_command_still_works() {
        let (_paths, session, shadow) = a_session_with_two_checkpoints();

        let err = what_to_keep(&shadow, &session, "1", false, false, &|| Ok(false))
            .expect_err("this git cannot take a selection");
        assert!(
            err.to_string().contains("--empty"),
            "the refusal names what git is missing: {err}"
        );
        assert!(
            err.to_string().contains("--keep"),
            "and what still works without it: {err}"
        );
        assert!(
            what_to_keep(&shadow, &session, "1", false, false, &|| Ok(true)).is_ok(),
            "and a git that can, does"
        );
        // `--keep` on its own — and with `--edit` — asks nothing of git that
        // omh has not always asked, so neither may be refused for this. They
        // must not even ask: the probe forks a process, and returning `Err`
        // from it here proves it was never called.
        let never = || -> Result<bool> { panic!("`--keep` asked git a question it does not need") };
        assert!(what_to_keep(&shadow, &session, "", false, false, &never).is_ok());
        assert!(what_to_keep(&shadow, &session, "", true, true, &never).is_ok());

        // The question comes before the list is read, so a number that is also
        // wrong reports the git first — there is no point telling someone
        // which checkpoint they meant on a git that cannot take any.
        let err = what_to_keep(&shadow, &session, "9", false, false, &|| Ok(false))
            .expect_err("this git cannot take a selection");
        assert!(
            err.to_string().contains("--empty"),
            "the git is the answer, not the number: {err}"
        );

        // And *could not ask* is neither yes nor no.
        let err = what_to_keep(&shadow, &session, "1", false, false, &|| {
            Err(anyhow::anyhow!("no git on PATH"))
        })
        .expect_err("omh could not tell");
        // Through `problem`, because that is how an error reaches a person:
        // omh's sentence is the headline and git's reason is the cause under
        // it. `to_string()` alone shows only the outer half and would pass for
        // an error that had thrown git's words away.
        let printed = out::problem(&out::Palette::plain(), &err);
        assert!(
            printed.contains("no git on PATH"),
            "git's own reason reaches the user: {printed}"
        );
        assert!(
            !printed.contains("newer git"),
            "and omh does not invent a diagnosis it cannot support: {printed}"
        );
    }

    /// The host's answers come after the sandbox's, so an empty probe is still
    /// an empty probe.
    ///
    /// The host side is not a parameter, which is the real guard — as two
    /// arguments, swapping them silenced the emptiness check and passing an
    /// empty list dropped git from the report, and neither mistake could be
    /// reached by a test, because `doctor_cmd` needs a container. Now neither
    /// compiles. What is left to assert is the ordering and that the host's
    /// row is actually there.
    #[test]
    fn host_checks_never_stand_in_for_a_probe_that_did_not_run() {
        let sandbox = vec![doctor::Outcome {
            name: "rules".into(),
            ok: true,
            detail: "reads".into(),
        }];

        let err = every_check(Vec::new()).expect_err("a sandbox that ran nothing is not a pass");
        assert!(err.to_string().contains("did not run it"), "{err}");

        let both = every_check(sandbox).unwrap();
        assert_eq!(
            both.first().map(|o| o.name.as_str()),
            Some("rules"),
            "the sandbox's answers first: {both:?}"
        );
        assert!(
            both.iter().any(|o| o.name == "git on the host"),
            "and the host's are appended: {both:?}"
        );
    }

    /// What `--keep` and `--edit` mean, as a table.
    ///
    /// The headline of this change is that `--keep` alone no longer opens an
    /// editor, and nothing could prove it while `is_terminal` was consulted
    /// inline: no test process has a terminal, so every test took the same arm
    /// and a mutation that reopened the editor for every real user left the
    /// suite green. With the answer passed in, the decision is a table.
    #[test]
    fn what_keep_and_edit_mean_together() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();

        let keep = |selection: &str, edit: bool, terminal: bool| {
            what_to_keep(&shadow, &session, selection, edit, terminal, &|| Ok(true))
        };
        let _ = &paths;

        assert_eq!(
            keep("", false, true).unwrap(),
            shadow::Keep::All,
            "a bare --keep takes everything and opens nothing, terminal or not"
        );
        assert_eq!(keep("", false, false).unwrap(), shadow::Keep::All);
        assert_eq!(
            keep("", true, true).unwrap(),
            shadow::Keep::Edit,
            "--edit is what asks for the list"
        );
        assert!(
            keep("", true, false)
                .unwrap_err()
                .to_string()
                .contains("no terminal"),
            "and it needs somewhere to draw"
        );
        // Reachable at last: this refusal used to sit behind the terminal
        // check, so a script naming both was told about a terminal instead.
        assert!(
            keep("1", true, true)
                .unwrap_err()
                .to_string()
                .contains("twice"),
            "a selection and --edit name what to take twice"
        );
    }

    /// A merge cannot be picked one commit at a time, and is turned away
    /// before the fetch rather than by git afterwards.
    ///
    /// `log` renders a merge as `merge`, so `--keep 3` naming one is an
    /// ordinary thing to type. git's own answer is *"is a merge but no -m
    /// option was given"* — advice about a flag `--keep` forbids, and one that
    /// means something else entirely in omh.
    #[test]
    fn a_selection_naming_a_merge_is_refused_before_anything_moves() {
        let (paths, session, shadow) = a_session_with_two_checkpoints();
        let sandbox_git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .arg("--work-tree")
                .arg(&session.worktree)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        let seed = shadow.seed().unwrap();
        sandbox_git(&["checkout", "-q", "-b", "side", &seed]);
        std::fs::write(session.worktree.join("side.rs"), "fn side() {}\n").unwrap();
        sandbox_git(&["add", "-A", "."]);
        sandbox_git(&["commit", "-q", "--no-verify", "-m", "on the side"]);
        sandbox_git(&["checkout", "-q", "-"]);
        sandbox_git(&["merge", "-q", "--no-ff", "side", "-m", "Merge the side"]);

        let read = shadow.checkpoints(&session.worktree).unwrap();
        let merge = read
            .commits
            .iter()
            .find(|c| c.touched.is_none())
            .expect("the merge is a checkpoint");
        let err = what_to_keep(
            &shadow,
            &session,
            &merge.number.to_string(),
            false,
            false,
            &|| Ok(true),
        )
        .expect_err("a merge cannot be picked on its own");
        assert!(
            err.to_string().contains("merge") && !err.to_string().contains("-m option"),
            "refused in omh's words, not git's: {err}"
        );
        assert!(
            !paths.repo.join(".git/omh-harvest-s01-scratch").exists(),
            "and nothing was built to find that out"
        );
    }

    /// A number the session does not have is refused against the session's own
    /// list.
    ///
    /// The CLI test that claimed this could not reach it: `sb.session()`
    /// builds a worktree and no sandbox repository, so every spec — `9`, `0`,
    /// `two`, `4-2` — died identically inside `seed()`, about a record the
    /// user has never heard of. Replacing `chosen`'s body with a parser that
    /// ignored every rule left that test green.
    #[test]
    fn a_number_the_session_does_not_have_is_refused_with_the_range() {
        let (_paths, session, shadow) = a_session_with_two_checkpoints();
        for spec in ["9", "0", "two", "4-2", "1,1"] {
            let err = what_to_keep(&shadow, &session, spec, false, false, &|| Ok(true))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("1 to 2") || err.contains("twice") || err.contains("backwards"),
                "`{spec}` is refused against the session's own list: {err}"
            );
        }
    }

    /// A repository, a session, and a sandbox holding two checkpoints.
    fn a_session_with_two_checkpoints() -> (Paths, Session, shadow::Shadow) {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();
        for name in ["one", "two"] {
            std::fs::write(
                session.worktree.join(format!("{name}.rs")),
                format!("fn {name}() {{}}\n"),
            )
            .unwrap();
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", name],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
        }
        (paths, session, shadow)
    }

    /// Work the branch already has cannot be handed over twice.
    ///
    /// `omh sNN log` numbers every checkpoint, including the ones below the
    /// divider, so `--keep 1` is a reasonable thing to type about work that
    /// has already landed — and replaying it applies the same patch a second
    /// time. Refused by name rather than skipped, because silently dropping a
    /// number the user typed lands a different set than the one they asked
    /// for.
    #[test]
    fn a_selection_naming_work_the_branch_already_has_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            let out = Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();

        let mut ids = Vec::new();
        for name in ["one", "two"] {
            std::fs::write(
                session.worktree.join(format!("{name}.rs")),
                format!("fn {name}() {{}}\n"),
            )
            .unwrap();
            for args in [
                vec!["add", "-A", "."],
                vec!["commit", "-q", "--no-verify", "-m", name],
            ] {
                let out = Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.gitdir)
                    .arg("--work-tree")
                    .arg(&session.worktree)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{args:?}: {out:?}");
            }
            let head = Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.gitdir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            ids.push(String::from_utf8_lossy(&head.stdout).trim().to_string());
        }
        // The first was handed over; the second was not.
        std::fs::write(&shadow.landed_record, format!("{}\n", ids[0])).unwrap();

        let err = what_to_keep(&shadow, &session, "1", false, false, &|| Ok(true))
            .expect_err("checkpoint 1 is already on the branch");
        assert!(
            err.to_string().contains('1') && err.to_string().contains("already"),
            "the refusal names the number and says why: {err}"
        );
        assert!(
            what_to_keep(&shadow, &session, "2", false, false, &|| Ok(true)).is_ok(),
            "and the one that has not landed is still keepable"
        );
    }

    /// A session whose sandbox never ran is *nothing yet*; one whose
    /// repository is there without a record of where it started is not.
    ///
    /// `reap` leaves the second behind when `remove_dir_all` fails on a live
    /// mount and the seed file goes anyway — a repository holding every
    /// checkpoint the agent made, and no way to say where they began. Reported
    /// as an empty session, the user's next move is `rm`.
    #[test]
    fn a_sandbox_repository_with_no_record_of_its_start_is_not_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            Command::new("git")
                .current_dir(&paths.repo)
                .args(&args)
                .output()
                .unwrap();
        }
        std::fs::create_dir_all(paths.shadows()).unwrap();
        let session = Session::new(&paths.worktrees().join("s01"), "s01".to_string());
        session.ensure(&paths.repo, "main").unwrap();

        // Never launched: no repository, no record.
        let log = log_report(&paths, &session, false, &out::Ctx::plain()).unwrap();
        assert!(
            log.read.commits.is_empty(),
            "asking before the agent has run is an ordinary thing to do"
        );

        // Launched, then half-reaped.
        let shadow = shadow::Shadow::new(&paths.shadows(), "s01");
        shadow.ensure(&session.worktree, &[]).unwrap();
        std::fs::remove_file(&shadow.seed_record).unwrap();
        let err = log_report(&paths, &session, false, &out::Ctx::plain())
            .expect_err("a repository omh cannot place is not an empty session");
        assert!(
            err.to_string().contains("rm"),
            "and the refusal warns about the move that would destroy it: {err}"
        );
    }

    /// `resolved` is the wiring between the manifest and every launch, and
    /// nothing reached it: replacing its body with a pair of defaults — omh
    /// contributing no hooks, no rules sections, nothing — left the whole suite
    /// green.
    ///
    /// That is the failure `tests/cli.rs` says in its own module doc it exists
    /// to notice: a guard correct while the wiring that reaches it is missing.
    /// `container` and `doctor` each build the pair in their fixtures, so they
    /// prove a plan handles one and say nothing about whether one arrives.
    #[test]
    fn resolved_reads_the_manifest_and_this_repos_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let write = |p: std::path::PathBuf, body: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        install_bundled(&paths.base(), bundled::Shipped::Base, &out::Ctx::plain()).unwrap();
        // Which servers the catalogue declares decides whether a feature was
        // *removed* rather than merely switched off, so the fixture has to
        // declare them — in the catalogue, which is the only place a server
        // lives. Seeded into `.omh/profile/mcp.json` this asserted nothing:
        // `installed` came back empty, every feature was already `gone` by the
        // removed-server path, and the `[omh]` half below was asserting an
        // absence that was there before the setting was written.
        write(
            paths.root.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"},"memory":{"command":"omh"}}}"#,
        );

        let (own, _) = resolved(&paths).unwrap();
        assert!(
            !own.hooks.is_empty() && !own.sections.is_empty(),
            "a launch must be given what the manifest ships"
        );
        assert!(
            own.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "with every feature on, the graph hooks are what `[omh]` below removes: {:?}",
            own.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );

        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n\n[mcp.memory.env]\nOMH_TEST = \"seen\"\n",
        );
        let (off, policy) = resolved(&paths).unwrap();
        assert!(
            !off.hooks.iter().any(|h| h.name.starts_with("graph-")),
            "and `[omh]` in this repo has to reach it: {:?}",
            off.hooks.iter().map(|h| h.name).collect::<Vec<_>>()
        );
        assert!(
            off.sections.iter().any(|s| s.name == "git-rules"),
            "without taking a different feature with it"
        );
        // The other half of the same wire. `settings::resolve` produces this
        // and `render::document` applies it, both tested — and the assignment
        // between them was asserted nowhere, so deleting it left the suite
        // green and a token reached no server.
        assert_eq!(
            policy.mcp_env["memory"]["OMH_TEST"], "seen",
            "a per-repo MCP environment has to reach the plan too"
        );
        // And the half that used to hang off `Own`: switching a feature off is
        // what drops its server from the document, and it is a fact about this
        // repo rather than about what omh generates.
        assert!(
            policy.disabled_servers.contains("codegraph"),
            "the feature's server travels with the feature: {:?}",
            policy.disabled_servers
        );
    }

    // ── the provisioning resolution ─────────────────────────────────────────

    fn outcome(name: &str, ok: bool, detail: &str) -> doctor::Outcome {
        doctor::Outcome {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }

    /// A probe that reported nothing means the container never ran it, and
    /// recording that as "nothing applies" is **destructive**: `reconcile` drops
    /// every `true` it is not told about, so an empty answer would erase the
    /// resolution and, on the next launch, the repo would provision nothing.
    ///
    /// Silence is cannot-tell, and cannot-tell writes nothing at all — the same
    /// asymmetry `detect::program` and `facts::Facts` are built on, at the one
    /// point where acting on it would delete somebody's file contents.
    #[test]
    fn a_resolution_nobody_measured_is_never_recorded() {
        assert_eq!(
            fired_from(3, &[]),
            None,
            "three provides asked, none answered — the container never ran"
        );
    }

    /// A partial report is not an answer either, and here that distinction
    /// deletes from a committed file.
    ///
    /// The protocol prints one line per provide, so fewer lines than provides
    /// means the container died part-way — OOM, a torn pipe, a runtime that
    /// truncates. Accepting the prefix as the whole answer makes
    /// `stack::reconcile` drop every `true` it was not told about, and
    /// `config::write_provision` then rewrites `.omh/settings.toml` without
    /// them. A rust repo loses `rust/linker = true`, the next layer is built
    /// with no `gcc`, and `cargo test` fails at link — the exact failure this
    /// design opens by describing.
    ///
    /// The now-deleted `[toolchain]` question had to be fixed for this same
    /// defect, where it only cost a spurious question. Here it edits a file
    /// under version control, which is why the guard outlived the question.
    #[test]
    fn a_partial_report_is_not_a_resolution() {
        let truncated = [outcome("rust/toolchain", true, "applies")];
        assert_eq!(
            fired_from(2, &truncated),
            None,
            "two provides asked, one answered — the container died mid-script"
        );
    }

    /// Nothing to ask is an **answer**, not silence, and the difference decides
    /// whether a stale resolution is ever cleared.
    ///
    /// A repo that stops being a stack — `Cargo.toml` deleted, the crate moved
    /// into a subdirectory — has no candidates, so no container runs. Treating
    /// that like an unanswered probe leaves `[provision]` asserting
    /// `rust/toolchain = true` for ever: the stack layer keeps installing a
    /// toolchain nothing uses, and the committed file describes a repo that no
    /// longer exists.
    #[test]
    fn nothing_to_ask_is_an_answer_and_clears_a_stale_resolution() {
        assert_eq!(
            fired_from(0, &[]),
            Some(std::collections::BTreeSet::new()),
            "no candidates is a measured 'nothing applies', not a failure to measure"
        );
    }

    /// And a probe that *did* answer is taken at its word — only the provides
    /// that applied. A provide that could not answer is simply absent, which is
    /// the safe direction: it does not get installed, its `needs` then fails to
    /// resolve, and that is reported. Installing on a coin-flip would be silent.
    #[test]
    fn only_the_provides_that_applied_are_recorded() {
        let answered = [
            outcome("rust/toolchain", true, "applies"),
            outcome("node/pnpm", false, "1 does not apply"),
            outcome("node/bun", false, "2 could not answer"),
        ];
        assert_eq!(
            fired_from(3, &answered),
            Some(std::collections::BTreeSet::from([
                "rust/toolchain".to_string()
            ]))
        );
    }

    /// Installs run in file order, and only for provides the resolution
    /// recorded. A provide that asserts something the base image already
    /// ships — node's `runtime` — contributes no `RUN` and must not move the
    /// tag.
    #[test]
    fn installs_are_the_recorded_recipes_in_file_order() {
        let defs = stack::load_dir(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/stacks"
        )))
        .unwrap();
        let node = defs.iter().find(|d| d.name == "node").expect("node ships");

        // Everything applies, so only `install`-carrying provides may appear,
        // in the order the file gives them.
        let all: BTreeMap<String, bool> = node
            .provides
            .iter()
            .map(|p| (stack::key(&node.name, &p.name), true))
            .collect();
        let got = installs_for(&[node], &all);

        let expected: Vec<&str> = node
            .provides
            .iter()
            .filter_map(|p| p.install.as_deref())
            .collect();
        assert_eq!(got, expected, "order or filtering changed");
        assert!(
            !got.is_empty() && got.len() < node.provides.len(),
            "node must have both kinds of provide for this to prove anything: {got:?}"
        );
    }

    /// Only the recipes that fired, and the **file** decides their order.
    ///
    /// The test above records every provide, so it exercises the filter only in
    /// the case where the filter does nothing, and it draws its fixture from
    /// `stacks/node.toml`, whose recipes happen to already be in alphabetical
    /// order. Two wrong implementations pass it: one that ignores the
    /// resolution entirely, and one that sorts.
    ///
    /// Both are real failures. Ignoring the resolution installs *every* package
    /// manager into a node repo — the outcome `stacks/node.toml` opens by
    /// forbidding, since a repo with a `pnpm-lock.yaml` must not also get yarn
    /// and bun. Sorting puts `corepack enable pnpm` ahead of the node provide
    /// it needs, and the image build fails on a stack file that is correct.
    ///
    /// So the fixture is hostile on both axes at once: what was recorded is a
    /// strict subset, and file order is not sorted order.
    #[test]
    fn only_the_recorded_recipes_run_and_the_file_decides_their_order() {
        fn provide(name: &str, install: Option<&str>) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![name.into()],
                when: None,
                install: install.map(str::to_string),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "fixture".into(),
            marker: "fixture.toml".into(),
            provides: vec![
                provide("zulu", Some("install zulu")),
                provide("alpha", Some("install alpha")),
                provide("asserted", None),
                provide("mike", Some("install mike")),
            ],
        };
        // `mike` was never recorded; `asserted` applies and has no recipe.
        let resolved: BTreeMap<String, bool> =
            ["fixture/zulu", "fixture/alpha", "fixture/asserted"]
                .iter()
                .map(|k| ((*k).to_string(), true))
                .collect();

        assert_eq!(
            installs_for(&[&def], &resolved),
            vec!["install zulu", "install alpha"],
            "a provide the resolution does not name must contribute no recipe, \
             and sorted order is not file order"
        );
    }

    /// An opt-out keeps the recipe out of the image, which is the only thing an
    /// opt-out could mean.
    ///
    /// `[provision] "rust/linker" = false` is how somebody says *do not install
    /// this* — because it costs 124 MB they do not want, or because their base
    /// image already has it. `reconcile` preserves that `false` faithfully and
    /// `settings::resolve` reads it back, and there was a version where both
    /// were ceremony: the install set was built from what *fired*, so the
    /// recipe ran anyway. The file said one thing, the image was another, and
    /// `omh why` would cite the file.
    ///
    /// Kept as its own case now that `installs_for` reads only `true`, because
    /// what it guards is not the filter's spelling but the outcome: a `false`
    /// and a key nobody recorded must reach the image identically, and a
    /// future `unwrap_or(true)` would break exactly this and nothing else.
    #[test]
    fn a_provide_somebody_opted_out_of_is_not_installed() {
        fn provide(name: &str, install: &str) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![name.into()],
                when: None,
                install: Some(install.into()),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: vec![
                provide("toolchain", "install rustup"),
                provide("linker", "apt-get install -y gcc"),
            ],
        };
        let resolved = BTreeMap::from([
            ("rust/toolchain".to_string(), true),
            ("rust/linker".to_string(), false),
        ]);

        assert_eq!(
            installs_for(&[&def], &resolved),
            vec!["install rustup"],
            "the predicate said the linker applies; a person said not here, and \
             a person outranks a predicate"
        );
    }
    /// **A repo that provisions runs a different image from one that does
    /// not**, and two repos provisioning different things do not share one
    /// either.
    ///
    /// This is what the whole design comes down to, and it is the check the
    /// plan reserved for a machine with docker: same stack, same marker,
    /// different lockfiles, *different images*. The half that needs no
    /// container is the arithmetic — which tag a repo resolves to — and that is
    /// what is asserted here. Whether the image then contains a working pnpm
    /// is `omh doctor`'s question and no green test can answer it.
    ///
    /// Read through `sandbox`, the same function every launch and `init` call,
    /// because the bug this replaces was not a wrong tag: it was two places
    /// computing one.
    #[test]
    fn a_repo_that_provisions_runs_a_different_image() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.repo.join("package.json"), "{}").unwrap();
        std::fs::copy(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/stacks/node.toml")),
            paths.stacks().join("node.toml"),
        )
        .unwrap();
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();

        let with = |keys: &[&str]| {
            let mut repo = settings::RepoPolicy::default();
            for k in keys {
                repo.provision.insert((*k).to_string(), true);
            }
            sandbox(&paths, &adapter, &repo).unwrap().tag
        };

        let nothing = with(&[]);
        let pnpm = with(&["node/pnpm"]);
        let yarn = with(&["node/yarn"]);

        assert_eq!(
            nothing,
            image::tag_for(&adapter),
            "a repo that provisions nothing runs the harness image, not an \
             empty layer on top of it"
        );
        assert_ne!(pnpm, nothing, "provisioning changes the image");
        assert_ne!(
            pnpm, yarn,
            "same stack, same marker, different lockfile — and a shared image \
             would hand the yarn repo pnpm and nothing else"
        );
        assert_eq!(
            pnpm,
            with(&["node/pnpm"]),
            "and it is stable, or every launch rebuilds"
        );
    }

    /// A probe that **ran and failed** is not a measurement of nothing, and the
    /// difference is a sentence on the user's terminal.
    ///
    /// `docker run` against a missing image, a refusing daemon or a full disk
    /// exits non-zero with empty stdout. Parsed anyway that is an empty outcome
    /// list — indistinguishable from a sandbox that answered and had nothing,
    /// except that one of the two is a broken machine. Both leave every hook
    /// unsuppressed, which is the safe direction; only one of them deserves
    /// silence, and it is not this one.
    ///
    /// This is the same defect `init`'s predicate call was fixed for, in a
    /// function written afterwards — which is why the guard is a value here
    /// rather than a `println!` no test can watch vanish.
    #[test]
    fn a_probe_that_ran_and_failed_is_a_reason_not_a_measurement() {
        let failed = measured_or_reason(false, "", "Error: No such image: omh/x:abc\n");
        let Err(reason) = failed else {
            panic!("a failed container was read as a sandbox with nothing in it");
        };
        assert!(
            reason.contains("could not ask the sandbox"),
            "the reason has to say nobody was asked: {reason}"
        );
        assert!(
            reason.contains("No such image"),
            "and carry what the runtime said, or it names no cause: {reason}"
        );

        // A probe that succeeded is taken at its word, protocol and all.
        assert_eq!(
            measured_or_reason(true, "ok\tcargo\tresolves\n", "")
                .expect("a successful probe is an answer")
                .len(),
            1
        );
        // Including when it honestly measured nothing — an empty *successful*
        // report is a report, and must not be dressed as a failure.
        assert_eq!(measured_or_reason(true, "", ""), Ok(Vec::new()));
    }

    /// Stderr is carried, but not all of it.
    ///
    /// A runtime failing to pull or mount can produce a page of output, and a
    /// diagnostic that buries the line above it under its own noise is one
    /// people learn to scroll past — the same reason `init`'s predicate report
    /// takes three lines.
    #[test]
    fn the_reason_carries_a_few_lines_of_evidence_not_a_page() {
        let noisy: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let Err(reason) = measured_or_reason(false, "", &noisy) else {
            panic!("must be a reason");
        };
        assert_eq!(
            reason.lines().count(),
            4,
            "one reason and three lines of evidence: {reason}"
        );
    }

    // ── the questions of last resort ────────────────────────────────────────

    fn unclaimed(stacks: &[&str]) -> Vec<stack::Marker> {
        stacks
            .iter()
            .map(|s| stack::Marker {
                file: format!("{s}.manifest"),
                stack: (*s).to_string(),
            })
            .collect()
    }

    fn exchange(
        markers: &[stack::Marker],
        has_test: bool,
        typed: &str,
    ) -> (usize, Vec<ask::Answer>) {
        let refs: Vec<&stack::Marker> = markers.iter().collect();
        let mut out = Vec::new();
        ask_all(
            &refs,
            has_test,
            &mut std::io::BufReader::new(typed.as_bytes()),
            &mut out,
        )
        .unwrap()
    }

    /// **A decline stops the remaining marker questions**, rather than putting
    /// every one of them into the same void.
    ///
    /// A decline and a closed pipe are indistinguishable at this level, and the
    /// one that matters is the pipe: a polyglot repo with three unclaimed
    /// markers would otherwise print three questions nobody can see. One "no"
    /// is answer enough to stop asking — the scar the deleted `[toolchain]`
    /// question earned, carried over rather than re-learned.
    #[test]
    fn declining_one_question_stops_the_rest() {
        let three = unclaimed(&["elixir", "ruby", "php"]);
        let (asked, answers) = exchange(&three, true, "\n");
        assert_eq!(asked, 1, "one question put, and no more after the decline");
        assert!(answers.is_empty());

        // A closed pipe reaches the same place without a prompt being answered
        // at all.
        assert_eq!(exchange(&three, true, ""), (1, Vec::new()));
    }

    /// **A question declined is a question asked.** The headline counts what
    /// was put on screen, not what came back — claiming "asked nothing" after
    /// interrogating somebody is the promise the tagline sells, broken while
    /// they watch.
    #[test]
    fn the_count_is_what_was_put_not_what_was_answered() {
        let one = unclaimed(&["elixir"]);
        let (asked, answers) = exchange(&one, true, "\n");
        assert_eq!((asked, answers.len()), (1, 0));

        let (asked, answers) = exchange(&one, true, "apt-get install -y elixir\nmix\n");
        assert_eq!((asked, answers.len()), (1, 1));
        assert_eq!(answers[0].path, std::path::Path::new("stacks/elixir.toml"));
    }

    /// Neither question is put where nothing is unknown — which is most repos,
    /// most of the time, and is what keeps this from being a wizard.
    #[test]
    fn a_repo_with_nothing_unknown_is_asked_nothing() {
        assert_eq!(exchange(&[], true, "mix test\n"), (0, Vec::new()));
    }

    /// The test question stands alone: a repo omh understands entirely, that
    /// still has no way to check its own work.
    #[test]
    fn a_project_with_no_way_to_test_itself_is_asked_about_that_alone() {
        let (asked, answers) = exchange(&[], false, "mix test\n");
        assert_eq!(asked, 1);
        assert_eq!(answers[0].path, std::path::Path::new("hooks/test.json"));
    }

    /// **Covered means covered *here*.** A catalogue hook for an ecosystem this
    /// repo is not speaks for nothing in it.
    ///
    /// Without the intersection this answers `{rust, go, python}` in every repo
    /// — that is simply what omh ships — and `derive::hooks` reads a non-empty
    /// `covered` as *some ecosystem hook already runs this project's tests*. So
    /// runner derivation, the whole hand-rolled `Makefile`/`justfile`/`Taskfile`
    /// scanner, could not fire for anybody, and a C project with a working
    /// `make test` was then told by `omh init` that omh had found **no runner**.
    ///
    /// Every unit test of `derive::hooks` passes a hand-built `covered`, so none
    /// of them could see this: the defect was in what the caller computed, and
    /// nothing tested the caller. That is why this is a function.
    #[test]
    fn what_the_catalogue_covers_elsewhere_covers_nothing_here() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("rust-test.json"),
            r#"{"on":"turn-end","stack":"rust","run":"cargo test"}"#,
        )
        .unwrap();
        std::fs::write(
            hooks.join("shellcheck.json"),
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        )
        .unwrap();
        let dirs = [hooks];

        let rust = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: Vec::new(),
        };

        assert_eq!(
            covered_here(&dirs, &[]).unwrap(),
            BTreeSet::new(),
            "a repo that is no ecosystem omh ships a hook for is covered by \
             none of them — this is the C project with a Makefile, and the \
             whole runner path depends on it"
        );
        assert_eq!(
            covered_here(&dirs, &[&rust]).unwrap(),
            ["rust".to_string()].into_iter().collect(),
            "and a rust repo is covered, so its Makefile earns no second hook"
        );
    }

    /// A hook belonging to an ecosystem this repo is not could never have been
    /// taken here, so it is not offered and not reported as unselected.
    ///
    /// This is **applicability, not selection**, and the distinction is the
    /// whole of it. `[use]` is what you chose from what you could have chosen;
    /// once omh ships a hook per ecosystem, a rust repo's catalogue holds
    /// `go-test` and `python-format` too, and listing them as "available but
    /// not selected" would turn a real report — *here is what you are not
    /// using* — into a page of things nobody could ever use. The launcher's
    /// unselected line exists to be read, and a report nobody reads is one that
    /// stops catching the entry you did mean to take.
    ///
    /// A hook naming **no** stack belongs everywhere: `graph-refresh`, or
    /// somebody's `shellcheck`. Those are never filtered, and the asymmetry
    /// matters — filtering by "names a detected stack" rather than "does not
    /// name an undetected one" would hide every general hook in the catalogue.
    #[test]
    fn a_hook_for_an_ecosystem_this_repo_is_not_is_not_offered() {
        let declared = BTreeMap::from([
            ("rust-test".to_string(), Some("rust".to_string())),
            ("go-test".to_string(), Some("go".to_string())),
            ("shellcheck".to_string(), None),
        ]);
        let names = vec![
            "rust-test".to_string(),
            "go-test".to_string(),
            "shellcheck".to_string(),
            // A name in the list that no file declares — the repo's own hook
            // directory is read separately, and a name omh knows nothing about
            // is not a name omh may drop.
            "mine".to_string(),
        ];
        let detected: BTreeSet<String> = ["rust".to_string()].into_iter().collect();

        assert_eq!(
            applicable_hooks(names.clone(), &declared, &detected),
            vec![
                "rust-test".to_string(),
                "shellcheck".to_string(),
                "mine".to_string()
            ],
            "only the hook naming an ecosystem this repo is not comes out"
        );

        // A repo omh detects nothing for keeps everything that claims nothing.
        assert_eq!(
            applicable_hooks(names, &declared, &BTreeSet::new()),
            vec!["shellcheck".to_string(), "mine".to_string()]
        );
    }

    /// What the sandbox is asked about is the **union**, and neither half
    /// contains the other.
    ///
    /// `needs` is what a *stack* promised: it catches rustup installing a
    /// `cargo` that then cannot link, which is a provisioning failure and
    /// belongs in `init`'s report. A hook's program is what will be handed to a
    /// shell: a hand-written `shellcheck` hook is in no `needs` list anywhere,
    /// and asking only about `needs` ships it into a sandbox that cannot run
    /// it — `cargo: not found` with a different program in it.
    ///
    /// Both mutations are one line and neither is implausible, which is why
    /// this asserts the whole set rather than two `contains`.
    #[test]
    fn the_sandbox_is_asked_about_both_what_stacks_promised_and_what_hooks_run() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("lint.json"),
            r#"{"on":"turn-end","run":"shellcheck ./x.sh"}"#,
        )
        .unwrap();

        let def = stack::Definition {
            name: "rust".into(),
            marker: "Cargo.toml".into(),
            provides: vec![stack::Provide {
                name: "toolchain".into(),
                needs: vec!["cargo".into(), "rustc".into()],
                when: None,
                install: Some("install rustup".into()),
                because: "a fixture".into(),
                measured: Vec::new(),
            }],
        };
        let mut repo = settings::RepoPolicy::default();
        repo.provision.insert("rust/toolchain".to_string(), true);

        // Through `needs_of`, because that is what `sandbox` puts in `owed` —
        // asserting the union over a hand-written set would let the two halves
        // agree here and disagree in the one place it matters.
        let owed = needs_of(&[&def], &repo.provision);
        let asked = probe_targets(&[hooks], &Default::default(), &repo, &owed).unwrap();

        assert_eq!(
            asked,
            BTreeSet::from([
                "cargo".to_string(),
                "rustc".to_string(),
                "shellcheck".to_string()
            ]),
            "asking about only one of the two lists leaves the other unmeasured"
        );
    }

    /// A provide nobody recorded, and a provide somebody opted out of, owe
    /// nothing — so neither can be reported as a provisioning failure.
    ///
    /// `init` prints "did not resolve after installing" for these, and that
    /// sentence has to be true. A provide that was never installed did not
    /// fail to install; saying so is a gap omh invented, and it would print on
    /// every `init` for anybody who opted out of the 124 MB linker on purpose.
    ///
    /// The consequence of the opt-out is not silenced by this. If a hook names
    /// the program, `probe_targets` asks about it through the hooks half, and a
    /// hook that cannot run is dropped by name.
    #[test]
    fn only_what_was_provisioned_owes_a_program() {
        fn provide(name: &str, need: &str, install: Option<&str>) -> stack::Provide {
            stack::Provide {
                name: name.into(),
                needs: vec![need.into()],
                when: None,
                install: install.map(str::to_string),
                because: "a fixture".into(),
                measured: Vec::new(),
            }
        }
        let def = stack::Definition {
            name: "node".into(),
            marker: "package.json".into(),
            provides: vec![
                // No `install`: an assertion that the base image already ships
                // this, which is worth writing down only if something checks it.
                provide("runtime", "node", None),
                provide("pnpm", "pnpm", Some("corepack enable pnpm")),
                provide("yarn", "yarn", Some("corepack enable yarn")),
                provide("bun", "bun", Some("npm install -g bun")),
            ],
        };
        let resolved = BTreeMap::from([
            ("node/runtime".to_string(), true),
            ("node/pnpm".to_string(), true),
            ("node/yarn".to_string(), false),
            // `node/bun` was never recorded at all.
        ]);

        assert_eq!(
            needs_of(&[&def], &resolved),
            BTreeSet::from(["node".to_string(), "pnpm".to_string()]),
            "an assertion with no recipe is still owed; an opt-out and an \
             absence are not"
        );
    }

    /// A stacks directory omh cannot read is **said**, and it withdraws the
    /// drift report rather than filing a wrong one.
    ///
    /// `notice::hooks` decides which hooks name a stack this repo is not, so
    /// with no definitions it concludes that every stack-named hook belongs to
    /// nothing. Swallowing the error into an empty list therefore does not
    /// degrade the report — it inverts it, and prints the inversion in omh's
    /// own voice. The neighbouring branch already reports its error and returns
    /// `None`; this one used `unwrap_or_default` and said nothing at all.
    #[test]
    fn a_stacks_directory_omh_cannot_read_is_reported_rather_than_read_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.stacks().join("rust.toml"), "this is not toml {{{").unwrap();

        assert!(
            say_hooks(&paths, &out::Ctx::plain()).is_none(),
            "a report built on stacks that would not load is a wrong report"
        );
    }

    /// The resolution is read from and written to the **committed** layer, and
    /// nothing else guarded which layer that is.
    ///
    /// `config`'s own tests prove the reader answers for one layer, and prove
    /// it for a good reason — but a correct reader called with the wrong
    /// argument is the same bug with a passing guard in front of it. Both
    /// mutations survived the suite: reading `Local` exports one laptop's
    /// `false` into a file everybody clones, and writing `Local` means the
    /// resolution never reaches a teammate, which is the entire case for the
    /// table living in committed settings.
    #[test]
    fn the_resolution_is_read_and_written_in_the_committed_layer() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let write = |layer: config::Layer, body: &str| {
            let f = layer.file(&paths);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        write(
            config::Layer::Shared,
            "[provision]\n\"rust/toolchain\" = true\n",
        );
        let local_before = "[provision]\n\"node/pnpm\" = false\n";
        write(config::Layer::Local, local_before);

        let fired: BTreeSet<String> = ["rust/toolchain", "node/pnpm"]
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        record_resolution(&paths, &fired).unwrap();

        let shared = std::fs::read_to_string(config::Layer::Shared.file(&paths)).unwrap();
        let parsed: toml::Table = toml::from_str(&shared).expect("still TOML");
        assert_eq!(
            parsed["provision"]["rust/toolchain"].as_bool(),
            Some(true),
            "the resolution must land in the committed file: {shared}"
        );
        assert_eq!(
            parsed["provision"]["node/pnpm"].as_bool(),
            Some(true),
            "a laptop's opt-out must not be read back as the team's: {shared}"
        );
        assert_eq!(
            std::fs::read_to_string(config::Layer::Local.file(&paths)).unwrap(),
            local_before,
            "the local layer is somebody's own file and init does not edit it"
        );
    }

    // ── proposed guards ─────────────────────────────────────────────────────

    /// A stand-in for the container runtime: a script that records every call
    /// and answers the probe protocol.
    ///
    /// `measure` and `top_up` take the runtime's program name as an argument,
    /// so the whole measurement path is reachable without a container. What
    /// this cannot prove is what a *real* image contains — that is `omh
    /// doctor`'s, and no green test crosses that line. What it does prove is
    /// omh's own arithmetic: which questions get asked, of which image, and
    /// what is done with the answers.
    fn fake_runtime(dir: &std::path::Path, present: &[&str], absent: &[&str]) -> String {
        let log = dir.join("calls.log");
        let mut body = String::from("#!/bin/sh\n");
        body.push_str(&format!(
            "printf 'CALL %s\\n' \"$*\" | tr -d '\\n' >> {}; printf '\\n' >> {}\n",
            log.display(),
            log.display()
        ));
        for p in present {
            body.push_str(&format!("printf 'ok\\t{p}\\tresolves\\n'\n"));
        }
        for p in absent {
            body.push_str(&format!("printf 'fail\\t{p}\\tnot installed\\n'\n"));
        }
        let bin = dir.join("fake-runtime");
        std::fs::write(&bin, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin.to_string_lossy().to_string()
    }

    /// Every call the fake was given, and the probes among them — a probe is
    /// the one that runs the image rather than inspecting or building it.
    fn calls(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn probes(dir: &std::path::Path) -> Vec<String> {
        calls(dir)
            .into_iter()
            .filter(|c| c.contains("--pull=never"))
            .collect()
    }

    fn a_sandbox(tag: &str, owed: &[&str]) -> Sandbox {
        Sandbox {
            installs: Vec::new(),
            tag: tag.to_string(),
            resolves: BTreeMap::new(),
            owed: owed.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn measurement_fixture(dir: &std::path::Path) -> (Paths, Adapter) {
        let paths = Paths {
            root: dir.join("home"),
            repo: dir.join("repo"),
        };
        std::fs::create_dir_all(&paths.repo).unwrap();
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        (paths, adapter)
    }

    /// **A second launch asks the image nothing**, and that is the only reason
    /// a launch is not a container run.
    ///
    /// Everything the cache is for lives in one function nothing reached:
    /// skipping `unseen`, dropping the `save`, or throwing the answer away
    /// instead of storing it all left the suite green. The first two spend a
    /// container run on every launch of every repo forever; the third ships
    /// every hook into a sandbox that may not have its program, which is the
    /// `cargo: not found` failure this milestone exists to remove.
    #[cfg(unix)]
    #[test]
    fn a_second_launch_asks_the_image_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &["cc"]);
        let own = base::Own::default();
        let repo = settings::RepoPolicy::default();

        let mut first = a_sandbox("omh/claude:abc123", &["cargo", "cc"]);
        first
            .top_up(
                &paths,
                &runtime,
                &adapter,
                &[],
                &own,
                &repo,
                &out::Ctx::plain(),
            )
            .unwrap();

        assert_eq!(probes(dir.path()).len(), 1, "the first launch must ask");
        assert_eq!(
            first.resolves.get("cargo"),
            Some(&true),
            "and keep what it was told: {:?}",
            first.resolves
        );
        assert_eq!(first.resolves.get("cc"), Some(&false));
        assert!(
            paths.facts().exists(),
            "and write it down, or the next launch asks again"
        );

        let mut second = a_sandbox("omh/claude:abc123", &["cargo", "cc"]);
        second
            .top_up(
                &paths,
                &runtime,
                &adapter,
                &[],
                &own,
                &repo,
                &out::Ctx::plain(),
            )
            .unwrap();
        assert_eq!(
            probes(dir.path()).len(),
            1,
            "a repo whose hooks and stacks have not changed must start no container"
        );
        assert_eq!(
            second.resolves.get("cc"),
            Some(&false),
            "and still know what was measured before: {:?}",
            second.resolves
        );
    }

    /// The probe runs in **this** image and asks about what this repo owes —
    /// and the answers are filed under the same tag.
    ///
    /// Probing or filing under any other tag caches an answer about image A
    /// against image B. Both directions are silent: a hook suppressed in a
    /// sandbox that has its program, or shipped into one that does not.
    #[cfg(unix)]
    #[test]
    fn the_probe_asks_this_image_about_what_it_owes() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &[]);

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        sb.top_up(
            &paths,
            &runtime,
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        )
        .unwrap();

        let probe = probes(dir.path()).join("\n");
        assert!(
            probe.contains("omh/claude:abc123"),
            "the probe must run in the image this session will run: {probe}"
        );
        assert!(
            probe.contains("cargo"),
            "and ask about what the stacks promised: {probe}"
        );

        let raw = std::fs::read_to_string(paths.facts()).unwrap();
        assert!(
            raw.contains("omh/claude:abc123"),
            "and file the answer under that image's tag: {raw}"
        );
    }

    /// **There is an image before there is a question about it.**
    ///
    /// All three launch paths measured first and built inside `session_up`, so
    /// the first launch after a recipe changed probed a tag with no image
    /// behind it, learned nothing, and shipped every hook unsuppressed. It
    /// healed on the second launch, which is exactly the broken first turn this
    /// design removes. `top_up` now builds first, and nothing else says so.
    #[cfg(unix)]
    #[test]
    fn an_image_is_made_sure_of_before_it_is_asked_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        let runtime = fake_runtime(dir.path(), &["cargo"], &[]);

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        sb.top_up(
            &paths,
            &runtime,
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        )
        .unwrap();

        let all = calls(dir.path());
        let built = all
            .iter()
            .position(|c| !c.contains("--pull=never"))
            .expect("the image has to be made sure of at all");
        let asked = all
            .iter()
            .position(|c| c.contains("--pull=never"))
            .expect("and then asked");
        assert!(
            built < asked,
            "a probe against an image nobody built learns nothing: {all:?}"
        );
    }

    /// A runtime that cannot answer is **cannot-tell**, never a sandbox with
    /// nothing in it.
    ///
    /// Suppression acts on a measured `false`. A probe that could not run must
    /// leave the facts as they were, or one broken docker turns every hook in
    /// every repo off in a session that otherwise looks normal.
    #[cfg(unix)]
    #[test]
    fn a_probe_that_cannot_run_suppresses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, adapter) = measurement_fixture(dir.path());
        // Exits non-zero for everything, so the image "exists" is false and the
        // probe fails — the shape of a daemon that is refusing.
        let bin = dir.path().join("failing-runtime");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut sb = a_sandbox("omh/claude:abc123", &["cargo"]);
        let _ = sb.top_up(
            &paths,
            &bin.to_string_lossy(),
            &adapter,
            &[],
            &base::Own::default(),
            &settings::RepoPolicy::default(),
            &out::Ctx::plain(),
        );

        assert_eq!(
            sb.resolves.get("cargo"),
            None,
            "silence is cannot-tell, and cannot-tell is never a measured \
             absence: {:?}",
            sb.resolves
        );
    }

    /// A stack whose recipes are neither sorted nor all-applying — hostile on
    /// both axes, so a recipe that sorts and an `owed` built from the
    /// definitions rather than the resolution both come out wrong here.
    fn provisioned_fixture(paths: &Paths) {
        std::fs::create_dir_all(paths.stacks()).unwrap();
        std::fs::create_dir_all(&paths.repo).unwrap();
        std::fs::write(paths.repo.join("fixture.toml"), "").unwrap();
        std::fs::write(
            paths.stacks().join("fixture.toml"),
            r#"
name   = "fixture"
marker = "fixture.toml"

[[provide]]
name    = "zulu"
needs   = ["zulu"]
install = "install zulu"
because = "a fixture"

[[provide]]
name    = "alpha"
needs   = ["alpha"]
install = "install alpha"
because = "a fixture"

[[provide]]
name    = "declined"
needs   = ["declined"]
install = "install declined"
because = "a fixture"
"#,
        )
        .unwrap();
    }

    fn fixture_policy() -> settings::RepoPolicy {
        let mut repo = settings::RepoPolicy::default();
        repo.provision.insert("fixture/zulu".to_string(), true);
        repo.provision.insert("fixture/alpha".to_string(), true);
        repo.provision.insert("fixture/declined".to_string(), false);
        repo
    }

    /// **The recipe a launch builds must produce the tag that launch runs.**
    ///
    /// `session_up` takes the two as separate arguments — `opts.image` and
    /// `recipe` — so nothing stops them describing different images, which is
    /// the milestone-one bug in a new place. Asserted as agreement rather than
    /// against a literal, because the failure is divergence.
    #[test]
    fn the_layer_a_sandbox_names_is_the_layer_its_recipe_builds() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        provisioned_fixture(&paths);
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        let sb = sandbox(&paths, &adapter, &fixture_policy()).unwrap();

        assert_eq!(
            sb.recipe(),
            vec!["install zulu", "install alpha"],
            "file order is install order, and an opt-out contributes no recipe"
        );
        assert_ne!(
            sb.tag,
            image::tag_for(&adapter),
            "this fixture must provision something or it proves nothing"
        );
        assert_eq!(
            image::stack_tag(&adapter, &sb.recipe()),
            sb.tag,
            "the recipe handed to `ensure_stack` must build the tag `plan` runs, \
             or a session runs an image nothing built"
        );
    }

    /// What `sandbox` hands on is the **resolution's** list and the **tag's**
    /// measurements, and neither is re-derived from anything else.
    #[test]
    fn a_sandbox_carries_what_it_owes_and_what_is_already_known() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        provisioned_fixture(&paths);
        let adapter = Adapter::find(std::path::Path::new(BUNDLED_ADAPTERS), "claude").unwrap();
        let repo = fixture_policy();

        let first = sandbox(&paths, &adapter, &repo).unwrap();
        assert_eq!(
            first.owed,
            BTreeSet::from(["zulu".to_string(), "alpha".to_string()]),
            "a provide somebody opted out of was never installed and owes \
             nothing: {:?}",
            first.owed
        );
        assert!(
            first.resolves.is_empty(),
            "and nothing has been measured about this image yet"
        );

        let mut facts = facts::Facts::default();
        facts.learn(
            &first.tag,
            &[doctor::Outcome {
                name: "alpha".into(),
                ok: false,
                detail: "not installed in the sandbox".into(),
            }],
        );
        facts.save(&paths).unwrap();

        let second = sandbox(&paths, &adapter, &repo).unwrap();
        assert_eq!(
            second.resolves.get("alpha"),
            Some(&false),
            "a sandbox must arrive knowing what was measured about its own \
             tag: {:?}",
            second.resolves
        );
    }

    // ── the shipped hooks ───────────────────────────────────────────────────

    /// The safety property, restated where it now lives.
    ///
    /// It used to be `Layer::DEFAULT_WRITE`: one flag, one default, and an
    /// unqualified write could never reach version control. `--layer` split
    /// into two commands because the two scopes want opposite defaults, so the
    /// constant went — and the property it carried did not. Neither command's
    /// default may be the committed file, and reaching it has to be asked for
    /// in so many words.
    #[test]
    fn no_unqualified_write_can_reach_version_control() {
        assert!(
            !repo_layer(false).is_committed(),
            "omh repo set holds carry_in paths and MCP env"
        );
        assert!(
            !config::Layer::Personal.is_committed(),
            "omh config set writes your own file"
        );
        assert!(
            repo_layer(true).is_committed(),
            "and --shared is how you say you meant it"
        );
    }

    /// `omh <name>` treats any unknown word as a harness, so a command that is
    /// not in RESERVED could be shadowed by an adapter of the same name. This
    /// keeps the list honest without anyone remembering to update it.
    /// What `consumes_session` claims and what the dispatch does agree.
    ///
    /// The exhaustive match makes a new command a compile error until somebody
    /// classifies it. It cannot make them classify it *correctly* — and the
    /// wrong direction is silent: a command marked as reading the session that
    /// does not read it goes straight back to answering with the scope thrown
    /// away, which is the defect the predicate exists to end.
    ///
    /// So this reads both places out of the source and compares them: a
    /// variant whose dispatch arm mentions `cli.session` must be one
    /// `consumes_session` answers `true` for, and the reverse.
    ///
    /// Reading the source rather than calling the function is deliberate —
    /// building one `Cmd` of every variant to ask it would mean writing the
    /// list a third time, and the third copy is the one that rots.
    #[test]
    fn the_reads_and_the_refusals_agree_with_the_dispatch() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )
        .unwrap();

        // Every `Cmd::` in a slice of source, ignoring `MemoryCmd::` and
        // friends — a suffix match would count `MemoryCmd::Remember` as a
        // top-level `Remember`, which is how the first version of this scan
        // grew a variant that does not exist.
        fn variants(text: &str) -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            let bytes = text.as_bytes();
            for (at, _) in text.match_indices("Cmd::") {
                if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                    continue;
                }
                let name: String = text[at + 5..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric())
                    .collect();
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            out
        }

        // Anchored on the function first: `session_prefix` matches on
        // `&cli.cmd` too, and splitting on that alone reads the wrong block —
        // silently, and with an empty result that would have looked like
        // agreement without the floors below.
        let dispatch = body
            .split_once("fn dispatch(cli: &Cli, ctx: &out::Ctx) -> Result<()> {")
            .expect("dispatch is still spelled this way")
            .1
            .split_once("match &cli.cmd {")
            .expect("dispatch still matches on cli.cmd")
            .1;

        let mut reads = std::collections::BTreeSet::new();
        let mut arm: Option<(String, String)> = None;
        // An arm consumes the session if it names `cli.session`, or if it
        // hands the whole `cli` down — `Cmd::Run` does the latter and reads the
        // session inside `run`, which no scan of this block could see.
        let consuming = |text: &str| {
            text.contains("cli.session") || text.contains(", cli,") || text.contains("(cli,")
        };
        for line in dispatch.lines() {
            if line == "    }" {
                break;
            }
            if line.starts_with("        Cmd::") {
                if let Some((name, text)) = arm.take() {
                    if consuming(&text) {
                        reads.insert(name);
                    }
                }
                let name = variants(line).into_iter().next().unwrap_or_default();
                arm = Some((name, line.to_string()));
            } else if let Some((_, text)) = arm.as_mut() {
                text.push_str(line);
            }
        }
        if let Some((name, text)) = arm {
            if consuming(&text) {
                reads.insert(name);
            }
        }

        // Variants the predicate answers `true` for. Arms are read by their
        // *result*, not by position: `matches!` counts as true because one of
        // that command's verbs reads the session.
        let predicate = body
            .split_once("fn consumes_session(cmd: &Cmd) -> bool {")
            .expect("the predicate is still spelled this way")
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        let chunks: Vec<&str> = predicate.split("=>").collect();
        let mut claims = std::collections::BTreeSet::new();
        for i in 0..chunks.len().saturating_sub(1) {
            let result = chunks[i + 1].trim_start();
            if !(result.starts_with("true") || result.starts_with("matches!")) {
                continue;
            }
            // For any arm but the first, drop the previous arm's result before
            // reading the pattern.
            let pattern = if i == 0 {
                chunks[i]
            } else {
                chunks[i]
                    .split_once(',')
                    .map(|(_, rest)| rest)
                    .unwrap_or("")
            };
            claims.extend(variants(pattern));
        }

        assert!(
            !reads.is_empty(),
            "the scan found no dispatch arm reading the session, so its \
             agreement means nothing"
        );
        assert!(
            !claims.is_empty(),
            "the scan could not read `consumes_session`, so its agreement \
             means nothing"
        );
        assert_eq!(
            reads, claims,
            "these disagree about which commands act on a session.\n  \
             dispatch: {reads:?}\n  consumes_session: {claims:?}"
        );
    }

    #[test]
    fn reserved_lists_every_command_and_alias() {
        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            assert!(
                RESERVED.contains(&name),
                "command `{name}` missing from RESERVED"
            );
            for alias in sub.get_visible_aliases() {
                assert!(
                    RESERVED.contains(&alias),
                    "alias `{alias}` missing from RESERVED"
                );
            }
        }
    }

    #[test]
    fn no_bundled_definition_shadows_a_command() {
        for a in Adapter::load_dir(std::path::Path::new(BUNDLED_ADAPTERS)).unwrap() {
            assert!(
                !RESERVED.contains(&a.name.as_str()),
                "adapter `{}` is a command",
                a.name
            );
        }
        for e in editor::Editor::load_dir(std::path::Path::new(BUNDLED_EDITORS)).unwrap() {
            assert!(
                !RESERVED.contains(&e.name.as_str()),
                "editor `{}` is a command",
                e.name
            );
        }
    }

    /// The grammar splits harnesses from editors, so the one mistake everybody
    /// will make is typing an editor where a harness goes. Say the fix.
    #[test]
    fn naming_an_editor_where_a_harness_goes_names_the_fix() {
        let hint = tool_hint("zed", &["claude".into()], &["zed".into()]);
        assert!(hint.contains("omh attach zed"), "got: {hint}");
    }

    #[test]
    fn an_unknown_word_lists_the_harnesses() {
        let hint = tool_hint(
            "emacs",
            &["claude".into(), "opencode".into()],
            &["zed".into()],
        );
        assert!(
            hint.contains("claude") && hint.contains("opencode"),
            "got: {hint}"
        );
        assert!(!hint.contains("attach"), "not an editor: {hint}");
    }

    #[test]
    fn a_command_typed_as_a_harness_points_at_its_help() {
        let hint = tool_hint("config", &["claude".into()], &[]);
        assert!(hint.contains("omh config --help"), "got: {hint}");
    }

    /// Regression: bundled definitions were written only if absent, so a fix
    /// omh shipped never reached anyone who had already run `init`. The one
    /// that mattered was a wrong credential path, which made auth silently
    /// capture nothing.
    #[test]
    fn bundled_definitions_are_refreshed_not_just_seeded() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("claude.toml"), "name = \"stale\"\n").unwrap();

        install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        let shipped =
            std::fs::read_to_string(std::path::Path::new(BUNDLED_ADAPTERS).join("claude.toml"))
                .unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("claude.toml")).unwrap(),
            shipped
        );
    }

    /// The refresh above is only acceptable because the old bytes survive it,
    /// and nothing asserted that they did — deleting the backup entirely kept
    /// the suite green.
    #[test]
    fn the_file_it_replaces_is_kept_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let mine = "name = \"mine, edited\"\n";
        std::fs::write(dest.join("claude.toml"), mine).unwrap();

        install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("claude.toml.yours")).unwrap(),
            mine,
            "the replaced file must be recoverable byte for byte"
        );
    }

    /// **And it is recoverable under a name that exists**, for every kind omh
    /// ships rather than only the TOML ones.
    ///
    /// The backup was named by *replacing* the extension with a literal
    /// `toml.yours`, which was right by accident while everything shipped was
    /// TOML and wrong the moment `hooks/` shipped JSON: an edited
    /// `rust-test.json` was saved as `rust-test.toml.yours` while the message
    /// on screen said `rust-test.json.yours`. Somebody looking for their edit
    /// where omh told them to look would not find it, and would reasonably
    /// conclude it had been discarded.
    ///
    /// Iterated over every shipped kind, because a guard written against
    /// adapters alone is a guard that passes on exactly the case that broke.
    #[test]
    fn a_replaced_file_is_kept_under_the_name_omh_names() {
        for kind in bundled::ALL {
            let d = tempfile::tempdir().unwrap();
            let dest = d.path().join(kind.dir());
            std::fs::create_dir_all(&dest).unwrap();
            let first = kind.files()[0].name;
            let mine = "this is what I wrote\n";
            std::fs::write(dest.join(first), mine).unwrap();

            install_bundled(&dest, kind, &out::Ctx::plain()).unwrap();

            // The name omh prints, spelled the way omh prints it.
            let backup = dest.join(format!("{first}.yours"));
            assert_eq!(
                std::fs::read_to_string(&backup).ok().as_deref(),
                Some(mine),
                "{}: an edit must be recoverable at {}",
                kind.dir(),
                backup.display()
            );
        }
    }

    /// A file omh cannot read as text is still a file somebody wrote.
    ///
    /// `read_to_string` fails on a single non-UTF-8 byte — one accented
    /// character pasted into a description is enough — and collapsing that to
    /// "absent" meant the overwrite went ahead with no backup and no message.
    /// The read failed, the write succeeded, and the edit was gone.
    #[test]
    fn an_edit_omh_cannot_read_as_text_is_still_backed_up() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let mine = b"name = \"caf\xe9\"\n"; // latin-1 é: valid file, invalid UTF-8
        std::fs::write(dest.join("claude.toml"), mine).unwrap();

        install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();

        assert_eq!(
            std::fs::read(dest.join("claude.toml.yours")).unwrap(),
            mine,
            "bytes omh cannot decode are still bytes it must not discard"
        );
    }

    /// Definitions you add yourself are yours; omh only manages its own.
    #[test]
    fn definitions_omh_does_not_ship_are_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("mine.toml"), "name = \"mine\"\n").unwrap();

        install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("mine.toml")).unwrap(),
            "name = \"mine\"\n"
        );
    }

    /// Aliases only earn their keep if they are actually short.
    #[test]
    fn every_alias_is_a_single_letter() {
        for sub in Cli::command().get_subcommands() {
            for alias in sub.get_visible_aliases() {
                assert_eq!(alias.chars().count(), 1, "`{alias}` is not a shortcut");
            }
        }
    }

    // ── omh's flags versus the harness's ────────────────────────────────────

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// `omh opencode --dry-run` launched for real. Everything after the harness
    /// name is the harness's argv, so omh's own flag went to opencode and omh
    /// never saw it. Found by hand while investigating something else — and a
    /// flag whose entire meaning is "change nothing" is the worst one to
    /// swallow quietly.
    #[test]
    fn omhs_own_flag_after_the_harness_name_is_refused() {
        let err = passthrough(&argv(&["opencode", "--dry-run"]), &omh_globals()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--dry-run"), "name the flag: {msg}");
        assert!(
            msg.contains("omh --dry-run opencode"),
            "and show the form that works: {msg}"
        );
    }

    #[test]
    fn a_flag_the_harness_owns_passes_through_untouched() {
        let given = argv(&["claude", "--resume", "x"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// Short flags are deliberately left alone. `-s` is omh's session flag and
    /// is also a flag plenty of harnesses have; refusing it would break
    /// launches that work today to guard a mistake nobody has made. The long
    /// forms are the ones worth protecting — they are unlikely to collide and
    /// they are what people actually type.
    #[test]
    fn short_flags_belong_to_the_harness() {
        let given = argv(&["claude", "-s", "something"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// The escape hatch, for the day a harness really does have `--new`.
    /// Consumed on the way through, the way every tool that offers `--` does.
    #[test]
    fn a_double_dash_hands_the_rest_to_the_harness() {
        let out = passthrough(&argv(&["claude", "--", "--dry-run"]), &omh_globals()).unwrap();
        assert_eq!(out, argv(&["claude", "--dry-run"]));
    }

    /// The harness's own name is never a flag, and a session id that happens to
    /// look like one is not omh's business either.
    #[test]
    fn only_the_arguments_are_inspected_not_the_harness_name() {
        let given = argv(&["--dry-run"]);
        assert_eq!(passthrough(&given, &omh_globals()).unwrap(), given);
    }

    /// **Nothing in this file writes to a stream directly.**
    ///
    /// The reason `out::Ctx` exists at all: 197 `println!`s here meant the
    /// wording could not be tested, the same fact was phrased two ways in two
    /// commands, and `--json` had nowhere to hook in. All of them now go
    /// through `Ctx`, and this is what stops the 198th being added — the pull
    /// is real, because a bare `println!` is one line and a report type is
    /// twenty.
    ///
    /// Two exemptions, both named:
    ///
    /// - `main` itself renders the error and must write it without a `Ctx`
    ///   method, because a `Ctx` method is what it would be reporting about.
    /// - `memory_serve` speaks MCP on stdout. What it writes is protocol, not
    ///   a report, and one report-shaped line would break the handshake.
    ///
    /// Read off the source rather than enforced by the type system, which
    /// cannot express "no macro calls here". A grep in a test is cruder than a
    /// lint and catches the same mistake at the same moment.
    #[test]
    fn no_command_writes_to_a_stream_behind_the_output_layer() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
            .expect("this file is readable from its own test");

        let offenders: Vec<(usize, &str)> = source
            .lines()
            .enumerate()
            .map(|(i, line)| (i + 1, line.trim()))
            // Only calls, never the word in a doc comment or a string that
            // talks *about* the rule — this very comment mentions `println!`.
            .filter(|(_, line)| {
                ["println!", "print!(", "eprintln!", "eprint!("]
                    .iter()
                    .any(|m| line.starts_with(m))
            })
            .collect();

        assert_eq!(
            offenders.len(),
            1,
            "every write goes through out::Ctx but the error sink in `main` — found {offenders:#?}"
        );
        assert!(
            offenders[0].1.contains("out::problem"),
            "and the one exemption is the error renderer, not something new — got {:?}",
            offenders[0]
        );
    }

    /// Derived from the parser rather than typed out, so a global added later
    /// inherits the guard instead of quietly falling outside it — the same
    /// reason `RESERVED` is checked against the subcommand list.
    #[test]
    fn every_global_flag_is_covered_without_anyone_listing_them() {
        let globals = omh_globals();
        let declared: Vec<String> = Cli::command()
            .get_arguments()
            .filter(|a| a.is_global_set())
            .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
            .collect();
        assert!(!declared.is_empty(), "the parser must have globals at all");
        for flag in declared {
            assert!(globals.contains(&flag), "{flag} is not guarded");
            assert!(
                passthrough(&argv(&["claude", &flag]), &globals).is_err(),
                "{flag} reaches the harness"
            );
        }
    }

    // ── candidate guards (mutation testing) ─────────────────────────────────

    /// **Only "not found" means absent.** A bundled file omh cannot open — a
    /// permission it does not have, a name that is now a directory — is not a
    /// file that is not there, and collapsing the two overwrites somebody's
    /// edit with no backup and no message. The read fails, the write succeeds,
    /// and the edit is gone.
    ///
    /// The non-UTF-8 half of this rule is guarded one test up. This is the
    /// other half, and deleting it changed no test.
    #[test]
    #[cfg(unix)]
    fn an_edit_omh_cannot_open_at_all_is_never_treated_as_absent() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        let target = dest.join("claude.toml");
        std::fs::write(&target, "name = \"mine\"\n").unwrap();
        // Write-only, deliberately. A mode omh can neither read nor write
        // would be caught by the *write* failing, which proves nothing about
        // the read — this is the shape the shipped bug actually had: the read
        // failed, the write succeeded, and the edit was gone.
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o200)).unwrap();

        let outcome = install_bundled(&dest, bundled::Shipped::Adapters, &out::Ctx::plain());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

        let e = format!(
            "{:#}",
            outcome.expect_err("a file omh could not read must not be overwritten in silence")
        );
        assert!(e.contains("claude.toml"), "and it names the file: {e}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "name = \"mine\"\n",
            "and the edit is still there"
        );
    }

    /// **A hook belonging to an ecosystem this repo is not stays out of the
    /// list; everything else stays in.** Both halves, because the filter has
    /// two ways to be wrong and one of them offers a rust repo `go-test` while
    /// the other drops every hook that belongs to nothing in particular —
    /// which is most of them.
    #[test]
    fn a_hook_that_belongs_to_nothing_is_offered_everywhere() {
        let declared = BTreeMap::from([
            ("rust-test".to_string(), Some("rust".to_string())),
            ("go-test".to_string(), Some("go".to_string())),
            ("graph-refresh".to_string(), None),
        ]);
        let names = vec![
            "rust-test".to_string(),
            "go-test".to_string(),
            "graph-refresh".to_string(),
            "never-declared".to_string(),
        ];
        let rust: BTreeSet<String> = ["rust".to_string()].into_iter().collect();

        assert_eq!(
            applicable_hooks(names, &declared, &rust),
            vec![
                "rust-test".to_string(),
                "graph-refresh".to_string(),
                "never-declared".to_string()
            ],
            "an ecosystem hook is filtered by the ecosystem; nothing else is"
        );
    }
}
