use chrono::Utc;
use sqlx::{PgPool, Row};

use labrys_core::{ApplicationId, Deployment, DeploymentId, DeploymentPhase, EnvironmentId};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{from_value, to_value};

/// Durable store for first-class deployment lifecycle records.
///
/// The deployment is stored as an extensible JSONB snapshot (its contract is
/// versioned in `labrys-core`) plus indexed key columns for ownership, phase,
/// and environment isolation.
#[derive(Clone)]
pub struct PgDeploymentStore {
    pool: PgPool,
}

impl PgDeploymentStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, deployment: &Deployment) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO deployments
                 (id, application_id, environment_id, phase, record, created_at, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7)
               ON CONFLICT (id) DO UPDATE SET
                 phase = EXCLUDED.phase,
                 record = EXCLUDED.record,
                 updated_at = EXCLUDED.updated_at"#,
        )
        .bind(deployment.id.to_string())
        .bind(deployment.application_id.to_string())
        .bind(deployment.environment_id.to_string())
        .bind(phase_name(deployment.phase))
        .bind(to_value(deployment)?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &DeploymentId) -> Result<Deployment> {
        let record: serde_json::Value = sqlx::query("SELECT record FROM deployments WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .map(|row| row.try_get("record").expect("record is jsonb"))
            .ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))?;
        from_value(record)
    }

    pub async fn list_for_environment(
        &self,
        application_id: &ApplicationId,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<Deployment>> {
        let rows = sqlx::query(
            "SELECT record FROM deployments WHERE application_id = $1 AND environment_id = $2 ORDER BY created_at, id",
        )
        .bind(application_id.to_string())
        .bind(environment_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| from_value(row.try_get("record").expect("record is jsonb")))
            .collect()
    }

    pub async fn set_phase(&self, id: &DeploymentId, phase: DeploymentPhase) -> Result<Deployment> {
        let mut deployment = self.get(id).await?;
        deployment.phase = phase;
        self.save(&deployment).await?;
        self.get(id).await
    }
}

fn phase_name(phase: DeploymentPhase) -> &'static str {
    match phase {
        DeploymentPhase::Pending => "pending",
        DeploymentPhase::Building => "building",
        DeploymentPhase::Deploying => "deploying",
        DeploymentPhase::Healthy => "healthy",
        DeploymentPhase::Degraded => "degraded",
        DeploymentPhase::Failed => "failed",
        DeploymentPhase::Superseded => "superseded",
        DeploymentPhase::RolledBack => "rolled_back",
    }
}
