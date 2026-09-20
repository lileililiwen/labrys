# Design

Expose create-session, prompt, interrupt, and resume through a versioned
AgentBackend protocol. Normalize events such as planning, file changes,
commands, capability requests, builds, previews, deployments, verification,
errors, and completion. Enforce policy at the platform boundary, not inside a
third-party adapter. Use Platform API → framework tool → generic tool → raw
shell precedence. Native Agent implements Plan → Act → Observe → Verify → Retry.

