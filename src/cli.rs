//! The shape of a command line, and nothing about what a command does.
//!
//! Three shorthands compose here and are the whole of the grammar:
//! `sessions` is `s`, a leading session id replaces `-s`, and every session
//! verb takes one. So `omh sessions -s s01 diff`, `omh s -s s01 diff` and
//! `omh s01 diff` are one line spelled three ways.
//!
//! `RETIRED` is the other half of that: a spelling omh used to accept is
//! refused **by name**, with its replacement, rather than being quietly
//! unrecognised. Nothing retired parses.

use crate::{auth, memory, out};
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "omh", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Print the launch plan instead of running it.
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,

    /// Reuse an existing session instead of creating a new one.
    #[arg(long, short, global = true)]
    pub(crate) session: Option<String>,

    // Global, and under `omh new` that is the whole rule: before `--` it is
    // omh's, after `--` it is the harness's. The bare-name form had to guess,
    // and `passthrough` did the guessing — it refused omh's long flags and
    // left shorts alone, a judgement about which mistake was likelier. With a
    // separator there is nothing to judge.
    //
    // Deliberately *not* a doc comment: clap prints those, and the reader of
    // `--help` is not the reader of this paragraph.
    /// Report as JSON, for a script rather than a person.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// When to colour the output.
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto")]
    pub(crate) color: out::Color,

    #[command(subcommand)]
    pub(crate) cmd: Cmd,
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
    pub(crate) fn output(&self) -> (out::Format, out::Palette) {
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
/// session and the command runs where it lives — `omh s01 attach zed`,
/// `omh s01 doctor`. It used to cover the launch too, back when `sessions` had
/// no verb for starting a harness; `resume` is that verb now.
///
/// Naming it is not the same as it being honoured. `dispatch` refuses a
/// command that does not act on a session, so `omh s01 why …` and
/// `omh s01 new claude` are errors rather than launches: the prefix always
/// means *this one*, and a command that cannot mean that says so.
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
/// only when it does not parse and the line as written does — a top-level
/// command the prefix scopes. When neither parses, the sessions error is the one worth showing —
/// bare `omh s01` becomes `omh s`, whose error names the verbs.
///
/// The pattern matters more than the list. `s\d+` is what `next_id` generates,
/// so it always matches a real session; `validate_id` refuses an id spelled
/// like a command, so an id that reaches here is one this can safely lift.
/// `--session` stays for a name that is not `sNN` — and because both can be
/// given, `main` refuses a line that names the session twice rather than
/// choosing between them.
pub(crate) fn session_prefix(argv: Vec<String>) -> (Option<String>, Vec<String>) {
    // Where the id sits, once the globals in front of it are stepped over.
    //
    // This was `argv.get(1)`, so the id had to be the literal first word — and
    // every global is declared `global = true`, so clap takes them anywhere.
    // `omh --json s01 log`, which is exactly what a script wants, got clap's
    // `unrecognized subcommand 's01'` and a tip pointing at `s`, which is a
    // different command. Same reading `retired` needs one function over, and
    // the same helper: three of the globals take a value, and that value is
    // not a verb.
    let words: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let Some(at) = verb_position(&words).map(|i| i + 1) else {
        return (None, argv);
    };
    let first = &argv[at];
    let looks_like_a_session =
        first.len() > 1 && first.starts_with('s') && first[1..].chars().all(|c| c.is_ascii_digit());
    if !looks_like_a_session {
        return (None, argv);
    }

    // The flags in front of the id keep their place. They are omh's either
    // way, and moving them would change what `--` means to the harness.
    let as_written: Vec<String> = argv
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, w)| w.clone())
        .collect();
    let mut through_sessions = as_written.clone();
    through_sessions.insert(at, "s".to_string());

    // Which reading is meant, decided by what each one parses to rather than by
    // whether it parses at all. `omh s01 commit --keep 1,3` proved the weaker
    // rule wrong when `--keep` was still a flag with no value: the sessions
    // reading was refused by clap and the line as written parsed fine, as a
    // request to launch a harness called `commit`. #56 gave `--keep` a value,
    // so that line now parses both ways — but the rule it established stands
    // for every line that still does not, `omh s01 commit --whatever` among
    // them. That check is gone with the catch-all: a word that is not a
    // command no longer parses, so a mistyped verb cannot become anything.
    let as_typed = match (
        Cli::try_parse_from(&through_sessions),
        Cli::try_parse_from(&as_written),
    ) {
        (Ok(_), _) => false,
        // The line as written, when the sessions reading does not parse. This
        // used to need a check — `Cmd::Run` swallowed *any* word, so the
        // as-written reading always parsed, and a mistyped verb would have
        // become a launch of a harness by that name. With no catch-all it
        // parses only when it names a real command, and `consumes_session`
        // refuses the ones that cannot honour the prefix.
        (Err(_), Ok(_)) => true,
        // Neither reads: the sessions error is the useful one, since a
        // leading `sNN` says the sessions grammar is what was meant. (Bare
        // `omh s01` was this arm's worked example until `omh s` alone became
        // the listing; it now parses, and is decided above.)
        (Err(_), Err(_)) => false,
    };
    (
        Some(first.clone()),
        if as_typed {
            as_written
        } else {
            through_sessions
        },
    )
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
pub(crate) fn the_one_session(
    prefix: Option<String>,
    flag: Option<String>,
) -> Result<Option<String>> {
    if let (Some(prefix), Some(flag)) = (&prefix, &flag) {
        anyhow::bail!(
            "this names the session twice — `{prefix}` and `{flag}`. Name it once:\n  omh {prefix} …"
        );
    }
    Ok(flag.or(prefix))
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Set this repo up. Decides everything; asks nothing.
    Init,
    /// Verify a harness actually sees the profile, inside a real sandbox.
    #[command(visible_alias = "d")]
    Doctor {
        /// Which harness to verify. Without it, the one your host says you use.
        #[arg(long)]
        harness: Option<String>,
    },
    /// Who put this here, and on what grounds.
    Why {
        /// A base-set entry, something you added, or something omh rejected.
        thing: String,
    },
    /// Open the code graph in your browser.
    ///
    /// Not per session, and `omh s01 graph` is refused rather than
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
        /// Which harness to log in: `claude`, `opencode`, `omp`.
        harness: String,
        /// Account name, e.g. `personal` or `work`.
        #[arg(long = "name", short = 'n', default_value = auth::DEFAULT_ACCOUNT)]
        account: String,
    },
    /// What you have here: harnesses, editors, sessions, your catalogue.
    Info {
        /// This checkout instead: what it resolved, and which file decided it.
        ///
        /// A flag rather than a command because it is the same question at a
        /// different scope — `omh info` is the machine, `omh info --repo` is
        /// the checkout. It was its own command until 0.7.0.
        #[arg(long)]
        repo: bool,
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
    /// Your defaults — what a repo starts from, and how to change them.
    ///
    /// Not `omh set`, which is this checkout. The two are one letter apart
    /// with opposite scopes, and clap is deliberately not told to accept
    /// unambiguous prefixes: `omh setting` is refused rather than guessed at.
    Settings {
        #[command(subcommand)]
        cmd: Option<SettingsCmd>,
    },
    /// Select a catalogue entry for this repo.
    ///
    /// Writes the committed file: what a project uses is a fact about the
    /// project, and a teammate cloning it should get the same one.
    Use {
        /// One of rules, skills, mcp, commands, subagents, hooks.
        capability: Option<String>,
        /// Which entry. Without it, everything the catalogue has of that kind.
        name: Option<String>,
        /// Resync every list to the whole catalogue.
        #[arg(long)]
        all: bool,
    },
    /// Stop using a catalogue entry here.
    Unuse {
        /// One of rules, skills, mcp, commands, subagents, hooks.
        capability: String,
        /// Which entry to drop from this repo's list.
        name: String,
    },
    /// Change a setting, or switch one of omh's features.
    ///
    /// Which file it lands in follows from the key, not from a flag: every
    /// repo file that already holds it, or — if none does — the committed one,
    /// except a key that can name a credential, which is kept out of git.
    Set {
        /// Which setting, or one of omh's features. `omh why <name>` says what
        /// omh reads it for.
        key: String,
        /// What to set it to. A feature takes `on` or `off`.
        value: String,
        /// Write the committed file, whatever the key is.
        #[arg(long, conflicts_with = "local")]
        save: bool,
        /// Write the gitignored file, whatever the key is.
        #[arg(long)]
        local: bool,
    },
    /// Drop a setting, or hand one of omh's features back to its default.
    ///
    /// Reaches every repo file that holds it, because *where would `set` put
    /// this* and *where is this set* are different questions.
    Unset {
        /// Which setting to drop, or which feature to stop switching.
        key: String,
        /// Drop it from the committed file only.
        #[arg(long, conflicts_with = "local")]
        save: bool,
        /// Drop it from the gitignored file only.
        #[arg(long)]
        local: bool,
    },
    /// The note store: what is in it, and what is wrong with it.
    Memory {
        #[command(subcommand)]
        cmd: Option<MemoryCmd>,
    },
    /// Write out this repo's config for a harness, and step aside.
    ///
    /// The exit. Everything omh renders on a launch, written as ordinary
    /// files a harness reads without a container — so leaving omh is a
    /// command rather than an afternoon of reconstruction, and adopting it is
    /// a default rather than a cage.
    Eject {
        /// Which harness's shapes to render into.
        harness: String,
        /// Where to write. Deliberately required and deliberately not the
        /// checkout: eject exists to *show* you the files, and writing them
        /// over a working tree by default is how a command meant to reassure
        /// people becomes one they are afraid of.
        #[arg(long)]
        to: std::path::PathBuf,
    },

    /// Bring a setup you already have into omh.
    Import {
        /// What to bring over: `hooks`, `skills`, `mcp`, `rules`.
        capability: String,
        /// Which installed harness to read it out of.
        harness: String,
        /// Read this instead of where the adapter says the harness keeps it —
        /// for a config somewhere else, and for seeing what omh would do
        /// without pointing it at your own.
        #[arg(long)]
        from: Option<std::path::PathBuf>,
    },
    // The verb for what `--new` did. A flag is the wrong shape for the
    // commonest thing a person does here, and a *global* flag doubly so: it
    // can be typed anywhere omh's own flags are taken, including after a
    // session prefix that contradicts it — `omh s01 --new claude` resumes
    // `s01` and drops the flag without a word, because the prefix lands in
    // `cli.session` after clap has already checked `conflicts_with`.
    //
    // `omh new` cannot be told that: a named session is refused outright. It
    // takes its arguments after a `--`: before it is omh's, after it is the
    // harness's, and nothing has to be inferred. The bare-name form had no
    // separator and so had to guess, which is what `passthrough` was for.
    //
    // Deliberately *not* a doc comment: clap prints those, and the reader of
    // `--help` is not the reader of this paragraph.
    /// Start a session and run a harness in it.
    New {
        /// The harness to start: `claude`, `opencode`, `omp`.
        harness: String,
        /// Arguments for the harness, after a `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum McpCmd {
    /// Servers, with the layer each comes from.
    Ls,
    /// Add a server to your catalogue.
    Add {
        /// What to call it. This is the name harnesses will see.
        name: String,
        /// The program to run, e.g. `npx`.
        command: String,
        /// Everything after the command, passed to it unchanged.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
    },
    /// Remove a server from your catalogue.
    Rm {
        /// Which server, as `omh settings mcp ls` names it.
        name: String,
    },
    /// Import servers you already configured in an installed harness.
    Import {
        /// Which installed harness to read servers out of.
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
/// outright, though not for the reason first recorded here. That reason was
/// that deleting it left the line *unrefusable*: the sessions reading failed,
/// the as-written reading was a live top-level `ls`, and `session_prefix`
/// handed the launch to a command that never reads `cli.session` — so the
/// scope vanished and every session was listed, looking like it had listed
/// one. Both halves of that have since gone. `consumes_session` refuses a
/// prefix nothing consumes, and the top-level verb was renamed, so the
/// as-written reading no longer parses either and the `(Err, Err)` arm keeps
/// the sessions grammar.
///
/// What is left is smaller and still worth the variant: clap would say
/// *unrecognized subcommand*, and this says which word replaced it. A better
/// sentence, not a prevented harm — and the spelling everyone still has
/// in their fingers gets somewhere to point.
#[derive(Subcommand)]
pub(crate) enum SessionsCmd {
    // Deleting the bare-name slot took `omh s01 claude` with it, and that was
    // the only way to say *run this harness in this session*. `resume` with a
    // name is that sentence: without one it reads the record, with one it
    // overrides and rewrites it. The refusal for a session with no record
    // points here, so the remedy has to be spellable.
    /// Open a session in an editor, over SSH.
    ///
    /// A session verb because a session is what it opens — and it already read
    /// the scope before it was spelled like one.
    #[command(visible_alias = "a")]
    Attach {
        /// Defaults to $OMH_EDITOR or $EDITOR.
        editor: Option<String>,
    },
    /// Rejoin a session, running the harness it ran before.
    Resume {
        /// Rejoin as this harness instead, and record it.
        harness: Option<String>,
        /// Arguments for the harness, after a `--`.
        #[arg(last = true)]
        args: Vec<String>,
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
        /// refuses when a session is named two ways at once.
        #[arg(long, conflicts_with = "checkpoint")]
        base: Option<String>,
        /// The patch itself, through your pager, rather than a summary.
        #[arg(long, short = 'p')]
        patch: bool,
    },
    /// Commit a session's work onto its branch.
    ///
    /// Run on the host: the sandbox's git is its own repository and cannot
    /// reach yours, and the worktree omh keeps out of your way is not
    /// somewhere you should have to go.
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
    // Last, because a command list is read top to bottom and this is the
    // one that deletes a container and a worktree. It was first, above
    // `attach`, for no reason anybody wrote down.
    /// Remove a session — its container and its worktree. A branch holding
    /// commits is kept.
    Rm {
        /// Remove it even though the sandbox holds work no branch has — or
        /// omh could not tell whether it does.
        ///
        /// The question without this is the whole point: those commits and
        /// those edits exist nowhere else, and `rm` is what deletes the
        /// repository holding them. This says *I know, and I want them gone*.
        ///
        /// **It does not make omh try harder to remove anything.** On a
        /// terminal omh asks instead of refusing, so this exists for the runs
        /// where there is nobody to ask — a script, a CI job, a closed pipe.
        /// A user whose worktree would not go once read this flag as "force
        /// the removal" and typed it to no effect; `git worktree remove
        /// --force` is passed either way.
        #[arg(long)]
        force: bool,
    },
}

/// Your side of the split: `omh settings` means **you**, and `omh info --repo`
/// means **this checkout**.
///
/// One flag carried both once, and it strained because the two want opposite
/// defaults — what a project *uses* is a fact about the project and should be
/// committed, while what a project *overrides* holds `carry_in` paths and MCP
/// env and must not be committable by accident. Two commands, so neither has
/// to be told which it meant.
///
/// The four verbs are the ones that act on `~/.omh`: two that write a default,
/// one that opens the file, and the MCP catalogue. Listing the catalogue is
/// not among them — that is `omh info`, which is where the machine is
/// described.
#[derive(Subcommand)]
pub(crate) enum SettingsCmd {
    /// Set one of your defaults, for every project you start after this.
    Set {
        /// Which setting. `omh why <key>` says what omh reads it for.
        key: String,
        /// What to set it to.
        value: String,
    },
    /// Drop one of your defaults, letting omh's own take over again.
    Unset {
        /// Which setting to drop.
        key: String,
    },
    /// Open your defaults, or one catalogue entry, in $EDITOR.
    Edit {
        /// One of rules, skills, mcp, commands, subagents, hooks. Without it,
        /// your defaults file.
        capability: Option<String>,
        /// Which entry. Without it, the capability's directory.
        name: Option<String>,
    },
    /// MCP servers — your catalogue's, and what they run with.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
}

/// Deliberately short. `promote` and `stale` arrive with the layers and the
/// expiry events they act on; a subcommand that prints "not implemented" is
/// worse than its absence, because `--help` advertises it.
#[derive(Subcommand)]
pub(crate) enum MemoryCmd {
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
        /// Which note, as `omh memory lint` and the recall output name it.
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
/// Spellings this release retired, and the sentence naming the replacement.
///
/// **Data, not variants.** These were `Cmd::Attach` and `SessionsCmd::Ls`:
/// members of a command enum whose only job was to make a dead spelling
/// *parse*. That is worse than verbose, because
/// `the_lines_omh_prints_are_lines_omh_accepts` decides staleness **by
/// parsing** — so a tombstone is a hole cut in the guard exactly the size of
/// the spelling it commemorates. It cost us one: `ssh.rs` wrote the retired
/// spelling into the user's `~/.ssh/config.d/` on every attach, and the guard
/// stayed green — because the tombstone made that line parse.
///
/// A table also covers what a variant could not: the retired *alias*, any
/// number of arguments after it, and `--session` on the old spelling — all of
/// which reached clap's complaint or the wrong refusal instead of the sentence.
///
/// Consulted **after** clap refuses the line, never before, so the parser stays
/// the only thing deciding what a command is. A refusing catch-all would make
/// every line parse again, which is the defect `session_prefix` had with
/// `Cmd::Run` and this release deleted.
pub(crate) const RETIRED: &[Retired] = &[
    Retired {
        spellings: &["attach", "a"], // types the retired verb on purpose
        at: At::Verb,
        said: "`attach` is a session verb now:\n  omh s attach [editor]     the session omh picks\n  omh s01 attach zed        that one",
    },
    // Also `omh info repo`, which was reaching the right sentence only because
    // the match used to be unscoped. Stated rather than relied on.
    Retired {
        spellings: &["repo"], // types the retired verb on purpose
        at: At::Under(&["info"]),
        said: "the report is a flag now:\n  omh info --repo",
    },
    Retired {
        spellings: &["repo"], // types the retired verb on purpose
        at: At::Verb,
        said: "`repo` is gone — the report is `omh info --repo` now:\n  omh info --repo           what this checkout resolved\n  omh set <key> <value>     change it\n  omh set <feature> on|off  switch one of omh's features",
    },
    Retired {
        spellings: &["config", "c"], // types the retired verb on purpose
        at: At::Verb,
        said: "`config` is gone — it is `omh settings` now:\n  omh settings              your defaults\n  omh settings set <key> <value>\n  omh settings edit         $EDITOR on them\n  omh settings mcp ls       your MCP servers",
    },
    Retired {
        spellings: &["ls"], // types the retired verb on purpose
        // Only under `sessions`. Top-level `ls` became `omh info`, and that
        // rename deliberately kept no sentence at all — `the_inventory_answers_
        // to_info_and_not_to_the_verb_it_replaced` asserts clap's own refusal.
        // Unscoped, the table would answer the top-level spelling with the
        // *sessions* replacement — a confident wrong answer where there had
        // been an honest plain one.
        at: At::Under(&["s", "sessions"]),
        said: "there is no `ls` verb any more:\n  omh s      is the listing\n  omh s01    is one row of it",
    },
];

/// One retired spelling, and where it was retired *from*.
pub(crate) struct Retired {
    spellings: &'static [&'static str],
    at: At,
    said: &'static str,
}

/// Where a retired spelling has to appear before it is the one being asked
/// about.
///
/// It was `after: &[]` meaning *anywhere in the line*, which is a different
/// claim from *the verb* — and with spellings as short as `a` and `c` in the
/// table, anywhere is almost always somewhere else. `omh s c` answered about
/// `config` when the word was a session id; `omh s attach --nope` answered
/// about a retired spelling when `attach` was the live one.
///
/// A sentinel empty slice is a `None` no compiler makes you handle. This is
/// the same decision as a value, so every entry states it.
pub(crate) enum At {
    /// The verb itself — the first word that is not a global option or one of
    /// their values, which is not the same as the first word after `omh`.
    Verb,
    /// The verb after one of these: `ls` was only ever a verb under `s`.
    Under(&'static [&'static str]),
}

/// Where the verb sits, once the global options in front of it are stepped
/// over.
///
/// `i == 0` was the first attempt and it broke `omh -s s01 attach`, which is
/// exactly the line the `attach` entry exists for: the globals are declared
/// `global = true`, so they may precede the subcommand, and three of them take
/// a value that would otherwise read as the verb.
///
/// Named here rather than derived from clap, which has no public way to ask.
/// A global added without a line here makes `retired` fall silent for lines
/// that start with it — quiet, and the direction that costs a sentence rather
/// than invents one.
pub(crate) fn verb_position(words: &[&str]) -> Option<usize> {
    const TAKES_A_VALUE: [&str; 5] = ["-s", "--session", "-a", "--account", "--color"];
    let mut i = 0;
    while let Some(word) = words.get(i) {
        if !word.starts_with('-') {
            return Some(i);
        }
        // `--color=never` carries its value; `--color never` does not.
        i += if TAKES_A_VALUE.contains(word) { 2 } else { 1 };
    }
    None
}

/// The better sentence for a line clap could not read, when a word in it names
/// a spelling this release retired.
///
/// Stops at `--`: everything after it belongs to a harness, and a harness is
/// allowed a verb omh retired.
pub(crate) fn retired(argv: &[String]) -> Option<&'static str> {
    let words: Vec<&str> = argv
        .iter()
        .skip(1)
        .take_while(|word| *word != "--")
        .map(String::as_str)
        .collect();
    let verb = verb_position(&words);
    words.iter().enumerate().find_map(|(i, word)| {
        RETIRED
            .iter()
            .find(|r| {
                r.spellings.contains(word)
                    && match r.at {
                        At::Verb => Some(i) == verb,
                        At::Under(parents) => i
                            .checked_sub(1)
                            .is_some_and(|prev| parents.contains(&words[prev])),
                    }
            })
            .map(|r| r.said)
    })
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
/// Whether this command can show you what it would do without doing it.
///
/// **A `--dry-run` that is accepted and discarded is a lie**, and it was one on
/// most of the surface: ten commands read the flag and the rest took it and
/// carried on, so `omh --dry-run use --all` wrote the file and printed
/// `wrote →`. The rule is that no command silently ignores it — either it
/// previews, or it says it cannot.
///
/// Exhaustive and matched by variant, like `consumes_session`, so a command
/// added without deciding this is a compile error rather than a silent `false`.
///
/// The ones answering `false` are the ones whose preview is real work: `init`
/// builds images, and the session verbs stop containers, replant commits and
/// delete worktrees — each has to compute what it *would* do, and a preview
/// that guessed would be worse than none. They refuse the flag until they can
/// answer it.
pub(crate) fn previews(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Set { .. }
        | Cmd::Unset { .. }
        | Cmd::Use { .. }
        | Cmd::Unuse { .. }
        | Cmd::Import { .. }
        // Its whole effect is writing a directory tree, so it is exactly the
        // shape of command that has to answer this rather than refuse it.
        | Cmd::Eject { .. }
        | Cmd::New { .. }
        | Cmd::Doctor { .. } => true,
        Cmd::Settings { cmd } => !matches!(cmd, Some(SettingsCmd::Edit { .. })),
        Cmd::Memory { cmd } => matches!(cmd, Some(MemoryCmd::Rm { .. })),
        Cmd::Sessions { cmd } => matches!(cmd, Some(SessionsCmd::Resume { .. })),
        // Read-only: the command *is* its own dry run, so there is nothing to
        // withhold and nothing to describe. Refusing says that, where accepting
        // would imply a preview it never gives.
        Cmd::Info { .. } | Cmd::Why { .. } => false,
        Cmd::Init | Cmd::Auth { .. } | Cmd::Graph { .. } => false,
    }
}

pub(crate) fn consumes_session(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Sessions { .. } => true,
        // **The store is repo-wide, including `remember`.** This returned
        // `true` for it and claimed a relationship the store does not have:
        // the id reached exactly one line, and it was not a scope —
        // `input.source = format!("session {id}, cli")`. Provenance, in a text
        // field, which `--source` already writes. Nothing is filed, scoped or
        // retrieved by session.
        //
        // It bought a global flag for a spelling that half-worked, too: only
        // `-s s01` ever reached it. `omh s01 memory remember` fell to the
        // sessions grammar and answered with clap's *unrecognized subcommand*.
        //
        // The in-sandbox path is untouched — `memory serve` carries its own
        // `--session`, which is how an agent's notes get attribution.
        Cmd::Memory { .. } => false,
        // `graph` is per repo, not per session, and says so in its own doc
        // comment — every session's graph lives in one volume. It took
        // `cli.session` and bound it `_id`, which is how it came to claim
        // otherwise in the comment above `Cmd::Graph`.
        Cmd::Graph { .. }
        | Cmd::Init
        | Cmd::Doctor { .. }
        | Cmd::Why { .. }
        | Cmd::Auth { .. }
        | Cmd::Info { .. }
        | Cmd::Settings { .. }
        | Cmd::Set { .. }
        | Cmd::Unset { .. }
        | Cmd::Use { .. }
        | Cmd::Unuse { .. }
        | Cmd::Import { .. }
        | Cmd::Eject { .. }
        // A fresh session's id is generated, so there is nothing for a named
        // one to mean. A global `--new` used to refuse the same contradiction,
        // but only when the session was spelled `--session`: clap checks
        // `conflicts_with` before the `sNN` prefix is lifted, so the spelling
        // people type went through. That flag is gone; this refuses however
        // the id arrived.
        | Cmd::New { .. } => false,
    }
}

/// A note's layer, which is a different set from a profile's: notes have no
/// personal layer, and the two they do have never merge.
pub(crate) fn parse_note_layer(s: &str) -> std::result::Result<memory::Layer, String> {
    s.parse().map_err(|e: anyhow::Error| e.to_string())
}

pub(crate) fn parse_if_exists(s: &str) -> std::result::Result<memory::IfExists, String> {
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

pub(crate) fn parse_env(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))
}
