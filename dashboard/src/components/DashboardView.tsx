"use client";

import {
  isProductionHealthy,
  platformFailures,
  type AttributedRow,
} from "../lib/attribution";
import { AttributionLabel, HealthBadge, RecoveryHint } from "./StatusBadges";

/**
 * Operator view over one application environment.
 *
 * Platform failure always wins over an agent success claim: the banner shows
 * the platform failure and recovery path and never reports healthy production
 * from agent text alone.
 */
export function DashboardView({
  application,
  environment,
  rows,
  agentClaim,
}: {
  application: string;
  environment: string;
  rows: AttributedRow[];
  agentClaim?: string | null;
}) {
  const healthy = isProductionHealthy(rows, agentClaim);
  const failures = platformFailures(rows);

  return (
    <main aria-label={`dashboard for ${application} in ${environment}`}>
      <h1>
        {application} — {environment}
      </h1>
      <section aria-label="production health">
        {healthy ? (
          <p role="status">Production healthy (platform-verified).</p>
        ) : (
          <p role="alert">
            Production is not healthy. Platform evidence takes precedence over agent claims.
          </p>
        )}
        {agentClaim ? (
          <p aria-label="agent completion claim, non-authoritative">
            Agent claim: {agentClaim} (non-authoritative)
          </p>
        ) : null}
      </section>
      {failures.length > 0 ? (
        <section aria-label="platform failures">
          <h2>Platform failures</h2>
          <ul>
            {failures.map((failure) => (
              <li key={failure.key}>
                <HealthBadge state={failure.state} /> {failure.key}: {failure.value}
                <RecoveryHint recovery={failure.recovery} />
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      <section aria-label="evidence rows">
        <h2>Evidence</h2>
        {rows.length === 0 ? (
          <p>No evidence recorded yet.</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th scope="col">Item</th>
                <th scope="col">State</th>
                <th scope="col">Source</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={`${row.attribution}:${row.key}`}>
                  <td>{row.value}</td>
                  <td>
                    <HealthBadge state={row.state} />
                  </td>
                  <td>
                    <AttributionLabel kind={row.attribution} />
                    {row.evidenceSource ? ` — ${row.evidenceSource}` : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </main>
  );
}
