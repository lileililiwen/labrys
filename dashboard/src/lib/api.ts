/**
 * Typed client for the versioned control-plane API (API_VERSION = 1).
 *
 * The dashboard is never a source of truth: every view reads through these
 * envelopes and refreshes from platform observations after a mutation.
 * The browser never receives secret values — only references.
 */

export const API_VERSION = 1;

/** Machine-readable envelope returned by every API route. */
export interface ApiEnvelope<T = unknown> {
  ok: boolean;
  action: string;
  actor: string;
  trace_id: string;
  resource?: { kind: string; id: string } | null;
  data?: T | null;
  recovery?: string | null;
  protocol?: { api: number; agent: number; plugin: number } | null;
}

export interface HealthEnvironmentEntry {
  environment: string;
  state: "healthy" | "degraded" | "failed" | "in_progress" | "unknown" | "superseded";
  deployment?: string;
  digest?: string | null;
  job?: { id: string; state: string } | null;
  detail?: string;
}

export interface HealthData {
  state: string;
  environments: HealthEnvironmentEntry[];
}

export interface DeploymentItem {
  id: string;
  environment: string;
  phase: string;
  revision_sha: string;
  digest?: string | null;
  rollback_target?: string | null;
}

export interface LogItem {
  sequence: number;
  source: "agent" | "build" | "runtime" | "deployment" | "capability" | "resource";
  level: string;
  message: string;
  trace: string;
}

export interface DoctorFinding {
  code: string;
  severity: "info" | "warning" | "error";
  message: string;
  recovery?: string;
}

/** Secret metadata: reference, scope, and rotation state. Never a value. */
export interface SecretReference {
  ref_id: string;
  version_hint: string;
  environment: string;
  rotation_state?: "current" | "rotating" | "stale";
}

export interface DashboardClientOptions {
  baseUrl: string;
  token: string;
  actor: string;
  traceId?: string;
  fetchImpl?: typeof fetch;
}

export function headersFor(
  opts: Pick<DashboardClientOptions, "token" | "actor" | "traceId">,
  idempotencyKey?: string,
): Record<string, string> {
  const headers: Record<string, string> = {
    authorization: `Bearer ${opts.token}`,
    "x-labryst-actor": opts.actor,
    "x-trace-id": opts.traceId ?? `trace-${Math.random().toString(36).slice(2)}`,
  };
  if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
  return headers;
}

async function readEnvelope<T>(
  response: Response,
  path: string,
): Promise<ApiEnvelope<T>> {
  let body: ApiEnvelope<T>;
  try {
    body = (await response.json()) as ApiEnvelope<T>;
  } catch (error) {
    throw new Error(`response from ${path} is not JSON: ${String(error)}`);
  }
  // Authentication/authorization failures are actionable exceptions, not
  // silent envelopes: the server recovery names the token/actor fix.
  if (response.status === 401 || response.status === 403 || response.status === 429) {
    const recovery =
      typeof body.recovery === "string" && body.recovery.trim()
        ? body.recovery
        : "check the dashboard API token and actor, then retry";
    throw new Error(`API ${response.status} from ${path}: ${recovery}`);
  }
  return body;
}

/** Minimal typed client: reads are GET, writes are idempotent POSTs. */
export class DashboardApiClient {
  private baseUrl: string;
  private token: string;
  private actor: string;
  private traceId: string;
  private fetchImpl: typeof fetch;

  constructor(opts: DashboardClientOptions) {
    if (!opts.baseUrl) throw new Error("API base URL is required");
    if (!opts.token) throw new Error("API token is required (LABRYS_API_TOKEN)");
    this.baseUrl = opts.baseUrl.replace(/\/$/, "");
    this.token = opts.token;
    this.actor = opts.actor;
    this.traceId = opts.traceId ?? `trace-${Math.random().toString(36).slice(2)}`;
    this.fetchImpl = opts.fetchImpl ?? fetch;
  }

  private headers(idempotencyKey?: string): Record<string, string> {
    return headersFor(
      { token: this.token, actor: this.actor, traceId: this.traceId },
      idempotencyKey,
    );
  }

  private async get<T>(path: string, query = ""): Promise<ApiEnvelope<T>> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}${query}`, {
      headers: this.headers(),
    });
    return readEnvelope<T>(response, path);
  }

  private async post<T>(
    path: string,
    body: unknown,
    idempotencyKey?: string,
  ): Promise<ApiEnvelope<T>> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: "POST",
      headers: { ...this.headers(idempotencyKey), "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    return readEnvelope<T>(response, path);
  }

  version(): Promise<ApiEnvelope<{ api: number }>> {
    return this.get("/v1/version");
  }

  inspect(application: string): Promise<ApiEnvelope<unknown>> {
    return this.get(`/v1/applications/${application}/inspect`);
  }

  health(application: string, environment?: string): Promise<ApiEnvelope<HealthData>> {
    const query = environment ? `?environment=${encodeURIComponent(environment)}` : "";
    return this.get<HealthData>(`/v1/applications/${application}/health`, query);
  }

  deployments(application: string, environment?: string): Promise<ApiEnvelope<{ items: DeploymentItem[] }>> {
    const query = environment ? `?environment=${encodeURIComponent(environment)}` : "";
    return this.get(`/v1/applications/${application}/deployments`, query);
  }

  logs(application: string, source?: string): Promise<ApiEnvelope<{ items: LogItem[] }>> {
    const query = source ? `?source=${encodeURIComponent(source)}` : "";
    return this.get(`/v1/applications/${application}/logs`, query);
  }

  doctor(application: string): Promise<ApiEnvelope<{ findings: DoctorFinding[] }>> {
    return this.get(`/v1/applications/${application}/doctor`);
  }

  job(jobId: string): Promise<ApiEnvelope<{ state: string }>> {
    return this.get(`/v1/jobs/${jobId}`);
  }

  deploy(
    application: string,
    environment: string,
    idempotencyKey: string,
    approval?: string,
  ): Promise<ApiEnvelope<unknown>> {
    return this.post(
      `/v1/applications/${application}/deploy`,
      approval ? { environment, approval: { approver: approval } } : { environment },
      idempotencyKey,
    );
  }

  rollback(
    application: string,
    target: string,
    approver: string,
    idempotencyKey: string,
  ): Promise<ApiEnvelope<unknown>> {
    if (!approver || !approver.trim()) {
      return Promise.reject(new Error("rollback requires a named approval"));
    }
    return this.post(
      `/v1/applications/${application}/rollback`,
      { target, approval: { approver } },
      idempotencyKey,
    );
  }
}

/** Exit semantics mirror the CLI: ok flag decides success. */
export function isOk(envelope: { ok: boolean }): boolean {
  return envelope.ok === true;
}

export function recoveryOf(envelope: {
  recovery?: string | null;
  data?: unknown;
}): string | null {
  if (typeof envelope.recovery === "string" && envelope.recovery) return envelope.recovery;
  const data = envelope.data as { recovery?: unknown } | null | undefined;
  return typeof data?.recovery === "string" ? data.recovery : null;
}
