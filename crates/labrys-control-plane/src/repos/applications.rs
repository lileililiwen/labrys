use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use labrys_core::environment::EnvironmentRefs;
use labrys_core::{
    Application, ApplicationId, ApplicationStatus, AuditMetadata, DesiredState, Environment,
    EnvironmentId, EnvironmentKind, EventDraft, LifecycleCollections, ObservedState,
    ObservedStatus,
};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{json_get, text_opt, to_value};
use crate::observability::{append_audit, append_event};

/// Durable store for the canonical `Application` aggregate and its
/// environment-scoped desired/observed state.
///
/// Desired state and observed state are written through separate methods so a
/// failed observation can never rewrite desired state. Every desired-state
/// mutation is guarded by an optimistic generation check and commits together
/// with its attributable event and audit record in a single transaction.
#[derive(Clone)]
pub struct PgApplicationStore {
    pool: PgPool,
}

impl PgApplicationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Upserts the full aggregate: application row, environments, and the
    /// desired/observed state snapshots, in one transaction.
    pub async fn save(&self, app: &Application) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        upsert_application(&mut tx, app).await?;
        for env in &app.environments {
            upsert_environment(&mut tx, env).await?;
        }
        upsert_desired_state(&mut tx, &app.id, &app.desired_state).await?;
        upsert_observed_state(&mut tx, &app.id, &app.observed_state).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reads the aggregate back, reconstructing environments and both states.
    pub async fn get(&self, id: &ApplicationId) -> Result<Application> {
        let row = sqlx::query("SELECT * FROM applications WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))?;
        let mut app = application_from_row(&row)?;
        app.environments = load_environments(&self.pool, id).await?;
        app.desired_state = load_desired_state(&self.pool, id).await?;
        app.observed_state = load_observed_state(&self.pool, id).await?;
        Ok(app)
    }

    pub async fn list(&self) -> Result<Vec<Application>> {
        let rows = sqlx::query("SELECT * FROM applications ORDER BY created_at, id")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut app = application_from_row(&row)?;
            let id = app.id;
            app.environments = load_environments(&self.pool, &id).await?;
            app.desired_state = load_desired_state(&self.pool, &id).await?;
            app.observed_state = load_observed_state(&self.pool, &id).await?;
            out.push(app);
        }
        Ok(out)
    }

    pub async fn delete(&self, id: &ApplicationId) -> Result<()> {
        let affected = sqlx::query("DELETE FROM applications WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Applies a desired-state update under an optimistic generation check and
    /// commits it together with its attributable event and audit record.
    ///
    /// A writer whose `expected_version` no longer matches the stored version
    /// receives [`ControlPlaneError::Conflict`] and neither the state nor the
    /// event changes.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_desired_state(
        &self,
        id: &ApplicationId,
        expected_version: u64,
        next: &DesiredState,
        event: &EventDraft,
        audit_detail: &str,
        updated_by: &str,
        secrets: &[&str],
    ) -> Result<Application> {
        if next.version != expected_version + 1 {
            return Err(ControlPlaneError::Conflict(format!(
                "next desired version {} is not expected_version+1 {}",
                next.version,
                expected_version + 1
            )));
        }
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query(
            "UPDATE desired_states SET version = $1, profiles = $2, config = $3 \
             WHERE application_id = $4 AND version = $5",
        )
        .bind(next.version as i64)
        .bind(to_value(&next.profiles)?)
        .bind(to_value(&next.config)?)
        .bind(id.to_string())
        .bind(expected_version as i64)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 0 {
            // Roll back implicitly by dropping the transaction.
            drop(tx);
            let current = self.current_desired_version(id).await?;
            return Err(ControlPlaneError::Conflict(format!(
                "desired-state generation conflict for {id}: expected {expected_version}, stored {current}"
            )));
        }
        let now = Utc::now();
        sqlx::query("UPDATE applications SET updated_at = $1, updated_by = $2 WHERE id = $3")
            .bind(now)
            .bind(updated_by)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        append_event(&mut *tx, event, secrets).await?;
        append_audit(
            &mut tx,
            now,
            updated_by,
            "desired_state_update",
            &id.to_string(),
            audit_detail,
            secrets,
        )
        .await?;
        tx.commit().await?;
        self.get(id).await
    }

    /// Records an observation through the observed-state table only; desired
    /// state is never touched by this path.
    pub async fn record_observation(
        &self,
        id: &ApplicationId,
        observed: &ObservedState,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let affected = upsert_observed_state_conn(&mut tx, id, observed).await?;
        if affected == 0 {
            return Err(ControlPlaneError::NotFound(id.to_string()));
        }
        sqlx::query("UPDATE applications SET updated_at = $1 WHERE id = $2")
            .bind(observed.last_observed_at)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Updates one environment's references, enforcing that the environment
    /// belongs to the application. A preview or development write can therefore
    /// never target (or mutate) the production environment.
    pub async fn set_environment_refs(
        &self,
        application_id: &ApplicationId,
        environment_id: &EnvironmentId,
        refs: &EnvironmentRefs,
    ) -> Result<()> {
        let affected =
            sqlx::query("UPDATE environments SET refs = $1 WHERE id = $2 AND application_id = $3")
                .bind(to_value(refs)?)
                .bind(environment_id.to_string())
                .bind(application_id.to_string())
                .execute(&self.pool)
                .await?
                .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::EnvironmentIsolation(format!(
                "environment {environment_id} does not belong to application {application_id}"
            )));
        }
        Ok(())
    }

    async fn current_desired_version(&self, id: &ApplicationId) -> Result<i64> {
        let version: Option<i64> =
            sqlx::query("SELECT version FROM desired_states WHERE application_id = $1")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .map(|row| row.try_get("version").expect("version is bigint"));
        version.ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))
    }
}

