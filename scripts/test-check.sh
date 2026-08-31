#!/usr/bin/env bash
# Tests for check.sh, which is what stands between a local edit and CI.
#
#   ./scripts/test-check.sh
#
# The defect it exists for is real and was live on the author's machine:
# `cargo clippy` resolved to a stale 0.1.81 shim on PATH while `rustc` was
# 1.98, so clippy refused on `rust-version = 1.85` and its output was silently
# absent from every local check. `rust-toolchain.toml` documents that failure
# and cannot prevent it — rustup reads the file, a Homebrew cargo does not.
#
# So the thing under test is the *guard*, not the lints. Every case below runs
# check.sh against stub `cargo` and `rustc` binaries on a PATH of its own,
# which is what lets the mismatch be provoked on a machine where the real
# toolchain agrees — and what keeps the suite from spending a minute compiling
# to answer a question about version parsing.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
script="$here/scripts/check.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

ok()  { printf '  \033[32mok\033[0m   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n     %s\n' "$1" "$2"; fail=$((fail + 1)); }

# A PATH holding the tools check.sh may use and nothing else — deliberately no
# `rustup`, so the fallback branch is the one under test. With the real PATH
# inherited, `rustup run stable cargo` would resolve and route straight past
# the stubs, and every case below would pass without exercising anything.
stubs="$work/bin"
mkdir -p "$stubs"
for t in sh bash env grep sed awk cut head tr printf uname mktemp rm cat dirname pwd; do
  p="$(command -v "$t" 2>/dev/null)" && ln -sf "$p" "$stubs/$t"
done

# Writes a stub pair reporting the versions asked for. `cargo` answers
# `clippy --version` with the crafted string and exits 0 for everything else,
# so a run that gets past the guard finishes immediately instead of building.
stub() {
  local clippy_version="$1" rustc_version="$2"
  cat > "$stubs/cargo" <<EOF
#!/bin/sh
if [ "\$1" = clippy ] && [ "\$2" = --version ]; then
  echo "$clippy_version"
  exit 0
fi
exit 0
EOF
  cat > "$stubs/rustc" <<EOF
#!/bin/sh
echo "$rustc_version"
EOF
  chmod +x "$stubs/cargo" "$stubs/rustc"
}

# Runs check.sh under the stub PATH and reports "<exit>|<combined output>".
attempt() {
  local out status
  set +e
  out="$(PATH="$stubs" sh "$script" 2>&1)"
  status=$?
  set -e
  printf '%s|%s' "$status" "$out"
}

echo "check.sh"

# --- the guard --------------------------------------------------------------
# The exact pairing measured on the author's machine: clippy 0.1.81 against
# rustc 1.98.0. Clippy's version is `0.1.<rustc minor>`, which is the only
# thread connecting the two — `cargo clippy --version` never names a rustc.
stub "clippy 0.1.81 (eeb90cda 2024-09-04)" "rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)"
res="$(attempt)"
status="${res%%|*}"; out="${res#*|}"
if [ "$status" = 0 ]; then
  bad "refuses a clippy built against another rustc" "expected a refusal, got success"
elif ! printf '%s' "$out" | grep -qF "clippy is built against rustc"; then
  bad "refuses a clippy built against another rustc" "message did not say so: $out"
else
  ok "refuses a clippy built against another rustc"
fi

# The refusal has to name the file it is talking about, or the reader is left
# to rediscover which of several cargos on their PATH answered.
#
# The `$status` half is not decoration. Without it this passed against a
# check.sh whose comparison had been deleted — the success path ends
# `all checks passed — <cargo>`, which carries the same path and satisfied a
# bare grep. A guard that a passing run can satisfy is not testing the
# refusal.
if [ "$status" != 0 ] && printf '%s' "$out" | grep -qF "$stubs/cargo"; then
  ok "the refusal names the cargo that answered"
else
  bad "the refusal names the cargo that answered" "no refusal naming a path in: $out"
fi

# --- agreement passes -------------------------------------------------------
stub "clippy 0.1.98 (88d9e12ae 2026-08-18)" "rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)"
res="$(attempt)"
if [ "${res%%|*}" = 0 ]; then
  ok "an agreeing toolchain runs the checks"
else
  bad "an agreeing toolchain runs the checks" "${res#*|}"
fi

# --- a version string neither parse recognises ------------------------------
# Added because deleting check.sh's unreadable-version refusal left every case
# above green: the comparison it guards is between two empty strings, which
# are equal, so a format change would have sailed through as agreement. That
# is the same fail-open shape the whole script exists to close.
stub "clippy 2.0-preview" "rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)"
res="$(attempt)"
status="${res%%|*}"; out="${res#*|}"
if [ "$status" = 0 ]; then
  bad "refuses a version it cannot parse" "expected a refusal, got success"
elif ! printf '%s' "$out" | grep -qF "cannot read a version to compare"; then
  bad "refuses a version it cannot parse" "message did not say so: $out"
else
  ok "refuses a version it cannot parse"
fi

# --- a clippy that is not installed at all ----------------------------------
# Distinct from disagreement and worth its own message: `cargo clippy` on a
# toolchain without the component fails at the subcommand, and a guard that
# reported that as a version mismatch would send the reader to the wrong fix.
cat > "$stubs/cargo" <<'EOF'
#!/bin/sh
echo "error: no such command: \`clippy\`" >&2
exit 101
EOF
chmod +x "$stubs/cargo"
res="$(attempt)"
status="${res%%|*}"; out="${res#*|}"
if [ "$status" = 0 ]; then
  bad "refuses when clippy is not installed" "expected a refusal, got success"
elif ! printf '%s' "$out" | grep -qF "clippy is not installed"; then
  bad "refuses when clippy is not installed" "message did not say so: $out"
else
  ok "refuses when clippy is not installed"
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
