import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardView } from "../src/components/DashboardView";

describe("dashboard shows platform truth over agent claims", () => {
  it("agent success plus platform failure shows failure and recovery, never healthy", () => {
    render(
      <DashboardView
        application="demo"
        environment="production"
        agentClaim="deployment completed"
        rows={[
          {
            key: "deploy-1",
            value: "deployment rev-1",
            attribution: "platform",
            state: "failed",
            evidenceSource: "platform:deployment",
            recovery: "inspect the failed probe, then roll back",
          },
        ]}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(/not healthy/i);
    expect(screen.getByLabelText(/agent completion claim/i)).toHaveTextContent(
      /non-authoritative/i,
    );
    expect(screen.getByLabelText(/platform failures/i)).toHaveTextContent(/roll back/i);
    expect(screen.queryByText(/production healthy/i)).not.toBeInTheDocument();
  });

  it("empty evidence renders loading/empty semantics with landmark navigation", () => {
    render(<DashboardView application="demo" environment="staging" rows={[]} />);
    expect(screen.getByLabelText(/dashboard for demo in staging/i)).toBeInTheDocument();
    expect(screen.getByText(/no evidence recorded yet/i)).toBeInTheDocument();
  });
});
