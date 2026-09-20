use chrono::Utc;
use sqlx::{PgPool, Row};

use labrys_core::{ApplicationId, BindingId, BoundCapability, CapabilityMode};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{from_value, to_value};

/// Durable store for bound capabilities (the binding/mode lifecycle record).
///
/// The binding carries only secret *references*, never values, so the whole
/// record is safe to persist as an extensible JSONB snapshot.
#[derive(Clone)]
pub struct PgCapabilityStore {
    pool: PgPool,
}

impl PgCapabilityStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, bound: &BoundCapability) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO capabilities
                 (id, application_id, environment_name, capability, provider, mode, binding, record, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
               ON CONFLICT (id) DO UPDATE SET
                 mode = EXCLUDED.mode,
                 record = EXCLUDED.record,
                 updated_at = EXCLUDED.updated_at"#,
        )
        .bind(bound.id.to_string())
        .bind(bound.application_id.to_string())
        .bind(&bound.environment_name)
        .bind(&bound.capability)
        .bind(&bound.provider)
        .bind(mode_name(bound.mode))
        .bind(&bound.binding)
        .bind(to_value(bound)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &BindingId) -> Result<BoundCapability> {
        let record: serde_json::Value =
            sqlx::query("SELECT record FROM capabilities WHERE id = $1")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .map(|row| row.try_get("record").expect("record is jsonb"))
                .ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))?;
        from_value(record)
    }

    pub async fn list_for_application(
        &self,
        application_id: &ApplicationId,
    ) -> Result<Vec<BoundCapability>> {
        let rows = sqlx::query(
            "SELECT record FROM capabilities WHERE application_id = $1 ORDER BY capability",
        )
        .bind(application_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| from_value(row.try_get("record").expect("record is jsonb")))
            .collect()
    }
}

fn mode_name(mode: CapabilityMode) -> &'static str {
    match mode {
        CapabilityMode::Managed => "managed",
        CapabilityMode::External => "external",
        CapabilityMode::Adopted => "adopted",
    }
}
