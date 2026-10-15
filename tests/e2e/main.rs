//! Every feature omh gives a harness, run against the harness itself.
//!
//! The rest of the suite proves what omh *writes* — a rendered hook, a mounted
//! file, an MCP document — and runs a rendered hook outside the harness against
//! a payload written by hand. Neither proves the harness reads what omh wrote,
//! or sends what the hand-written payload assumed. Those are claims about
//! software omh does not control, and this suite checks them: each harness, at
//! the version `adapters/` pins, in the image omh builds for it, in a session
//! `omh new` started, playing turns against a fake model whose request log is
//! what the model saw.
//!
//! It proves omh and the harness together, with a model that does exactly what
//! the script says. It does not prove a real model would choose to make those
//! calls.
//!
//! Behind the `e2e` feature, so it builds only when asked for: it needs Docker,
//! the four images, and minutes. Run it with `cargo test --features e2e --test
//! e2e`; the nightly workflow runs it once per harness, `-- --exact <name>`.

mod driver;
mod sandbox;
mod scenario;

#[test]
fn claude() {
    scenario::all(&driver::CLAUDE);
}

#[test]
fn codex() {
    scenario::all(&driver::CODEX);
}

#[test]
fn omp() {
    scenario::all(&driver::OMP);
}

#[test]
fn opencode() {
    scenario::all(&driver::OPENCODE);
}
