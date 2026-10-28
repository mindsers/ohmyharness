//! Where a session keeps what is its own of the account's — its copies of
//! account files (`auth::Seeds`) and its preferences (`prefs`) — and whether
//! that is kept between its launches. A leaf: `auth` and `prefs` both use it,
//! and `container` decides where each lives.

/// Where a session's own copies of account files live, and whether they are
/// kept between its launches: a real session's are (`Kept`), and a scratch
/// session's are seeded afresh each time (`Scratch`) — `omh doctor`'s probe
/// trusts nothing an earlier probe left there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    Kept(std::path::PathBuf),
    Scratch(std::path::PathBuf),
}

impl Home {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Home::Kept(p) | Home::Scratch(p) => p,
        }
    }
}
