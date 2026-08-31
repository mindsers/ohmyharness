//! One module per command, or per family of them.
//!
//! `main.rs` was the crate root and also where almost every command lived —
//! 8,434 lines of production code and about a hundred top-level functions,
//! while the other forty modules stayed cleanly bounded. Nothing failed
//! because of it, which is why it grew: no test in the suite had an opinion
//! about the size of a file, so the only thing that ever pushed back was
//! somebody reading it and minding.
//!
//! What stays in the root is the shape of a command line — `main`, `dispatch`
//! and the few helpers that run before any command does. What lives here is
//! what each command *does*. The split is by the verb a user types rather
//! than by the code's own structure, so the file to open is the one named
//! after the thing that went wrong.
//!
//! `the_crate_root_stays_a_crate_root` in `main.rs` keeps it that way.

pub mod auth;
pub mod catalogue;
pub mod harvest;
pub mod init;
pub mod inspect;
pub mod mcp;
pub mod memory;
pub mod session;
pub mod settings;
