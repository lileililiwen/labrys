# Design

Deployment records revision, build, image, runtime, environment, resources,
endpoint, health, logs, and rollback target. The production path builds an OCI
image, starts it under a deployment runtime, waits for platform health, and
only then marks the revision healthy. Source, deployment, and configuration
rollback are separate operations; database data rollback is never implied by a
deployment rollback.

