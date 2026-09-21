import { DashboardView } from "../components/DashboardView";

/**
 * Overview route. Data loads from the control-plane API at request time;
 * the API and database remain authoritative and the page holds no secret
 * values. This static fallback renders loading/empty semantics until the
 * client hydrates with platform observations.
 */
export default function OverviewPage() {
  return (
    <DashboardView
      application="labrys-app"
      environment="production"
      rows={[]}
      agentClaim={null}
    />
  );
}
