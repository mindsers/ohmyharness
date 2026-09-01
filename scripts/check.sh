#!/usr/bin/env bash
# What CI runs, run here, with the one check CI does not need.
#
#   ./scripts/check.sh          format, lints, tests
#   ./scripts/check.sh --all    and the tests that need a container runtime
#
# The extra check is a toolchain-agreement guard, and it is here because the
# lint it protects was silently absent for an unknown length of time. Measured
# on the author's machine: `cargo clippy` resolved to a stale 0.1.81 shim on
# PATH while `rustc` was 1.98, so clippy refused on `rust-version = 1.85` and
# printed a dependency wall instead of a lint. Nothing failed. The lint simply
# never ran, locally, while CI stayed green and everyone believed it had.
#
# `rust-toolchain.toml` documents that incident and cannot prevent it: rustup
# reads that file, a Homebrew or distro cargo does not. A pin only binds the
# tool that agreed to be bound, which is why the fix is a check rather than a
# stronger pin.
#
# Tested by scripts/test-check.sh, against stub toolchains — the mismatch has
# to be provokable on a machine whose real toolchain agrees.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

# The channel rust-toolchain.toml names. Duplicated from that file on purpose,
# in the same spirit as test-install.sh's target triple: if the two disagree,
# this script runs the wrong toolchain and the message below says which one it
# ran, which is the failure worth hearing about.
channel=stable

if command -v rustup >/dev/null 2>&1; then
  use_rustup=yes
  toolchain="rustup run $channel"
else
  use_rustup=no
  toolchain="$(command -v cargo 2>/dev/null || echo 'cargo (not found)')"
fi

# Every toolchain invocation goes through here, so the guard below and the
# checks after it cannot end up asking different compilers.
run() {
  if [ "$use_rustup" = yes ]; then
    rustup run "$channel" "$@"
  else
    "$@"
  fi
}

step() { printf '\033[1m%s\033[0m\n' "$1"; }
die()  { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 1; }

# --- the guard --------------------------------------------------------------

# `cargo clippy --version` never names a rustc. What connects them is that
# clippy is versioned `0.1.<rustc minor>`, so the minor is the only thread
# between the two — and comparing it is the whole check.
#
# Measured here, which is the pairing that prompted this: `clippy 0.1.81`
# alongside `rustc 1.98.0`. The parse is pinned by scripts/test-check.sh, and
# a string neither half recognises is a refusal below rather than a shrug —
# so if the format ever moves, this says so instead of quietly comparing two
# empty strings and calling them equal.
if ! clippy_version="$(run cargo clippy --version 2>/dev/null)"; then
  die "clippy is not installed for $toolchain.
     rustup component add clippy --toolchain $channel"
fi
rustc_version="$(run rustc --version)"

clippy_minor="$(printf '%s' "$clippy_version" | sed -n 's/^clippy 0\.1\.\([0-9][0-9]*\).*/\1/p')"
rustc_minor="$(printf '%s' "$rustc_version" | sed -n 's/^rustc 1\.\([0-9][0-9]*\)\..*/\1/p')"

# An unreadable version is a refusal, not a shrug. Both strings have been
# stable for years, so a parse that comes back empty means the format moved —
# and a guard that treats "cannot tell" as "fine" is the failure this whole
# script exists to stop, reintroduced one level up.
if [ -z "$clippy_minor" ] || [ -z "$rustc_minor" ]; then
  die "cannot read a version to compare:
       $clippy_version
       $rustc_version
     check.sh's parse needs updating — see scripts/test-check.sh"
fi

# Agreeing with each other is not the same as being new enough. The first
# version of this checked only clippy against rustc, so a toolchain where both
# sat at 1.81 passed — and cargo then produced the dependency wall this script
# exists to make legible, one layer down and just as unhelpfully. Measured: a
# fresh `brew install rustup` whose `stable` was a 2024 build.
#
# Read from Cargo.toml rather than typed, because a duplicated MSRV is a
# second thing to keep in step and this file already carries one of those.
needed="$(sed -n 's/^rust-version *= *"1\.\([0-9][0-9]*\).*/\1/p' "$here/Cargo.toml")"
if [ -n "$needed" ] && [ "$rustc_minor" -lt "$needed" ]; then
  die "omh needs rustc 1.$needed and $toolchain has 1.$rustc_minor.
     $rustc_version
     cargo will refuse to build the dependency tree and print a wall of
     'requires rustc 1.$needed' lines, which is the failure this script exists
     to name rather than repeat.
     rustup update stable"
fi

if [ "$clippy_minor" != "$rustc_minor" ]; then
  die "clippy is built against rustc 1.$clippy_minor but rustc here is 1.$rustc_minor.
     $clippy_version
     $rustc_version
     answered by $toolchain
     A clippy older than the crate's rust-version refuses to run at all and
     prints a dependency wall, so the lint is absent rather than failing.
     Install rustup, or take the stale shim off your PATH."
fi

# --- what CI runs -----------------------------------------------------------
# Same commands, same flags, same order as .github/workflows/ci.yml. A local
# check that is merely similar to CI teaches you to distrust it.

step "format"
run cargo fmt --check

step "lints"
run cargo clippy --locked --all-targets -- -D warnings

if [ "${1:-}" = --all ]; then
  step "tests, including the ones needing a container runtime"
  run cargo test --locked -- --include-ignored
else
  step "tests"
  run cargo test --locked
  printf '  (--all adds the tests needing docker and node)\n'
fi

printf '\n\033[32mall checks passed\033[0m — %s\n' "$toolchain"