// --- transactional write helpers ------------------------------------------

async fn upsert_application(conn: &mut sqlx::PgConnection, app: &Application) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO applications
             (id, name, origin, status, created_at, updated_at, created_by, updated_by,
              audit_event_ids, collections)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
           ON CONFLICT (id) DO UPDATE SET
             name = EXCLUDED.name,
             origin = EXCLUDED.origin,
             status = EXCLUDED.status,
             updated_at = EXCLUDED.updated_at,
             updated_by = EXCLUDED.updated_by,
             audit_event_ids = EXCLUDED.audit_event_ids,
             collections = EXCLUDED.collections"#,
    )
    .bind(app.id.to_string())
    .bind(&app.name)
    .bind(to_value(&app.origin)?)
    .bind(status_name(app.status))
    .bind(app.created_at)
    .bind(app.updated_at)
    .bind(&app.audit.created_by)
    .bind(app.audit.updated_by.clone())
    .bind(to_value(&app.audit.audit_event_ids)?)
    .bind(to_value(&app.collections)?)
    .execute(conn)
    .await?;
    Ok(())
}

async fn upsert_environment(conn: &mut sqlx::PgConnection, env: &Environment) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO environments (id, application_id, kind, name, refs)
           VALUES ($1,$2,$3,$4,$5)
           ON CONFLICT (id) DO UPDATE SET
             kind = EXCLUDED.kind,
             name = EXCLUDED.name,
             refs = EXCLUDED.refs"#,
    )
    .bind(env.id.to_string())
    .bind(env.application_id.to_string())
    .bind(to_value(&env.kind)?)
    .bind(&env.name)
    .bind(to_value(&env.refs)?)
    .execute(conn)
    .await?;
    Ok(())
}

async fn upsert_desired_state(
    conn: &mut sqlx::PgConnection,
    id: &ApplicationId,
    state: &DesiredState,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO desired_states (application_id, version, profiles, config)
           VALUES ($1,$2,$3,$4)
           ON CONFLICT (application_id) DO UPDATE SET
             version = EXCLUDED.version,
             profiles = EXCLUDED.profiles,
             config = EXCLUDED.config"#,
    )
    .bind(id.to_string())
    .bind(state.version as i64)
    .bind(to_value(&state.profiles)?)
    .bind(to_value(&state.config)?)
    .execute(conn)
    .await?;
    Ok(())
}

async fn upsert_observed_state(
    conn: &mut sqlx::PgConnection,
    id: &ApplicationId,
    state: &ObservedState,
) -> Result<()> {
    upsert_observed_state_conn(conn, id, state).await?;
    Ok(())
}

async fn upsert_observed_state_conn(
    conn: &mut sqlx::PgConnection,
    id: &ApplicationId,
    state: &ObservedState,
) -> Result<u64> {
    let affected = sqlx::query(
        r#"INSERT INTO observed_states
             (application_id, desired_version, status, last_observed_at, detail)
           VALUES ($1,$2,$3,$4,$5)
           ON CONFLICT (application_id) DO UPDATE SET
             desired_version = EXCLUDED.desired_version,
             status = EXCLUDED.status,
             last_observed_at = EXCLUDED.last_observed_at,
             detail = EXCLUDED.detail"#,
    )
    .bind(id.to_string())
    .bind(state.desired_version as i64)
    .bind(observed_status_name(state.status))
    .bind(state.last_observed_at)
    .bind(state.detail.clone())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected)
}

