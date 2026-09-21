# Design

Use a typed API client and server-side/session-aware authorization boundary;
the browser never receives secret values. Dashboard sections map to the
existing `DashboardSection` contract and show loading, empty, stale,
degraded, failed, and permission-denied states. Mutations create approval or
job workflows and refresh from platform observations rather than optimistic
healthy state.

Agent and platform attribution use different visual treatments and accessible
labels. Health summaries prioritize platform-owned evidence. Destructive
actions require an explicit confirmation containing scope, warning, and named
approval. Component and browser tests cover responsive layout, keyboard and
screen-reader behavior, stale data, API failures, and secret redaction.
