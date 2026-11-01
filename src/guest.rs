//! Where things are inside a sandbox: the paths every container omh starts
//! has, named once, so the image, the launch plan and every report agree.
//!
//! A leaf: it names paths and reads nothing.

/// The agent's working directory inside the sandbox: the session's worktree.
/// Named once, so the note store and the launch plan cannot disagree about it.
pub const WORKDIR: &str = "/work";

/// The agent's home inside the sandbox.
///
/// The image establishes it — `image`'s Dockerfile interpolates this constant
/// into its `usermod -d` — so the image and the code that mounts into it
/// cannot disagree.
///
/// Declared once because the alternative is consistency by copying: a literal
/// in `auth`, another in `container`, another in `doctor`, each with a comment
/// claiming it mirrors the others. That is consistent right up until it isn't,
/// and the symptom is a session that starts and reads nothing.
pub const GUEST_HOME: &str = "/home/agent";

/// Where the note store is mounted inside the sandbox.
///
/// Deliberately not under `/work`: the code graph would index notes as source,
/// `git status` would show them, and an agent running `git add -A` would commit
/// them onto its session branch — where a guess arrives looking reviewed.
pub const GUEST_MEMORY: &str = "/omh/memory";

/// Where the sandbox expects to find omh's own binary, for the memory server.
/// Not under the agent's home, because it is a program rather than state, and
/// `/usr/local/bin` is already on PATH.
pub const GUEST_BIN: &str = "/usr/local/bin/omh";
