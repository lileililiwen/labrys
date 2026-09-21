use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};

use labrys_core::{EnqueueOutcome, IdempotencyKey, Job, JobAction, JobId, JobStatus, RetryPolicy};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{json_get, text_opt, to_value};
use crate::redact;

/// A job handed to a worker after a successful claim, together with the lease
/// that keeps it invisible to other workers until it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub job: Job,
    pub lease_owner: String,
    pub lease_expires_at: DateTime<Utc>,
}

/// Durable, idempotent controller-job repository implementing the `JobQueue`
/// semantics over PostgreSQL.
///
/// Claim uses a lease plus `FOR UPDATE SKIP LOCKED` so exactly one worker wins a
/// ready job; the idempotency key is globally unique so a duplicate request for
/// work that is active or already terminal returns the existing outcome instead
/// of enqueuing a second side effect; and backoff is driven by persisted
/// timestamps, never process sleep state.
#[derive(Clone)]
pub struct PgJobQueue {
    pool: PgPool,
}

impl PgJobQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Enqueues a convergence action unless its idempotency key is already
    /// taken by an active or terminal job, in which case the existing job is
    /// returned as a duplicate and no second side effect is scheduled.
    pub async fn enqueue(
        &self,
        target: impl Into<String>,
        action: JobAction,
        version: u64,
        policy: RetryPolicy,
        at: DateTime<Utc>,
    ) -> Result<EnqueueOutcome> {
        let target = target.into();
        let key = IdempotencyKey::new(&target, action, version);
        let id = JobId::new();
        let inserted: Option<String> = sqlx::query(
            r#"INSERT INTO jobs
                 (id, target, action, idempotency_key, status, attempts, policy,
                  next_attempt_at, last_error, enqueued_at, updated_at)
               VALUES ($1,$2,$3,$4,'queued',0,$5,$6,NULL,$6,$6)
               ON CONFLICT (idempotency_key) DO NOTHING
               RETURNING id"#,
        )
        .bind(id.to_string())
        .bind(target)
        .bind(action.as_str())
        .bind(key.as_str())
        .bind(to_value(&policy)?)
        .bind(at)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| row.try_get("id").expect("id is text"));
        match inserted {
            Some(_) => Ok(EnqueueOutcome::Enqueued(id)),
            None => {
                let existing: String =
                    sqlx::query("SELECT id FROM jobs WHERE idempotency_key = $1")
                        .bind(key.as_str())
                        .fetch_one(&self.pool)
                        .await?
                        .try_get("id")
                        .expect("id is text");
                Ok(EnqueueOutcome::Duplicate(parse_job_id(&existing)?))
            }
        }
    }

    /// Claims the earliest-due queued job for `worker_id`, moving it to running
    /// with a bounded lease. Returns `None` when nothing is due.
    pub async fn claim_next(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease: Duration,
    ) -> Result<Option<ClaimedJob>> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"WITH candidate AS (
                 SELECT id FROM jobs
                 WHERE status = 'queued' AND next_attempt_at <= $1
                 ORDER BY next_attempt_at, enqueued_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1
               )
               UPDATE jobs
                  SET status = 'running',
                      attempts = attempts + 1,
                      lease_owner = $2,
                      lease_expires_at = $3,
                      updated_at = $1
                 FROM candidate
                 WHERE jobs.id = candidate.id
                 RETURNING jobs.*"#,
        )
        .bind(now)
        .bind(worker_id)
        .bind(now + lease)
        .fetch_optional(&mut *tx)
        .await?;
        match row {
            Some(row) => {
                let job = job_from_row(&row)?;
                let lease_expires_at: DateTime<Utc> = row.try_get("lease_expires_at")?;
                tx.commit().await?;
                Ok(Some(ClaimedJob {
                    job,
                    lease_owner: worker_id.to_string(),
                    lease_expires_at,
                }))
            }
            None => {
                tx.commit().await?;
                Ok(None)
            }
        }
    }

    /// Marks a running job converged. Only the current lease holder may do so.
    pub async fn complete(&self, id: &JobId, worker_id: &str, now: DateTime<Utc>) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE jobs SET status='succeeded', last_error=NULL, lease_owner=NULL, lease_expires_at=NULL, updated_at=$1 WHERE id=$2 AND status='running' AND lease_owner=$3",
        )
        .bind(now)
        .bind(id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::JobState(format!(
                "job {id} is not running under worker {worker_id}"
            )));
        }
        Ok(())
    }

    /// Records an attempt failure: re-queues with persisted exponential backoff,
    /// or dead-letters once the attempt budget is exhausted.
    pub async fn fail(
        &self,
        id: &JobId,
        worker_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<JobStatus> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT attempts, policy FROM jobs WHERE id = $1 AND status = 'running' AND lease_owner = $2 FOR UPDATE",
        )
        .bind(id.to_string())
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ControlPlaneError::JobState(format!(
                "job {id} is not running under worker {worker_id}"
            ))
        })?;
        let attempts: i32 = row.try_get("attempts")?;
        let policy: RetryPolicy = json_get(&row, "policy")?;
        let attempts = attempts.max(1) as u32;
        let redacted = redact::redact_guard("job.last_error", error, &[])?;
        let (status, next_attempt_at) = if attempts >= policy.max_attempts {
            (JobStatus::Dead, now)
        } else {
            (JobStatus::Queued, now + policy.delay_for(attempts))
        };
        sqlx::query(
            "UPDATE jobs SET status=$1, last_error=$2, next_attempt_at=$3, lease_owner=NULL, lease_expires_at=NULL, updated_at=$3 WHERE id=$4",
        )
        .bind(status_name(status))
        .bind(&redacted)
        .bind(next_attempt_at)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(status)
    }

    /// Holds a queued job out of claiming.
    pub async fn pause(&self, id: &JobId, now: DateTime<Utc>) -> Result<()> {
        self.transition(id, "queued", "paused", now, false).await
    }

    /// Returns a paused job to the queue.
    pub async fn resume(&self, id: &JobId, now: DateTime<Utc>) -> Result<()> {
        self.transition(id, "paused", "queued", now, true).await
    }

    /// Explicitly requeues a dead-lettered job with a fresh attempt budget.
    pub async fn requeue(&self, id: &JobId, now: DateTime<Utc>) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE jobs SET status='queued', attempts=0, next_attempt_at=$1, last_error=NULL, lease_owner=NULL, lease_expires_at=NULL, updated_at=$1 WHERE id=$2 AND status='dead'",
        )
        .bind(now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::JobState(format!(
                "only dead jobs can be requeued; job {id} is not dead"
            )));
        }
        Ok(())
    }

    /// Makes jobs whose lease expired claimable again with a bounded recovery
    /// detail, so an interrupted worker never strands a job in `running`.
    pub async fn recover_expired_leases(&self, now: DateTime<Utc>) -> Result<Vec<JobId>> {
        let rows = sqlx::query(
            "UPDATE jobs SET status='queued', lease_owner=NULL, lease_expires_at=NULL, last_error=$1, updated_at=$2 WHERE status='running' AND lease_expires_at <= $2 RETURNING id",
        )
        .bind("lease expired; job recovered for re-claim")
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| parse_job_id(row.try_get("id").expect("id is text")))
            .collect()
    }

    pub async fn get(&self, id: &JobId) -> Result<Job> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ControlPlaneError::NotFound(id.to_string()))?;
        job_from_row(&row)
    }

    /// Latest job for one convergence target, newest first. Lets read paths
    /// (health, job status) report the durable operation behind an accepted
    /// mutation without claiming work the platform has not observed.
    pub async fn latest_for_target(&self, target: &str) -> Result<Option<Job>> {
        let row = sqlx::query(
            "SELECT * FROM jobs WHERE target = $1 ORDER BY updated_at DESC, enqueued_at DESC LIMIT 1",
        )
        .bind(target)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| job_from_row(&row)).transpose()
    }

    /// Jobs that exhausted their retry budget.
    pub async fn dead_letters(&self) -> Result<Vec<JobId>> {
        let rows = sqlx::query("SELECT id FROM jobs WHERE status = 'dead' ORDER BY updated_at")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| parse_job_id(row.try_get("id").expect("id is text")))
            .collect()
    }

    /// True when no job is queued, paused, or running.
    pub async fn is_quiet(&self) -> Result<bool> {
        let count: i64 =
            sqlx::query("SELECT count(*) FROM jobs WHERE status IN ('queued','paused','running')")
                .fetch_one(&self.pool)
                .await?
                .try_get("count")
                .expect("count is bigint");
        Ok(count == 0)
    }

    async fn transition(
        &self,
        id: &JobId,
        from: &str,
        to: &str,
        now: DateTime<Utc>,
        reset_next: bool,
    ) -> Result<()> {
        let next_clause = if reset_next {
            ", next_attempt_at = $1"
        } else {
            ""
        };
        let sql = format!(
            "UPDATE jobs SET status = $2{next_clause}, updated_at = $1 WHERE id = $3 AND status = $4"
        );
        let affected = sqlx::query(&sql)
            .bind(now)
            .bind(to)
            .bind(id.to_string())
            .bind(from)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(ControlPlaneError::JobState(format!(
                "cannot move job {id} from {from} to {to}"
            )));
        }
        Ok(())
    }
}

