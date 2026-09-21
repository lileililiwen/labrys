"use client";

import { renderSecretReference } from "../lib/secrets";
import type { SecretReference } from "../lib/api";

export function SecretList({ secrets }: { secrets: SecretReference[] }) {
  if (secrets.length === 0) {
    return <p>No secrets bound in this environment.</p>;
  }
  return (
    <ul aria-label="secret references">
      {secrets.map((secret) => (
        <li key={`${secret.environment}:${secret.ref_id}`}>
          {renderSecretReference(secret)}
        </li>
      ))}
    </ul>
  );
}
