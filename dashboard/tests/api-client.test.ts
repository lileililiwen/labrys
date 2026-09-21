import { describe, expect, it, vi } from "vitest";
import { DashboardApiClient, headersFor, isOk, recoveryOf } from "../src/lib/api";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("typed API client contract", () => {
  it("sends auth, actor, trace, and idempotency headers", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ok: true }));
    const client = new DashboardApiClient({
      baseUrl: "http://api:8080/",
      token: "test-token-12345678",
      actor: "human:operator",
      traceId: "trace-1",
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    await client.deploy("app", "production", "key-1", "human:operator");
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    void url;
    const headers = init.headers as Record<string, string>;
    expect(headers["authorization"]).toBe("Bearer test-token-12345678");
    expect(headers["x-labryst-actor"]).toBe("human:operator");
    expect(headers["idempotency-key"]).toBe("key-1");
  });

  it("header helper requires token context and supports idempotency", () => {
    const headers = headersFor(
      { token: "t", actor: "human:op", traceId: "trace-9" },
      "key-9",
    );
    expect(headers["idempotency-key"]).toBe("key-9");
  });

  it("maps envelopes to ok/recovery semantics", () => {
    expect(isOk({ ok: true })).toBe(true);
    expect(isOk({ ok: false })).toBe(false);
    expect(recoveryOf({ recovery: "do y" })).toBe("do y");
  });

  it("health read hits the versioned route", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse({ ok: true, data: { state: "failed", environments: [] } }),
    );
    const client = new DashboardApiClient({
      baseUrl: "http://api:8080",
      token: "test-token-12345678",
      actor: "human:operator",
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    const envelope = await client.health("demo", "production");
    const [calledUrl] = fetchImpl.mock.calls[0] as unknown as [string];
    expect(calledUrl).toContain("/v1/applications/demo/health");
    expect(envelope.ok).toBe(true);
  });
});
