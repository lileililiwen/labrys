#!/usr/bin/env bash
# Release packaging: versioned, reproducible, inspectable artifacts.
#
# Emits into --out (default dist/):
#   control-plane, labrys            versioned binaries (copies of the release build)
#   dashboard/                       static operator-console bundle (next build output)
#   SHA256SUMS                       checksums over every artifact file
#   release-manifest.json            source rev, versions, migration version,
#                                    dependency metadata, supported tiers
#   sbom.json                        lockfile-derived package inventory (hook for
#                                    a real SBOM signer when available)
#   signatures/                      cosign signatures when cosign is installed;
#                                    otherwise an UNSIGNED marker (never a fake signature)
#
# The manifest makes scope explicit; it never claims production readiness.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT="dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    *) echo "usage: package.sh [--out DIR]" >&2; exit 2 ;;
  esac
done

VERSION="${LABRYS_VERSION:-$(git describe --tags --always --dirty 2>/dev/null || echo 0.1.0-dev)}"
REV="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
mkdir -p "$OUT"

echo "package: building release binaries ($VERSION @ $REV)"
cargo build --quiet --release --bin control-plane --bin labrys
cp ./target/release/control-plane "$OUT/control-plane"
cp ./target/release/labrys "$OUT/labrys"

echo "package: building dashboard bundle"
npm ci --prefix dashboard --no-audit --no-fund >/dev/null 2>&1 || npm install --prefix dashboard --no-audit --no-fund
npm run build --prefix dashboard
rm -rf "$OUT/dashboard"
mkdir -p "$OUT/dashboard"
cp -r dashboard/.next/standalone "$OUT/dashboard/standalone" 2>/dev/null || cp -r dashboard/.next "$OUT/dashboard/dot-next"

API_VERSION="$(grep -aE 'pub const API_VERSION' crates/labrys-control-plane/src/api.rs | sed -E 's/.*= *([0-9]+).*/\1/')"
AGENT_VERSION="$(grep -aE 'pub const AGENT_PROTOCOL_VERSION' crates/labrys-core/src/agent.rs | sed -E 's/.*= *([0-9]+).*/\1/')"
PLUGIN_VERSION="$(grep -aE 'pub const PLUGIN_PROTOCOL_VERSION' crates/labrys-core/src/plugin.rs | sed -E 's/.*= *([0-9]+).*/\1/')"
SCHEMA_VERSION="$(grep -aE 'pub const CONTROL_PLANE_SCHEMA_VERSION' crates/labrys-control-plane/src/lib.rs | sed -E 's/.*= *([0-9]+).*/\1/')"
MIGRATION_VERSION="$(ls crates/labrys-control-plane/migrations/*.sql | sed -E 's/.*\/([0-9]+)_.*/\1/' | sort -n | tail -n 1)"

# SBOM hook: exact lockfile inventory; a signing SBOM tool can replace this
# file when available, but the package never ships without provenance.
python3 - "$OUT/sbom.json" <<'EOF'
import json, sys, tomllib
out = sys.argv[1]
with open("Cargo.lock", "rb") as fh:
    lock = tomllib.load(fh)
packages = [{"name": p["name"], "version": p["version"], "source": p.get("source", "local")} for p in lock.get("package", [])]
with open(out, "w") as fh:
    json.dump({"tool": "cargo-lock-inventory", "packages": packages}, fh, indent=1)
print(f"sbom: {len(packages)} packages inventoried")
EOF

python3 - "$OUT/release-manifest.json" <<EOF
import json, sys
manifest = {
    "name": "labrys",
    "version": "$VERSION",
    "source_revision": "$REV",
    "protocols": {"api": $API_VERSION, "agent": $AGENT_VERSION, "plugin": $PLUGIN_VERSION},
    "control_plane_schema_version": $SCHEMA_VERSION,
    "migration_version": "$MIGRATION_VERSION",
    "rust_toolchain": "$(rustc --version)",
    "node_runtime": "$(node --version)",
    "supported_runtime_tiers": ["tier-0-generic-dockerfile", "tier-1-language-detection", "tier-2-framework-layout"],
    "supported_provider_scope": ["local-test-doubles", "approval-gated-domain-delivery"],
    "production_proof": False,
}
with open(sys.argv[1], "w") as fh:
    json.dump(manifest, fh, indent=1)
print("manifest written")
EOF

# Signing hook: real signatures only; otherwise an explicit UNSIGNED marker.
mkdir -p "$OUT/signatures"
if command -v cosign >/dev/null; then
  for bin in control-plane labrys; do
    cosign sign-blob --yes --output-signature "$OUT/signatures/$bin.sig" "$OUT/$bin"
  done
  echo "signatures: cosign blobs signed"
else
  echo "UNSIGNED: no cosign available at package time; sign dist/ before publishing" > "$OUT/signatures/UNSIGNED"
  echo "signatures: BLOCKED (cosign unavailable) — UNSIGNED marker recorded"
fi

(cd "$OUT" && find . -type f -not -name SHA256SUMS | sort | xargs sha256sum > SHA256SUMS)
echo "package: PASS — artifacts in $OUT ($(wc -l < "$OUT/SHA256SUMS" | tr -d ' ') files checksummed)"
