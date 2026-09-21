#!/usr/bin/env bash
# Staging smoke: disposable end-to-end pass over the packaged entry points.
#
# Stages: version -> import -> inspect -> health (never healthy without
# platform observation) -> doctor -> agent deploy denial -> rollback without
# approval denial -> logs -> protocol negotiation.
#
# Runtime delivery (real container build/run, provider provisioning) is NOT
# exercised here: the packaged control plane ships the safe no-op dispatcher,
# so execution stages are recorded BLOCKED with the recovery action instead
# of a fake pass. See docs/release.md for the evidence vocabulary.
#
# Requires: LABRYS_DATABASE_URL (or DATABASE_URL), docker daemon for the
# daemon-presence probe, cargo-built binaries. Scratch state is cleaned up.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    *) echo "usage: smoke-staging.sh [--out DIR]" >&2; exit 2 ;;
  esac
done

DB_URL="${LABRYS_DATABASE_URL:-${DATABASE_URL:-}}"
if [ -z "$DB_URL" ]; then
  echo "smoke-staging: BLOCKED — no database (set LABRYS_DATABASE_URL)" >&2
  echo "next action: start the disposable environment (docker compose -f compose.test.yml up -d)" >&2
  exit 3
fi

cargo build --quiet --bin control-plane --bin labrys

ADMIN_URL="$(echo "$DB_URL" | sed -E 's|/[^/]*$|/postgres|')"
SCRATCH="labrys_smoke"
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $SCRATCH;" >/dev/null
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "CREATE DATABASE $SCRATCH;" >/dev/null
SCRATCH_URL="$(echo "$DB_URL" | sed -E "s|/[^/]*$|/$SCRATCH|")"

PORT="${LABRYS_SMOKE_PORT:-18080}"
TOKEN="smoke-test-token-0123456789"
FIXTURE="$(mktemp -d)"
cat > "$FIXTURE/Dockerfile" <<'EOF'
FROM alpine:3.20
CMD ["sh", "-c", "echo smoke-ok && sleep 5"]
EOF

LABRYS_BIN="./target/debug/labrys"
SERVER_LOG="$(mktemp)"
cleanup() {
  kill "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
  psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $SCRATCH;" >/dev/null
  rm -rf "$FIXTURE" "$SERVER_LOG"
}
trap cleanup EXIT

LABRYS_DATABASE_URL="$SCRATCH_URL" LABRYS_RUN_MIGRATIONS=1 \
  LABRYS_API_ADDR="127.0.0.1:$PORT" LABRYS_API_TOKEN="$TOKEN" \
  ./target/debug/control-plane >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

export LABRYS_API_URL="http://127.0.0.1:$PORT" LABRYS_API_TOKEN="$TOKEN"
for _ in $(seq 1 60); do
  if $LABRYS_BIN version >/dev/null 2>&1; then break; fi
  sleep 1
done
$LABRYS_BIN version >/dev/null || { echo "smoke-staging: FAIL — API did not start"; tail -n 20 "$SERVER_LOG"; exit 1; }

STAGES_FILE="$(mktemp)"
echo "[]" > "$STAGES_FILE"
record() { # name status detail
  python3 - "$STAGES_FILE" "$1" "$2" "$3" <<'EOF'
import json, sys
path, name, status, detail = sys.argv[1:5]
with open(path) as fh: stages = json.load(fh)
stages.append({"stage": name, "status": status, "tier": "staging", "detail": detail})
with open(path, "w") as fh: json.dump(stages, fh, indent=1)
EOF
}

pass=0; fail=0
run_stage() { # name, expected_exit, cli args...
  local name="$1"; local want="$2"; shift 2
  local detail
  set +e
  detail="$("$@" 2>&1)"
  local got=$?
  set -e
  if [ "$got" = "$want" ]; then
    record "$name" "pass" "exit $got as expected"
    pass=$((pass + 1))
  else
    record "$name" "failed" "exit $got, wanted $want :: $(echo "$detail" | tail -n 3 | tr '\n' ';')"
    fail=$((fail + 1))
  fi
  echo "$detail" | tail -n 1 > /dev/null || true
}

HUMAN=(env LABRYS_ACTOR=human:smoke-operator "$LABRYS_BIN")
AGENT=(env LABRYS_ACTOR="agent:smoke-session:smoke-bot" "$LABRYS_BIN")

run_stage "version" 0 "${HUMAN[@]}" version
IMPORT_OUT="$("${HUMAN[@]}" import --name smoke-app --path "$FIXTURE" --idempotency-key "smoke-import-1")"
APP="$(echo "$IMPORT_OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["resource"]["id"])')"
run_stage "import" 0 "${HUMAN[@]}" import --name smoke-app-dup --path "$FIXTURE" --idempotency-key "smoke-import-1"
run_stage "inspect" 0 "${HUMAN[@]}" inspect --application "$APP"
run_stage "health-never-healthy-without-observation" 0 "${HUMAN[@]}" health --application "$APP"
if "${HUMAN[@]}" health --application "$APP" | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["state"] != "healthy"'; then
  record "health-not-healthy" "pass" "no platform observation, not healthy"
  pass=$((pass + 1))
else
  record "health-not-healthy" "failed" "reported healthy without platform observation"
  fail=$((fail + 1))
fi
run_stage "doctor" 0 "${HUMAN[@]}" doctor --application "$APP"
run_stage "agent-deploy-denied" 1 "${AGENT[@]}" deploy --application "$APP" --environment production --idempotency-key "smoke-deploy-agent"
run_stage "rollback-without-approval-denied" 2 "${HUMAN[@]}" rollback --application "$APP" --target rev-0 --idempotency-key "smoke-rollback-1"
run_stage "logs" 0 "${HUMAN[@]}" logs --application "$APP" --limit 10
run_stage "negotiate" 0 "${HUMAN[@]}" negotiate --protocol-version 1

if docker info >/dev/null 2>&1; then
  record "container-daemon-present" "pass" "docker daemon reachable; delivery proof still requires provider scope"
else
  record "container-daemon-present" "blocked" "docker daemon unreachable; runtime delivery cannot be proven here"
fi
record "runtime-delivery" "blocked" "packaged control plane uses the no-op dispatcher; real execution needs provider scope (see docs/release.md)"

if [ -n "$OUT" ]; then
  mkdir -p "$OUT"
  cp "$STAGES_FILE" "$OUT/smoke.json"
  echo "smoke-staging: stages written to $OUT/smoke.json"
fi
echo "smoke-staging: $pass passed, $fail failed"
[ "$fail" = 0 ]
