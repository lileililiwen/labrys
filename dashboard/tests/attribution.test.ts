import { describe, expect, it } from "vitest";
import {
  isProductionHealthy,
  platformFailures,
  summarizeHealth,
} from "../src/lib/attribution";

describe("platform-owned application truth", () => {
  it("agent success never masks a platform deployment failure", () => {
    const healthy = isProductionHealthy(
      [
        {
          key: "deploy-1",
          value: "deployment 1",
          attribution: "platform",
          state: "failed",
          evidenceSource: "platform:deployment",
          recovery: "inspect the failed probe, then roll back",
        },
      ],
      "deployment completed",
    );
    expect(healthy).toBe(false);
  });

  it("healthy requires platform evidence, not an agent claim", () => {
    expect(
      isProductionHealthy(
        [{ key: "a", value: "agent says ok", attribution: "agent", state: null }],
        "deployment completed",
      ),
    ).toBe(false);
    expect(
      isProductionHealthy(
        [{ key: "probe", value: "prod", attribution: "platform", state: "healthy" }],
        null,
      ),
    ).toBe(true);
  });

  it("exposes platform failures with recovery paths", () => {
    const failures = platformFailures([
      { key: "d1", value: "bad", attribution: "platform", state: "failed", recovery: "roll back" },
      { key: "a1", value: "ok", attribution: "agent", state: null },
    ]);
    expect(failures.map((f) => f.key)).toEqual(["d1"]);
  });

  it("summarizes with platform precedence failed > degraded > in_progress", () => {
    expect(summarizeHealth(["healthy", "failed"])).toBe("failed");
    expect(summarizeHealth(["healthy", "degraded"])).toBe("degraded");
    expect(summarizeHealth(["healthy", "in_progress"])).toBe("in_progress");
    expect(summarizeHealth([])).toBe("unknown");
    expect(summarizeHealth(["healthy"])).toBe("healthy");
  });
});