// --- read helpers ----------------------------------------------------------

async fn load_environments(pool: &PgPool, id: &ApplicationId) -> Result<Vec<Environment>> {
    let rows = sqlx::query("SELECT * FROM environments WHERE application_id = $1 ORDER BY name")
        .bind(id.to_string())
        .fetch_all(pool)
        .await?;
    rows.iter().map(environment_from_row).collect()
}

async fn load_desired_state(pool: &PgPool, id: &ApplicationId) -> Result<DesiredState> {
    let row = sqlx::query("SELECT * FROM desired_states WHERE application_id = $1")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ControlPlaneError::NotFound(format!("desired state for {id}")))?;
    let version: i64 = row.try_get("version")?;
    Ok(DesiredState {
        version: version as u64,
        profiles: json_get(&row, "profiles")?,
        config: json_get(&row, "config")?,
    })
}

async fn load_observed_state(pool: &PgPool, id: &ApplicationId) -> Result<ObservedState> {
    let row = sqlx::query("SELECT * FROM observed_states WHERE application_id = $1")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ControlPlaneError::NotFound(format!("observed state for {id}")))?;
    observed_state_from_row(&row)
}

fn application_from_row(row: &sqlx::postgres::PgRow) -> Result<Application> {
    let id: String = row.try_get("id")?;
    let id: ApplicationId = id
        .parse()
        .map_err(|_| ControlPlaneError::Mapping(format!("invalid application id {id}")))?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let status_name: String = row.try_get("status")?;
    Ok(Application {
        id,
        name: row.try_get("name")?,
        origin: json_get(row, "origin")?,
        status: status_from_name(&status_name)?,
        created_at,
        updated_at,
        audit: AuditMetadata {
            created_by: row.try_get("created_by")?,
            updated_by: text_opt(row, "updated_by")?,
            audit_event_ids: json_get(row, "audit_event_ids")?,
        },
        environments: Vec::new(),
        desired_state: DesiredState::initial(),
        observed_state: ObservedState::unknown(1),
        collections: json_get::<LifecycleCollections>(row, "collections")?,
    })
}

fn environment_from_row(row: &sqlx::postgres::PgRow) -> Result<Environment> {
    let id: String = row.try_get("id")?;
    let id: EnvironmentId = id
        .parse()
        .map_err(|_| ControlPlaneError::Mapping(format!("invalid environment id {id}")))?;
    let application_id: String = row.try_get("application_id")?;
    let application_id: ApplicationId = application_id.parse().map_err(|_| {
        ControlPlaneError::Mapping(format!("invalid application id {application_id}"))
    })?;
    let kind: EnvironmentKind = json_get(row, "kind")?;
    Ok(Environment {
        id,
        application_id,
        kind,
        name: row.try_get("name")?,
        refs: json_get(row, "refs")?,
    })
}

fn observed_state_from_row(row: &sqlx::postgres::PgRow) -> Result<ObservedState> {
    let desired_version: i64 = row.try_get("desired_version")?;
    let status_name: String = row.try_get("status")?;
    let last_observed_at: DateTime<Utc> = row.try_get("last_observed_at")?;
    Ok(ObservedState {
        desired_version: desired_version as u64,
        status: observed_status_from_name(&status_name)?,
        last_observed_at,
        detail: text_opt(row, "detail")?,
    })
}

// --- name <-> enum helpers -------------------------------------------------

fn status_name(status: ApplicationStatus) -> &'static str {
    match status {
        ApplicationStatus::Draft => "draft",
        ApplicationStatus::Active => "active",
        ApplicationStatus::Archived => "archived",
    }
}

fn status_from_name(name: &str) -> Result<ApplicationStatus> {
    Ok(match name {
        "draft" => ApplicationStatus::Draft,
        "active" => ApplicationStatus::Active,
        "archived" => ApplicationStatus::Archived,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown application status {other}"
            )))
        }
    })
}

fn observed_status_name(status: ObservedStatus) -> &'static str {
    match status {
        ObservedStatus::Unknown => "unknown",
        ObservedStatus::Healthy => "healthy",
        ObservedStatus::Degraded => "degraded",
        ObservedStatus::Unhealthy => "unhealthy",
    }
}

fn observed_status_from_name(name: &str) -> Result<ObservedStatus> {
    Ok(match name {
        "unknown" => ObservedStatus::Unknown,
        "healthy" => ObservedStatus::Healthy,
        "degraded" => ObservedStatus::Degraded,
        "unhealthy" => ObservedStatus::Unhealthy,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown observed status {other}"
            )))
        }
    })
}
