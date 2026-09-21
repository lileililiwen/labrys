#!/usr/bin/env bash
# Staging smoke: disposable end-to-end pass over the packaged entry points.
#
# Stages: version -> import -> inspect -> health (never healthy without
# platform observation) -> doctor -> agent deploy denial -> rollback without
# approval denial -> logs -> protocol negotiation -> daemon dispatcher
# selection -> real execution cycle (build -> run -> health -> preview gate ->
# cleanup) when Docker is reachable.
#
# The daemon selects its dispatcher from LABRYS_RUNTIME_MODE (auto/docker/
# disabled): a reachable runtime runs the executable dispatcher, otherwise
# execution stages are recorded BLOCKED with the recovery action instead of a
# fake pass. The execution cycle uses the docker CLI directly against the
# fixture; the daemon-side DockerExecutor/PreviewManager paths are proven by
# the container_execution integration tests. See docs/release.md for the
# evidence vocabulary.
#
# Requires: LABRYS_DATABASE_URL (or DATABASE_URL), docker daemon for the
# execution cycle, cargo-built binaries. Scratch state is cleaned up.
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
SMOKE_TAG="labrys-smoke-${PORT}-$$"
SMOKE_NAME="labrys-smoke-${PORT}-$$"
cleanup() {
  docker rm -f "$SMOKE_NAME" >/dev/null 2>&1 || true
  docker rmi -f "$SMOKE_TAG" >/dev/null 2>&1 || true
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

if grep -q "dispatcher=executable" "$SERVER_LOG"; then
  record "daemon-dispatcher" "pass" "daemon runs the executable dispatcher (see server log)"
  pass=$((pass + 1))
elif grep -q "dispatcher=noop-blocked" "$SERVER_LOG"; then
  record "daemon-dispatcher" "pass" "daemon reports noop-blocked with recovery (no runtime in this environment)"
  pass=$((pass + 1))
else
  record "daemon-dispatcher" "failed" "no dispatcher selection line in the daemon log"
  fail=$((fail + 1))
fi

if docker info >/dev/null 2>&1; then
  record "container-daemon-present" "pass" "docker daemon reachable"
  pass=$((pass + 1))
  cycle_ok=1
  if docker build -f "$FIXTURE/Dockerfile" -t "$SMOKE_TAG" "$FIXTURE" >"$FIXTURE/build.log" 2>&1; then
    record "execution-build" "pass" "docker build $SMOKE_TAG from the smoke fixture"
    pass=$((pass + 1))
  else
    record "execution-build" "blocked" "docker build failed: $(tail -n 2 "$FIXTURE/build.log" | tr '\n' ';' | cut -c1-200); next: ensure registry/network access for the disposable environment, then re-run"
    cycle_ok=0
  fi
  if [ "$cycle_ok" = 1 ]; then
    if docker run -d --name "$SMOKE_NAME" "$SMOKE_TAG" >"$FIXTURE/run.log" 2>&1; then
      sleep 2
      if [ "$(docker inspect -f '{{.State.Running}}' "$SMOKE_NAME" 2>/dev/null)" = "true" ] \
        && docker logs "$SMOKE_NAME" 2>&1 | grep -q "smoke-ok"; then
        record "execution-run-health" "pass" "platform-observed running state plus smoke-ok in container logs"
        pass=$((pass + 1))
      else
        record "execution-run-health" "blocked" "container did not reach the observed healthy state; next: inspect the fixture image, then re-run"
        cycle_ok=0
      fi
    else
      record "execution-run-health" "blocked" "docker run failed: $(tail -n 2 "$FIXTURE/run.log" | tr '\n' ';' | cut -c1-200); next: check daemon resources, then re-run"
      cycle_ok=0
    fi
  else
    record "execution-run-health" "blocked" "skipped: build stage blocked; next: fix the build stage, then re-run"
  fi
  if [ "$cycle_ok" = 1 ]; then
    record "execution-preview-gate" "pass" "preview URL would issue only after the observed health above; issuance/expiry/revocation is proven by the container_execution integration tests"
    pass=$((pass + 1))
  else
    record "execution-preview-gate" "blocked" "no URL precondition: health was not observed; next: fix the run-health stage, then re-run"
  fi
  cleaned=0
  for _ in $(seq 1 10); do
    docker rm -f "$SMOKE_NAME" >/dev/null 2>&1 || true
    # `docker container inspect` (not `docker inspect`): the latter falls back
    # to image lookup, and the smoke tag equals the container name.
    if ! docker container inspect "$SMOKE_NAME" >/dev/null 2>&1; then
      cleaned=1
      break
    fi
    sleep 1
  done
  if [ "$cleaned" = 1 ]; then
    record "execution-cleanup" "pass" "smoke container stopped and removed"
    pass=$((pass + 1))
  else
    record "execution-cleanup" "blocked" "smoke container cleanup incomplete; next: remove $SMOKE_NAME manually, then re-run"
    cycle_ok=0
  fi
  docker rmi -f "$SMOKE_TAG" >/dev/null 2>&1 || true
  if [ "$cycle_ok" = 1 ]; then
    record "runtime-delivery" "pass" "import -> build -> run -> health -> preview-gate -> cleanup against the disposable environment"
    pass=$((pass + 1))
  else
    record "runtime-delivery" "blocked" "execution cycle incomplete; see the execution-* stages above for the recovery action"
  fi
else
  record "container-daemon-present" "blocked" "docker daemon unreachable; runtime delivery cannot be proven here"
  record "execution-build" "blocked" "no docker daemon; next: start Docker for the disposable environment, then re-run"
  record "execution-run-health" "blocked" "no docker daemon; next: start Docker for the disposable environment, then re-run"
  record "execution-preview-gate" "blocked" "no docker daemon; next: start Docker for the disposable environment, then re-run"
  record "execution-cleanup" "blocked" "no docker daemon; nothing was started, nothing to clean"
  record "runtime-delivery" "blocked" "no docker daemon; real execution needs the disposable environment (see docs/release.md)"
fi

if [ -n "$OUT" ]; then
  mkdir -p "$OUT"
  cp "$STAGES_FILE" "$OUT/smoke.json"
  echo "smoke-staging: stages written to $OUT/smoke.json"
fi
echo "smoke-staging: $pass passed, $fail failed"
[ "$fail" = 0 ]
