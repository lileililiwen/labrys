"use client";

import { attributionLabel, type AttributedRow } from "../lib/attribution";

const STATE_LABELS: Record<string, string> = {
  healthy: "Healthy",
  degraded: "Degraded",
  failed: "Failed",
  unknown: "Unknown",
  in_progress: "In progress",
  superseded: "Superseded",
};

export function HealthBadge({ state }: { state: AttributedRow["state"] }) {
  const label = STATE_LABELS[state ?? "unknown"] ?? "Unknown";
  return (
    <span role="status" aria-label={`platform health: ${label.toLowerCase()}`}>
      {label}
    </span>
  );
}

export function AttributionLabel({ kind }: { kind: AttributedRow["attribution"] }) {
  return (
    <span aria-label={kind === "platform" ? "platform-owned evidence" : "agent claim, not authoritative"}>
      {attributionLabel(kind)}
    </span>
  );
}

export function RecoveryHint({ recovery }: { recovery?: string | null }) {
  if (!recovery) return null;
  return <p role="note">Recovery: {recovery}</p>;
}
