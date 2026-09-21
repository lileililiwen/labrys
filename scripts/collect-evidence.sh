#!/usr/bin/env bash
# Evidence collection: runs the check suite and writes a tier-labeled report.
#
# Tiers (see docs/release.md): static, model-only, integration, staging.
# Production evidence is NEVER emitted here — the report records it blocked
# with the next action. Blocked infrastructure is reported, never passed.
# A failed check fails this script; blockers are allowed but suppress any
# release claim in the report summary.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT="evidence"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    *) echo "usage: collect-evidence.sh [--out DIR]" >&2; exit 2 ;;
  esac
done
mkdir -p "$OUT"
REPORT="$OUT/report.json"
echo "[]" > "$REPORT"

record() { # check tier status command detail [next_action]
  python3 - "$REPORT" "$1" "$2" "$3" "$4" "$5" "${6:-}" <<'EOF'
import json, sys
path, check, tier, status, command, detail, nxt = sys.argv[1:8]
with open(path) as fh: items = json.load(fh)
entry = {"check": check, "tier": tier, "status": status, "command": command, "detail": detail}
if nxt: entry["next_action"] = nxt
items.append(entry)
with open(path, "w") as fh: json.dump(items, fh, indent=1)
EOF
}

failed=0
blocked=0
run_check() { # check tier command...
  local check="$1"; local tier="$2"; shift 2
  local out
  set +e
  out="$("$@" 2>&1)"
  local code=$?
  set -e
  local tail
  tail="$(echo "$out" | tail -n 2 | tr '\n' ';' | cut -c1-300)"
  if [ "$code" = 0 ]; then
    record "$check" "$tier" "pass" "$*" "$tail"
  elif [ "$code" = 3 ]; then
    record "$check" "$tier" "blocked" "$*" "$tail" "provision the missing infrastructure, then re-run"
    blocked=$((blocked + 1))
  else
    record "$check" "$tier" "failed" "$*" "$tail" "fix the failure; the change is blocked until this passes"
    failed=$((failed + 1))
  fi
}

run_check "format" "static" cargo fmt --all --check
run_check "lint" "static" cargo clippy --workspace --all-targets -- -D warnings
run_check "secret-scan" "static" bash scripts/secret-scan.sh
run_check "openspec-names" "static" node scripts/check-openspec-change-names.mjs
run_check "openspec-strict" "static" openspec validate --all --strict
run_check "dashboard-typecheck" "model-only" npm run typecheck --prefix dashboard
run_check "dashboard-tests" "model-only" npm test --prefix dashboard
run_check "build" "model-only" cargo build --workspace
run_check "core-unit-tests" "model-only" cargo test -p labrys-core

if [ -n "${LABRYS_DATABASE_URL:-${DATABASE_URL:-}}" ]; then
  run_check "migration-check" "integration" bash scripts/verify-migrations.sh
  run_check "workspace-tests" "integration" cargo test --workspace
  run_check "staging-smoke" "staging" bash scripts/smoke-staging.sh --out "$OUT"
else
  record "migration-check" "integration" "blocked" "bash scripts/verify-migrations.sh" \
    "no database configured" "set LABRYS_DATABASE_URL (disposable compose.test.yml), then re-run"
  record "workspace-tests" "integration" "blocked" "cargo test --workspace" \
    "no database configured; unit-only subset ran above" "set LABRYS_DATABASE_URL, then re-run"
  record "staging-smoke" "staging" "blocked" "bash scripts/smoke-staging.sh" \
    "no database configured" "set LABRYS_DATABASE_URL, then re-run"
  blocked=$((blocked + 3))
fi

record "production-proof" "production" "blocked" "runtime proof outside CI" \
  "no production claim is made from this report" \
  "run staged lifecycle evidence against provider scope per docs/release.md"

python3 - "$REPORT" "$failed" "$blocked" <<'EOF'
import json, sys
path = sys.argv[1]
with open(path) as fh: items = json.load(fh)
summary = {"failed": int(sys.argv[2]), "blocked": int(sys.argv[3]),
           "release_claim": "none: model/integration/staging evidence only" if int(sys.argv[2]) == 0 else "none: failing checks"}
items.append({"summary": summary})
with open(path, "w") as fh: json.dump(items, fh, indent=1)
print(f"evidence: {len(items) - 1} checks, {summary['failed']} failed, {summary['blocked']} blocked -> {path}")
EOF

[ "$failed" = 0 ]
