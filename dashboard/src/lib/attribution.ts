/**
 * Platform-vs-agent attribution for the operator console.
 *
 * Platform evidence is authoritative. Agent claims are shown separately and
 * can never flip production health or mask a platform failure.
 */

export type AttributionKind = "platform" | "agent";

export interface AttributedRow {
  key: string;
  value: string;
  attribution: AttributionKind;
  /** Platform-observed state; agent rows never carry health. */
  state?: "healthy" | "degraded" | "failed" | "unknown" | "in_progress" | "superseded" | null;
  evidenceSource?: string | null;
  recovery?: string | null;
}

export function attributionLabel(kind: AttributionKind): string {
  return kind === "platform" ? "platform evidence" : "agent claim (non-authoritative)";
}

/**
 * Production is healthy only when platform evidence says healthy AND no
 * platform failure is present — even if the agent stream reports success.
 */
export function isProductionHealthy(rows: AttributedRow[], agentClaim?: string | null): boolean {
  void agentClaim;
  const platformRows = rows.filter((row) => row.attribution === "platform");
  if (platformRows.length === 0) return false;
  if (platformRows.some((row) => row.state === "failed" || row.state === "degraded")) {
    return false;
  }
  return platformRows.some((row) => row.state === "healthy");
}

/** Platform-attributed rows carrying a failure state, with recovery paths. */
export function platformFailures(rows: AttributedRow[]): AttributedRow[] {
  return rows.filter(
    (row) =>
      row.attribution === "platform" &&
      (row.state === "failed" || row.state === "degraded"),
  );
}

/**
 * Summarize health with platform precedence:
 * failed > degraded > in_progress > unknown > healthy.
 */
export function summarizeHealth(states: Array<AttributedRow["state"]>): string {
  const list = states.filter((s): s is NonNullable<AttributedRow["state"]> => s != null);
  if (list.includes("failed")) return "failed";
  if (list.includes("degraded")) return "degraded";
  if (list.includes("in_progress")) return "in_progress";
  if (list.includes("unknown") || list.length === 0) return "unknown";
  if (list.every((s) => s === "healthy")) return "healthy";
  return "unknown";
}
