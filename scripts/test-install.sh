#!/usr/bin/env bash
# Tests for install.sh, the one piece of omh that runs before omh does.
#
#   ./scripts/test-install.sh
#
# It serves a fake release over file:// built from the local debug binary, so
# every branch is exercised without publishing anything. The installer had no
# tests at all until a review pointed out that the file most likely to run on a
# stranger's machine was the file nothing checked.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
script="$here/install.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; pass=$((pass + 1)); }
# The same two-space format release.yml publishes, whichever tool this host has.
sums() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}
bad()  { printf '  \033[31mFAIL\033[0m %s\n     %s\n' "$1" "$2"; fail=$((fail + 1)); }

# The triple install.sh will ask for on this machine. Duplicated from the
# script on purpose: if the two ever disagree, these tests stop finding the
# tarball and say so, which is the failure worth hearing about.
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64)   TARGET=aarch64-apple-darwin ;;
  Darwin/x86_64)  TARGET=x86_64-apple-darwin ;;
  Linux/x86_64)   TARGET=x86_64-unknown-linux-musl ;;
  Linux/aarch64)  TARGET=aarch64-unknown-linux-musl ;;
  *) echo "no test coverage for $(uname -s)/$(uname -m)" >&2; exit 1 ;;
esac

binary="$here/target/debug/omh"
[ -x "$binary" ] || { echo "build first: cargo build" >&2; exit 1; }

# The good release is tagged with the version the binary actually reports, so
# the happy path is not itself a version mismatch. The lying-tarball case below
# uses a different tag on purpose.
TAG="v$("$binary" --version | awk '{print $NF}')"

# A release directory shaped exactly like the one release.yml publishes.
mkrelease() {
  local dir="$1/${2:-$TAG}" stage
  mkdir -p "$dir"
  stage="$1/stage/omh-$TARGET"
  mkdir -p "$stage"
  cp "$binary" "$stage/omh"
  cp "$here/LICENSE" "$here/README.md" "$stage/"
  ( cd "$1/stage" && tar -czf "$dir/omh-$TARGET.tar.gz" "omh-$TARGET" )
  rm -rf "$1/stage"
  ( cd "$dir" && sums ./*.tar.gz | sed 's|\./||' > SHA256SUMS )
}

# Runs install.sh and reports "<exit>|<combined output>".
attempt() {
  local out status
  set +e
  out="$("$@" sh "$script" 2>&1)"
  status=$?
  set -e
  printf '%s|%s' "$status" "$out"
}

expect_fail() {
  local name="$1" needle="$2" result="$3"
  local status="${result%%|*}" out="${result#*|}"
  if [ "$status" = 0 ]; then
    bad "$name" "expected a refusal, got success"
  elif ! printf '%s' "$out" | grep -qF "$needle"; then
    bad "$name" "message did not mention '$needle': $out"
  else
    ok "$name"
  fi
}

echo "install.sh"

# --- the happy path ---------------------------------------------------------
good="$work/good"; mkrelease "$good"
bin="$work/bin-good"
res="$(attempt env OMH_VERSION="$TAG" OMH_BASE_URL="file://$good" OMH_BIN_DIR="$bin")"
if [ "${res%%|*}" = 0 ] && [ -x "$bin/omh" ]; then
  ok "installs a good release"
else
  bad "installs a good release" "${res#*|}"
fi

# --- refusals ---------------------------------------------------------------
tampered="$work/tampered"; mkrelease "$tampered"
printf 'x' >> "$tampered/$TAG/omh-$TARGET.tar.gz"
expect_fail "refuses a tampered tarball" "checksum mismatch" \
  "$(attempt env OMH_VERSION="$TAG" OMH_BASE_URL="file://$tampered" OMH_BIN_DIR="$work/bin-t")"

nosums="$work/nosums"; mkrelease "$nosums"; rm "$nosums/$TAG/SHA256SUMS"
expect_fail "refuses a release with no SHA256SUMS" "refusing to install unverified" \
  "$(attempt env OMH_VERSION="$TAG" OMH_BASE_URL="file://$nosums" OMH_BIN_DIR="$work/bin-n")"

partial="$work/partial"; mkrelease "$partial"; : > "$partial/$TAG/SHA256SUMS"
expect_fail "refuses when this platform has no checksum" "lists no entry" \
  "$(attempt env OMH_VERSION="$TAG" OMH_BASE_URL="file://$partial" OMH_BIN_DIR="$work/bin-p")"

# A version string is interpolated straight into the download URL. Traversal
# would silently fetch a different repository's release and verify it happily
# against that repository's own checksums.
expect_fail "refuses a version that is not a tag" "not a version tag" \
  "$(attempt env OMH_VERSION=../../../other/releases/download/v1 OMH_BASE_URL="file://$good" OMH_BIN_DIR="$work/bin-v")"

# --- a broken download must not disturb a working install -------------------
wrong="$work/wrong"; mkrelease "$wrong"
stage="$work/wrongstage/omh-$TARGET"; mkdir -p "$stage"
printf '#!/bin/sh\nexit 1\n' > "$stage/omh"; chmod +x "$stage/omh"
cp "$here/LICENSE" "$here/README.md" "$stage/"
( cd "$work/wrongstage" && tar -czf "$wrong/$TAG/omh-$TARGET.tar.gz" "omh-$TARGET" )
( cd "$wrong/$TAG" && sums ./*.tar.gz | sed 's|\./||' > SHA256SUMS )

before="$("$bin/omh" --version)"
res="$(attempt env OMH_VERSION="$TAG" OMH_BASE_URL="file://$wrong" OMH_BIN_DIR="$bin")"
expect_fail "refuses a binary that will not run" "does not run" "$res"
if [ "$("$bin/omh" --version)" = "$before" ]; then
  ok "a failed install leaves the working omh alone"
else
  bad "a failed install leaves the working omh alone" "omh was replaced by a broken build"
fi
if [ -z "$(find "$bin" -name '.omh.incoming.*' 2>/dev/null)" ]; then
  ok "a failed install leaves no staged file behind"
else
  bad "a failed install leaves no staged file behind" "$(ls -a "$bin")"
fi

# --- the version it reports is the version it installed ---------------------
lying="$work/lying"; mkrelease "$lying"
mv "$lying/$TAG" "$lying/v1.2.3"
( cd "$lying/v1.2.3" && sums ./*.tar.gz | sed 's|\./||' > SHA256SUMS )
expect_fail "refuses a tarball whose binary is not the tagged version" "does not match" \
  "$(attempt env OMH_VERSION=v1.2.3 OMH_BASE_URL="file://$lying" OMH_BIN_DIR="$work/bin-l")"

# --- tools it needs but never checked for -----------------------------------
# A PATH holding everything except the tool under test. Without the check, the
# script dies at the call site instead, blaming whatever that line was about.
minimal="$work/minimal"; mkdir -p "$minimal"
for t in sh curl tar uname mkdir rm mv cut cat awk mktemp install sha256sum shasum grep sed find; do
  p="$(command -v "$t" 2>/dev/null)" && ln -sf "$p" "$minimal/$t"
done
for missing in install awk mktemp; do
  rm -f "$minimal/$missing"
  expect_fail "says which tool is missing: $missing" "$missing is required" \
    "$(attempt env PATH="$minimal" OMH_VERSION="$TAG" OMH_BASE_URL="file://$good" OMH_BIN_DIR="$work/bin-$missing")"
  p="$(command -v "$missing" 2>/dev/null)" && ln -sf "$p" "$minimal/$missing"
done

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
