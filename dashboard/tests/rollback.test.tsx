import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import {
  DATABASE_DATA_WARNING,
  maySubmitRollback,
  validateRollbackConfirmation,
} from "../src/lib/rollback";
import { RollbackDialog, type RollbackSubmit } from "../src/components/RollbackDialog";
import { DashboardApiClient } from "../src/lib/api";

describe("rollback approval boundary", () => {
  it("shows the database-data warning text", () => {
    expect(DATABASE_DATA_WARNING.toLowerCase()).toContain("does not restore database data");
  });

  it("blocks submission until warning, scope, target, and approver are present", () => {
    const base = { target: "rev-2", approver: "human:op", acknowledgedWarning: true, confirmedScope: true };
    expect(maySubmitRollback(base)).toBe(true);
    expect(maySubmitRollback({ ...base, approver: "" })).toBe(false);
    expect(maySubmitRollback({ ...base, acknowledgedWarning: false })).toBe(false);
    expect(maySubmitRollback({ ...base, confirmedScope: false })).toBe(false);
    expect(validateRollbackConfirmation({ ...base, target: "" })).toMatch(/target/);
  });

  it("dialog renders the warning and refuses to submit without approval", async () => {
    const onSubmit = vi.fn(async () => {});
    render(<RollbackDialog targets={["rev-2"]} onSubmit={onSubmit} />);
    expect(screen.getByRole("note", { name: /database data warning/i })).toHaveTextContent(
      /does not restore database data/i,
    );
    fireEvent.click(screen.getByRole("button", { name: /request rollback/i }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("dialog submits once the operator confirms scope and named approval", async () => {
    const onSubmit = vi.fn(async () => {});
    render(<RollbackDialog targets={["rev-2"]} onSubmit={onSubmit} />);
    fireEvent.change(screen.getByRole("textbox", { name: /named approver/i }), {
      target: { value: "human:operator" },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: /i understand rollback/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /confirm rollback scope/i }));
    fireEvent.click(screen.getByRole("button", { name: /request rollback/i }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const [submitted] = onSubmit.mock.calls[0] as unknown as [RollbackSubmit];
    expect(submitted).toMatchObject({
      target: "rev-2",
      approver: "human:operator",
    });
  });

  it("client refuses rollback without a named approver before any fetch", async () => {
    const fetchImpl = vi.fn(async () => new Response("{}", { status: 500 }));
    const client = new DashboardApiClient({
      baseUrl: "http://127.0.0.1:1",
      token: "test-token-12345678",
      actor: "human:op",
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    await expect(client.rollback("app", "rev-2", "", "key-1")).rejects.toThrow(
      /named approval/,
    );
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
