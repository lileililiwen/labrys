# Tasks

- [x] Define Application, Origin, Environment, DesiredState, and ObservedState types.
  - Evidence: `crates/labrys-core/src/{application,origin,environment,state}.rs`;
    `cargo test --workspace` 10/10 pass (identity, isolation, desired/observed
    separation scenarios).
- [x] Define stable identifiers, lifecycle timestamps, and audit metadata.
  - Evidence: `crates/labrys-core/src/ids.rs` (`app_`/`env_` prefixed UUIDs with
    `FromStr` roundtrip); `ApplicationStatus`, `created_at`/`updated_at`,
    `AuditMetadata` in `application.rs`; `stable_identifiers_survive_json_roundtrip`.
- [x] Define portable manifest import/export without making `labrys.yaml` mandatory.
  - Evidence: `crates/labrys-core/src/manifest.rs` (`Manifest::from_yaml`,
    `to_yaml`, `import_application(None, ...)` uses DB state as truth);
    `missing_manifest_uses_database_state_then_exports`,
    `manifest_roundtrip_preserves_desired_state_version`.
- [x] Add repository and serialization tests for generated and imported origins.
  - Evidence: `crates/labrys-core/src/repository.rs`
    (`ApplicationRepository` trait + `InMemoryApplicationRepository`);
    `crates/labrys-core/tests/application_foundation.rs` (10 tests);
    `repository_roundtrips_generated_and_imported_origins`,
    `all_six_origins_deserialize_strictly`.
- [x] Verify strict schemas and migration compatibility before handoff.
  - Evidence: `deny_unknown_fields` on all public types;
    `strict_schema_rejects_unknown_fields`,
    `migration_compat_rejects_unsupported_manifest_version`;
    `cargo fmt --check` PASS, `cargo clippy -- -D warnings` PASS,
    `openspec validate --all --strict` 10 passed / 0 failed.
  - Gate: BLOCKED (see `HANDOFF.md`); change must NOT be archived yet.

