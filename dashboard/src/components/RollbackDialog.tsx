"use client";

import { useState } from "react";
import {
  DATABASE_DATA_WARNING,
  validateRollbackConfirmation,
} from "../lib/rollback";

export interface RollbackSubmit {
  target: string;
  approver: string;
  idempotencyKey: string;
}

export function RollbackDialog({
  targets,
  onSubmit,
}: {
  targets: string[];
  onSubmit: (input: RollbackSubmit) => Promise<void>;
}) {
  const [target, setTarget] = useState(targets[0] ?? "");
  const [approver, setApprover] = useState("");
  const [acknowledgedWarning, setAcknowledgedWarning] = useState(false);
  const [confirmedScope, setConfirmedScope] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState(false);

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    const problem = validateRollbackConfirmation({
      target,
      approver,
      acknowledgedWarning,
      confirmedScope,
    });
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    await onSubmit({
      target,
      approver,
      idempotencyKey: `rollback-${target}-${Date.now()}`,
    });
    setSubmitted(true);
  }

  return (
    <form onSubmit={handleSubmit} aria-label="deployment rollback">
      <div role="note" aria-label="database data warning">
        {DATABASE_DATA_WARNING}
      </div>
      <label>
        Rollback target
        <select
          aria-label="rollback target"
          value={target}
          onChange={(event) => setTarget(event.target.value)}
        >
          {targets.map((revision) => (
            <option key={revision} value={revision}>
              {revision}
            </option>
          ))}
        </select>
      </label>
      <label>
        Named approver
        <input
          aria-label="named approver"
          value={approver}
          onChange={(event) => setApprover(event.target.value)}
          placeholder="human:operator-name"
        />
      </label>
      <label>
        <input
          type="checkbox"
          checked={acknowledgedWarning}
          onChange={(event) => setAcknowledgedWarning(event.target.checked)}
        />
        I understand rollback does not restore database data
      </label>
      <label>
        <input
          type="checkbox"
          checked={confirmedScope}
          onChange={(event) => setConfirmedScope(event.target.checked)}
        />
        Confirm rollback scope
      </label>
      {error ? <p role="alert">{error}</p> : null}
      {submitted ? <p role="status">Rollback submitted for approval.</p> : null}
      <button type="submit">Request rollback</button>
    </form>
  );
}
