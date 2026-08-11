#!/usr/bin/env sh
# Install omh from a published release.
#
#   curl -fsSL https://raw.githubusercontent.com/mindsers/ohmyharness/main/install.sh | sh
#
# Picks the tarball for this machine, checks it against the published
# SHA256SUMS, and puts `omh` somewhere on your PATH. Nothing else — omh itself
# does the setup, and an installer that starts making decisions about your
# machine is the kind of thing this project exists to argue against.
#
#   OMH_VERSION   tag to install (default: the latest release)
#   OMH_BIN_DIR   where to put the binary (default: ~/.local/bin)
#   OMH_BASE_URL  where to fetch from — for mirrors, and for testing this file
#
# POSIX sh on purpose: this is the one piece of omh that runs before omh does,
# on a machine nobody has checked.

set -eu

REPO="mindsers/ohmyharness"
VERSION="${OMH_VERSION:-}"
BIN_DIR="${OMH_BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

need() {
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required and was not found"
  done
}

# Inlined rather than read back out of $0: piped through `curl | sh` there is
# no file to read, and a --help that works only when downloaded first is a
# --help that fails the one time somebody needs it.
usage() {
  cat <<'EOF'
Install omh from a published release.

  curl -fsSL https://raw.githubusercontent.com/mindsers/ohmyharness/main/install.sh | sh

  OMH_VERSION   tag to install (default: the latest release)
  OMH_BIN_DIR   where to put the binary (default: ~/.local/bin)
  OMH_BASE_URL  where to fetch from — for mirrors, and for testing this file
EOF
  exit 0
}

case "${1:-}" in
  -h|--help) usage ;;
  "") ;;
  *) die "unexpected argument: $1 (see --help)" ;;
esac

# Everything the script shells out to. Checked up front so a missing tool is
# reported as a missing tool: without this, a machine without coreutils reached
# `install` and was told to set OMH_BIN_DIR to somewhere writable, which was
# both wrong and a good way to spend an afternoon on file permissions.
need curl tar awk mktemp install

# macOS ships `shasum`, most Linuxes ship `sha256sum`, and a download nobody
# verified is a download nobody should run.
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "need sha256sum or shasum to verify the download"
fi

# Which tarball. Linux is static musl, so one build covers every distribution.
os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)          target="aarch64-apple-darwin" ;;
  Darwin/x86_64)         target="x86_64-apple-darwin" ;;
  Linux/x86_64|Linux/amd64)   target="x86_64-unknown-linux-musl" ;;
  Linux/aarch64|Linux/arm64)  target="aarch64-unknown-linux-musl" ;;
  *) die "no published build for $os/$arch — build from source: https://github.com/$REPO" ;;
esac

# The tag. Resolved from the redirect rather than the API, which is rate
# limited per IP and fails in exactly the shared environments — CI, corporate
# NAT — where the error is hardest to read.
if [ -z "$VERSION" ]; then
  latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/$REPO/releases/latest" 2>/dev/null)" \
    || die "could not reach github to find the latest release"
  VERSION="${latest##*/}"
  case "$VERSION" in
    v*) ;;
    *) die "no published release yet — build from source: https://github.com/$REPO" ;;
  esac
fi

# Validated whether it came from the redirect or from OMH_VERSION. It is
# interpolated straight into the download URL, so a value containing a path
# would fetch a different repository's release — and verify it happily against
# that repository's own SHA256SUMS, since both come from the same base.
case "$VERSION" in
  v[0-9]*) ;;
  *) die "$VERSION is not a version tag" ;;
esac
case "$VERSION" in
  */*|*..*) die "$VERSION is not a version tag" ;;
esac

base="${OMH_BASE_URL:-https://github.com/$REPO/releases/download}/$VERSION"
tarball="omh-$target.tar.gz"

# Both are cleaned on any exit. The staged binary matters as much as the temp
# directory: it lands in $BIN_DIR, so an interrupt between staging and the move
# would otherwise leave an executable named .omh.incoming.NNN there forever.
tmp=""
staged=""
cleanup() {
  [ -n "$tmp" ] && rm -rf "$tmp"
  [ -n "$staged" ] && rm -f "$staged"
  return 0
}
trap cleanup EXIT INT TERM

tmp="$(mktemp -d)"

say "omh $VERSION — $target"

curl -fsSL "$base/$tarball" -o "$tmp/$tarball" \
  || die "no $tarball in $VERSION — that platform may not be published"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" \
  || die "release $VERSION publishes no SHA256SUMS; refusing to install unverified"

# Match the exact file name, so a checksum for a different platform cannot
# stand in for this one.
# `\r` because a SHA256SUMS with CRLF endings would otherwise match nothing and
# report that the release lists no entry for a file it plainly lists.
expected="$(awk -v f="$tarball" '{ sub(/^\.\//, "", $2); sub(/\r$/, "", $2); if ($2 == f) print $1 }' \
  "$tmp/SHA256SUMS")"
[ -n "$expected" ] || die "SHA256SUMS lists no entry for $tarball"

actual="$(sha256 "$tmp/$tarball")"
# There is no pipefail in POSIX sh, so a hashing tool that dies still leaves
# `cut` exiting 0 and `actual` empty. Reported as a broken tool rather than as
# a corrupt download, which is a different afternoon.
[ -n "$actual" ] || die "could not hash $tarball"
if [ "$expected" != "$actual" ]; then
  die "checksum mismatch for $tarball
  expected $expected
  got      $actual"
fi
say "  checksum ok"

tar -xzf "$tmp/$tarball" -C "$tmp" || die "$tarball is not a readable tar.gz"
[ -f "$tmp/omh-$target/omh" ] || die "$tarball did not contain omh-$target/omh"

mkdir -p "$BIN_DIR" || die "could not create $BIN_DIR"

# Staged beside the target and moved into place only once it has been shown to
# run. A binary for the wrong architecture downloads and unpacks perfectly
# well and only says so when executed — and if that happened after overwriting
# `omh`, a failed install would have replaced a working one with a broken one.
# The `mv` is within a single directory, so it is atomic.
staged="$BIN_DIR/.omh.incoming.$$"
install -m 755 "$tmp/omh-$target/omh" "$staged" \
  || die "could not write to $BIN_DIR — set OMH_BIN_DIR to somewhere writable"

if ! probe="$("$staged" --version 2>&1)"; then
  die "the downloaded omh does not run here — wrong build for $os/$arch?
  $probe"
fi

# The tag names a version; so does the binary. A release whose tarball holds
# the previous build passes every check above, and the installer would announce
# the version it meant to install rather than the one it did.
want="${VERSION#v}"
case "$probe" in
  *"$want"*) ;;
  *) die "$tarball reports '$probe', which does not match the tag $VERSION" ;;
esac

mv -f "$staged" "$BIN_DIR/omh" || die "could not install into $BIN_DIR"
staged=""

say "  installed $BIN_DIR/omh"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    say ""
    say "$BIN_DIR is not on your PATH. Add it:"
    say "  export PATH=\"\$PATH:$BIN_DIR\""
    ;;
esac

# Stated, not enforced. omh needs a container runtime and git, and neither is
# something an installer should be quietly putting on your machine.
missing=""
command -v docker >/dev/null 2>&1 || command -v podman >/dev/null 2>&1 || missing="docker or podman"
command -v git >/dev/null 2>&1 || missing="${missing:+$missing and }git"
if [ -n "$missing" ]; then
  say ""
  say "omh needs $missing, which is not installed. 'omh doctor' checks the rest."
fi

say ""
say "next: cd into a repo and run \`omh init\`"