fn job_from_row(row: &sqlx::postgres::PgRow) -> Result<Job> {
    let id: String = row.try_get("id")?;
    let action_name: String = row.try_get("action")?;
    let status_name: String = row.try_get("status")?;
    let key: String = row.try_get("idempotency_key")?;
    let attempts: i32 = row.try_get("attempts")?;
    let next_attempt_at: DateTime<Utc> = row.try_get("next_attempt_at")?;
    let enqueued_at: DateTime<Utc> = row.try_get("enqueued_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let policy: RetryPolicy = json_get(row, "policy")?;
    Ok(Job {
        id: parse_job_id(&id)?,
        target: row.try_get("target")?,
        action: action_from_name(&action_name)?,
        idempotency_key: decode_idempotency_key(key),
        status: status_from_name(&status_name)?,
        attempts: attempts as u32,
        policy,
        next_attempt_at,
        last_error: text_opt(row, "last_error")?,
        enqueued_at,
        updated_at,
    })
}

/// `IdempotencyKey` is a transparent `String` newtype with no `FromStr`, so it
/// is rebuilt from its stored string through its serde representation.
fn decode_idempotency_key(stored: String) -> IdempotencyKey {
    serde_json::from_value(serde_json::Value::String(stored))
        .expect("IdempotencyKey is a transparent string newtype")
}

fn parse_job_id(value: &str) -> Result<JobId> {
    value
        .parse()
        .map_err(|_| ControlPlaneError::Mapping(format!("invalid job id {value}")))
}

fn action_from_name(name: &str) -> Result<JobAction> {
    Ok(match name {
        "provision" => JobAction::Provision,
        "update" => JobAction::Update,
        "rollout" => JobAction::Rollout,
        "verify" => JobAction::Verify,
        "delete" => JobAction::Delete,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown job action {other}"
            )))
        }
    })
}

fn status_name(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Paused => "paused",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Dead => "dead",
    }
}

fn status_from_name(name: &str) -> Result<JobStatus> {
    Ok(match name {
        "queued" => JobStatus::Queued,
        "paused" => JobStatus::Paused,
        "running" => JobStatus::Running,
        "succeeded" => JobStatus::Succeeded,
        "dead" => JobStatus::Dead,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown job status {other}"
            )))
        }
    })
}
