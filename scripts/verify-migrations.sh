#!/usr/bin/env bash
# Migration check: applies every versioned migration to a scratch database
# through the real control-plane binary and verifies the applied set matches
# the files on disk.
#
# Requires: LABRYS_DATABASE_URL (or DATABASE_URL) pointing at a PostgreSQL
# instance where scratch databases may be created, plus `psql` and cargo.
# Without a database this check is BLOCKED, never passed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DB_URL="${LABRYS_DATABASE_URL:-${DATABASE_URL:-}}"
if [ -z "$DB_URL" ]; then
  echo "verify-migrations: BLOCKED — no database (set LABRYS_DATABASE_URL)" >&2
  echo "next action: start the disposable environment (docker compose -f compose.test.yml up -d)" >&2
  exit 3
fi
command -v psql >/dev/null || { echo "verify-migrations: BLOCKED — psql not installed" >&2; exit 3; }

# Derive an admin URL (postgres db) and a scratch database name.
ADMIN_URL="$(echo "$DB_URL" | sed -E 's|/[^/]*$|/postgres|')"
SCRATCH="labrys_migrate_check"
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $SCRATCH;" >/dev/null
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "CREATE DATABASE $SCRATCH;" >/dev/null
SCRATCH_URL="$(echo "$DB_URL" | sed -E "s|/[^/]*$|/$SCRATCH|")"

cargo build --quiet --bin control-plane
# The binary applies migrations at startup, then serves workers until it
# receives a shutdown signal; a timeout kill after the marker is expected.
set +e
OUT="$(LABRYS_DATABASE_URL="$SCRATCH_URL" LABRYS_RUN_MIGRATIONS=1 timeout 90 ./target/debug/control-plane 2>&1)"
status=$?
set -e
echo "$OUT" | grep -q "migrations applied" || {
  echo "verify-migrations: FAIL — binary did not report applied migrations" >&2
  echo "$OUT" | tail -n 20 >&2
  exit 1
}

expected="$(ls crates/labrys-control-plane/migrations/*.sql | wc -l | tr -d ' ')"
applied="$(psql "$SCRATCH_URL" -tAX -c "SELECT count(*) FROM _sqlx_migrations WHERE success;")"
version="$(psql "$SCRATCH_URL" -tAX -c "SELECT max(version) FROM _sqlx_migrations WHERE success;")"
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -c "DROP DATABASE $SCRATCH;" >/dev/null

if [ "$applied" != "$expected" ]; then
  echo "verify-migrations: FAIL — applied $applied migrations, expected $expected files" >&2
  exit 1
fi
echo "verify-migrations: PASS ($applied/$expected applied, schema version $version)"
echo "MIGRATION_VERSION=$version"
