/**
 * Secret-reference rendering boundary.
 *
 * The dashboard displays references, versions, scope, and rotation state.
 * Secret values must never reach the browser: this module refuses any row
 * that carries a value-shaped field and strips values from free text.
 */

import type { SecretReference } from "./api";

const VALUE_KEYS = ["value", "secret", "plaintext", "password", "token_value"];

export function assertSecretReferenceSafe(row: Record<string, unknown>): void {
  for (const key of VALUE_KEYS) {
    if (key in row && row[key] !== undefined && row[key] !== null && row[key] !== "") {
      throw new Error(`secret value must not enter dashboard UI state (field '${key}')`);
    }
  }
}

/** Render a secret row as reference-only text; throws if a value is present. */
export function renderSecretReference(row: SecretReference): string {
  assertSecretReferenceSafe(row as unknown as Record<string, unknown>);
  const rotation = row.rotation_state ? ` [${row.rotation_state}]` : "";
  return `${row.ref_id} (v${row.version_hint}, ${row.environment})${rotation}`;
}

/** Redact known secret references from free text before display. */
export function redactSecretsFromText(text: string, known: string[]): string {
  let out = text;
  for (const secret of known) {
    if (secret) out = out.split(secret).join("[redacted]");
  }
  return out;
}
