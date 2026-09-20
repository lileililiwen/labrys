# Design

The CLI exposes init, import, inspect, dev, agent, preview, capability,
resources, build, deploy, deployments, logs, health, rollback, and doctor.
The dashboard presents Apps, Overview, Vibe, Code Changes, Preview, Capabilities,
Resources, Runtime, Deployments, Logs, Secrets, and Settings. Plugins cross a
process boundary using JSON-RPC over stdin/stdout in MVP, with SDK contracts for
AgentProvider, CapabilityProvider, BindingProvider, RuntimeProvider,
InspectorProvider, and DeployProvider.

