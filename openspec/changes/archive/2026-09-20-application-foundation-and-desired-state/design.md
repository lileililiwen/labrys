# Design

Model Application as the aggregate root. Store origin, source, workspaces,
environments, agent sessions, capabilities, resources, providers, secrets,
previews, deployments, domains, jobs, artifacts, logs, and audit events behind
stable identifiers. Treat the database as authoritative for imported projects;
`labrys.yaml` is an optional export/import manifest.

Environments include development, preview instances, and production. Desired
state is versioned separately from observed state so future controllers can
reconcile without letting an agent claim that an operation completed.

