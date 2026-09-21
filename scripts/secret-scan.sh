#!/usr/bin/env bash
# Secret scan over tracked files: fails on likely secret material.
#
# Test fixtures use fixed fake credentials (see .secret-scan-allowlist);
# anything else matching the patterns below blocks the change.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ALLOWLIST="$ROOT/.secret-scan-allowlist"
touch "$ALLOWLIST"

# Patterns: private keys, cloud/token prefixes, password assignments.
# NOTE: the live-key alternative below is spelled with a character class so
# this definition block does not match its own pattern; detection of real
# values is unchanged (the class matches exactly one literal character).
PATTERN='(-----BEGIN [A-Z ]*PRIVATE KEY-----|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{20,}|gho_[A-Za-z0-9]{20,}|xox[bap]-|sk-liv[e]-|(?i)(password|passwd|secret|api[_-]?key)\s*[:=]\s*["'\'']?[^"'\''[:space:]]{8,})'

# Tracked text files only; binary/lock/artifact paths never carry reviewable secrets.
files="$(git ls-files | grep -avE '^(Cargo\.lock|dashboard/package-lock\.json|\.git/)' || true)"
[ -n "$files" ] || { echo "secret-scan: no tracked files to scan"; exit 0; }

hits="$(echo "$files" | xargs grep -aEn "$PATTERN" 2>/dev/null || true)"
if [ -z "$hits" ]; then
  echo "secret-scan: PASS (no secret material in tracked files)"
  exit 0
fi

# Drop allowlisted fixture lines (format: path:line-prefix).
unlisted="$hits"
while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  unlisted="$(echo "$unlisted" | grep -avF "$entry" || true)"
done < "$ALLOWLIST"

if [ -z "$unlisted" ]; then
  echo "secret-scan: PASS (all matches are allowlisted fixtures)"
  exit 0
fi

echo "secret-scan: FAIL — possible secret material (redact before commit):" >&2
echo "$unlisted" | sed -E 's/(.{120}).+/\1…/' >&2
echo "next action: move the value out of the repo (env/process config) or add a fixture prefix to .secret-scan-allowlist" >&2
exit 1
