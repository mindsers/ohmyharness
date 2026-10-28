//! A harness's preferences, per session: the declared keys of its own config
//! (`[preferences]` in its adapter) kept per account, copied into each
//! session, and written back only as those keys.
//!
//! The account's config file is shared by every session on it, so whatever
//! one session wrote there — a server, `developer_instructions`, a provider —
//! the next session read. Here the account keeps only declared keys, at its
//! root where no mount reaches, and each session gets its own copy:
//!
//! ```text
//! ~/.omh/creds/<harness>/<account>/preferences/<file>   the account's kept values
//! ~/.omh/run/<repo>/<session>/.preferences/<harness>/
//!   account   the account this session launched with
//!   base/     the account's kept values when this session last synced
//!   reviewed/ the live values at the last decision
//!   live/     what the harness reads and writes, mounted at GUEST
//! ```

/// Where a container finds its session's `live/`, for a `link` adapter: the
/// account's file is a link to the same name here.
pub const GUEST: &str = "/omh/prefs";
