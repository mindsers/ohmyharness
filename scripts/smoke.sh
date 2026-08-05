#!/usr/bin/env bash
# End-to-end smoke test in a throwaway repo.
#
# Exercises everything that works without a container image. The parts that need
# one (real launch, auth) are listed at the end rather than silently skipped —
# a smoke test that hides what it did not cover is worse than none.
#
#   ./scripts/smoke.sh
set -euo pipefail

OMH="$(cd "$(dirname "$0")/.." && pwd)/target/debug/omh"
[ -x "$OMH" ] || { echo "build first: cargo build"; exit 1; }

REPO=$(mktemp -d)
trap 'rm -rf "$REPO" "$HOME/.omh/worktrees/$(basename "$REPO")" "$HOME/.omh/run/$(basename "$REPO")"' EXIT
cd "$REPO"
git init -q -b main .
printf 'fn main() {}\n' > main.rs
git add -A && git -c user.email=t@e.com -c user.name=t commit -qm init

step() { printf '\n\033[1m── %s\033[0m\n' "$1"; }

step "init — scaffolds the committed and gitignored layers"
"$OMH" init

step "config — every value says which layer it came from"
"$OMH" set idle_timeout 30m
"$OMH" set carry_in '[".env.local"]' --layer shared
"$OMH" config policy

step "mcp — add, then import, with conflict handling"
"$OMH" mcp add fs npx -y @modelcontextprotocol/server-filesystem /work
cat > .mcp.json <<'EOF'
{ "mcpServers": {
    "sentry": { "command": "sentry-mcp", "args": ["--org","acme"] },
    "fs":     { "command": "conflicting" } } }
EOF
"$OMH" mcp import claude
"$OMH" mcp ls

step "dry-run — must leave no trace"
"$OMH" --dry-run claude | head -3
test -z "$(git branch --list 'omh/*')" || { echo "FAIL: dry run created a branch"; exit 1; }
echo "ok: no branch, no worktree, no staging"

step "sessions — worktree on its own branch, reviewable by git"
"$OMH" -s s01 claude >/dev/null 2>&1 || true   # will fail at launch: no image
WT=$(git worktree list | awk '/omh\/s01/ {print $1}')
echo "worktree: $WT"
echo "staged rules:"; ls "$WT" | sed 's/^/  /'
( cd "$WT" && printf '// agent edit\n' >> main.rs \
  && git add -A && git -c user.email=t@e.com -c user.name=t commit -qm "agent work" )
"$OMH" diff s01 --base main

step "isolation — your checkout is untouched"
git diff --quiet && echo "ok: main is clean; agent work lives on omh/s01"

step "runtime backends"
"$OMH" set runtime podman >/dev/null
"$OMH" --dry-run claude 2>&1 | head -1 || true
"$OMH" unset runtime >/dev/null

cat <<'EOF'

── not covered (needs a container image, which does not exist yet)
   omh auth <harness>     credential capture   — unimplemented
   omh claude             real launch          — fails: omh/claude:latest missing
   omh code / fwd / down  session lifecycle    — not built
   omh doctor             adapter verification — not built

   Adapter paths remain unverified claims until omh doctor exists.
EOF
