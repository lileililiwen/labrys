# Design

ApplicationController, CapabilityController, ResourceController,
PreviewController, and DeploymentController read desired and observed state,
calculate diffs, and enqueue idempotent jobs. Resources move through requested,
provisioning, ready, degraded, failed, deleting, and deleted. Health uses the
shared Unknown, Provisioning, Healthy, Degraded, Failed, and Paused model.
PostgreSQL jobs and a Rust worker are the MVP queue boundary.

