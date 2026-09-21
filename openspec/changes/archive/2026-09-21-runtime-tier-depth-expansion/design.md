# Design

Each framework integration is a detector plus a convention record behind
the existing `RuntimeAdapter` detect/prepare contract: Django
(settings, manage.py, migrations), FastAPI (app entry, uvicorn,
alembic), Axum (Cargo layout, migration runner, health route
convention), deeper Rust (workspace, sqlx/diesel markers) and Expo
(app.json, routes, dev/build profiles). Detectors emit calibrated
confidence with file evidence so conflicts resolve through the existing
agreement/corroboration/unknown rules instead of new machinery.

`RuntimeConfig` planning gains per-framework production commands that
preserve existing invariants (dev never mints OCI, prod always does;
`dotnet watch`-style dev servers never reused as production commands).
Inspector ordering (`INSPECTION_ORDER`, plugins last) is unchanged; new
framework evidence plugs into the manifest/framework_convention stages.
A matrix test per framework covers detect, prepare, config planning,
and health semantics including dev-3xx versus production-healthy.
