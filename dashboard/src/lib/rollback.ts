/**
 * Rollback confirmation boundary.
 *
 * Mirrors `labrys_core::deployment::DATABASE_DATA_WARNING`: a deployment
 * rollback never restores database data, and it never begins without a
 * granted, named human approval accepted by the API.
 */

export const DATABASE_DATA_WARNING =
  "Deployment rollback does not restore database data. " +
  "Data migrations in the delta require separate review before rollback.";

export interface RollbackConfirmation {
  target: string;
  approver: string;
  acknowledgedWarning: boolean;
  confirmedScope: boolean;
}

export function validateRollbackConfirmation(input: RollbackConfirmation): string | null {
  if (!input.target.trim()) return "select a rollback target revision";
  if (!input.approver.trim()) return "rollback requires a named approver";
  if (!input.acknowledgedWarning) return "acknowledge the database-data warning first";
  if (!input.confirmedScope) return "confirm the rollback scope first";
  return null;
}

/** Guard: the UI must not call the API until validation passes. */
export function maySubmitRollback(input: RollbackConfirmation): boolean {
  return validateRollbackConfirmation(input) === null;
}
