use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use labrys_core::{
    ApplicationId, EnvironmentId, ObservationSource, Resource, ResourceId, ResourcePhase,
};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{json_get, json_get_opt, to_value, to_value_opt};

/// Durable store for platform-owned resources and their lifecycle evidence.
///
/// A resource is bound to exactly one application and (optionally) one
/// environment, so a preview resource never appears under production. Readiness
/// is only ever written from a platform-owned observation; the store itself
/// trusts no caller-provided "ready" flag beyond the phase it is handed.
#[derive(Clone)]
pub struct PgResourceStore {
    pool: PgPool,
}

impl PgResourceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn save(
        &self,
        resource: &Resource,
        application_id: &ApplicationId,
        environment_id: Option<&EnvironmentId>,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO resources
                 (id, application_id, environment_id, kind, desired_version, phase,
                  generation, failure, transitions, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
               ON CONFLICT (id) DO UPDATE SET
                 kind = EXCLUDED.kind,
                 desired_version = EXCLUDED.desired_version,
                 phase = EXCLUDED.phase,
                 generation = EXCLUDED.generation,
                 failure = EXCLUDED.failure,
                 transitions = EXCLUDED.transitions,
                 updated_at = EXCLUDED.updated_at"#,
        )
        .bind(resource.id.to_string())
        .bind(application_id.to_string())
        .bind(environment_id.map(|e| e.to_string()))
        .bind(&resource.kind)
        .bind(resource.desired_version as i64)
        .bind(phase_name(resource.phase))
        .bind(resource.generation as i32)
        .bind(to_value_opt(&resource.failure)?)
        .bind(to_value(&resource.transitions)?)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &ResourceId) -> Result<Resource> {
        let row = sqlx::query("SELECT * FROM resources WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))?;
        resource_from_row(&row)
    }

    pub async fn list_for_application(
        &self,
        application_id: &ApplicationId,
    ) -> Result<Vec<Resource>> {
        let rows =
            sqlx::query("SELECT * FROM resources WHERE application_id = $1 ORDER BY kind, id")
                .bind(application_id.to_string())
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(resource_from_row).collect()
    }

    pub async fn list_for_environment(
        &self,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<Resource>> {
        let rows =
            sqlx::query("SELECT * FROM resources WHERE environment_id = $1 ORDER BY kind, id")
                .bind(environment_id.to_string())
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(resource_from_row).collect()
    }

    /// Persists a platform-owned phase observation, appending its transition
    /// evidence. `reason` is stored as supplied (already redacted by callers).
    pub async fn observe_phase(
        &self,
        id: &ResourceId,
        phase: ResourcePhase,
        source: ObservationSource,
        reason: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<Resource> {
        let mut resource = self.get(id).await?;
        resource.observe(phase, source, reason, at)?;
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query(
            "UPDATE resources SET phase = $1, transitions = $2, updated_at = $3 WHERE id = $4",
        )
        .bind(phase_name(phase))
        .bind(to_value(&resource.transitions)?)
        .bind(at)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::NotFound(id.to_string()));
        }
        tx.commit().await?;
        self.get(id).await
    }

    /// Records a terminal provisioning failure with its redacted reason and
    /// retry policy, bumping the generation so a fresh attempt is idempotent.
    pub async fn record_failure(
        &self,
        id: &ResourceId,
        reason: &str,
        secrets: &[&str],
        policy: labrys_core::RetryPolicy,
        at: DateTime<Utc>,
    ) -> Result<Resource> {
        let mut resource = self.get(id).await?;
        resource.fail(reason, secrets, policy, at)?;
        sqlx::query(
            "UPDATE resources SET phase = $1, generation = $2, failure = $3, transitions = $4, updated_at = $5 WHERE id = $6",
        )
        .bind(phase_name(resource.phase))
        .bind(resource.generation as i32)
        .bind(to_value_opt(&resource.failure)?)
        .bind(to_value(&resource.transitions)?)
        .bind(at)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        self.get(id).await
    }
}

fn resource_from_row(row: &sqlx::postgres::PgRow) -> Result<Resource> {
    let id: String = row.try_get("id")?;
    let id: ResourceId = id
        .parse()
        .map_err(|_| ControlPlaneError::Mapping(format!("invalid resource id {id}")))?;
    let desired_version: i64 = row.try_get("desired_version")?;
    let generation: i32 = row.try_get("generation")?;
    let phase_name: String = row.try_get("phase")?;
    Ok(Resource {
        id,
        kind: row.try_get("kind")?,
        desired_version: desired_version as u64,
        phase: phase_from_name(&phase_name)?,
        generation: generation as u32,
        failure: json_get_opt(row, "failure")?,
        transitions: json_get(row, "transitions")?,
    })
}

fn phase_name(phase: ResourcePhase) -> &'static str {
    match phase {
        ResourcePhase::Requested => "requested",
        ResourcePhase::Provisioning => "provisioning",
        ResourcePhase::Ready => "ready",
        ResourcePhase::Degraded => "degraded",
        ResourcePhase::Failed => "failed",
        ResourcePhase::Deleting => "deleting",
        ResourcePhase::Deleted => "deleted",
    }
}

fn phase_from_name(name: &str) -> Result<ResourcePhase> {
    Ok(match name {
        "requested" => ResourcePhase::Requested,
        "provisioning" => ResourcePhase::Provisioning,
        "ready" => ResourcePhase::Ready,
        "degraded" => ResourcePhase::Degraded,
        "failed" => ResourcePhase::Failed,
        "deleting" => ResourcePhase::Deleting,
        "deleted" => ResourcePhase::Deleted,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown resource phase {other}"
            )))
        }
    })
}
