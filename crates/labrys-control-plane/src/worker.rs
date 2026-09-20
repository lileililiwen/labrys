use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use labrys_core::{
    Correlation, EventAction, EventActor, EventDraft, EventResource, EventResult, Job,
    ObservationSource, ResourceId, ResourcePhase,
};

use crate::config::Config;
use crate::error::Result;
use crate::jobs::{ClaimedJob, PgJobQueue};
use crate::observability::PgEventStore;
use crate::repos::PgResourceStore;

/// What the injected provider/runtime action observed for its target.
///
/// Only platform observations carry a resource id and phase; the worker applies
/// them through a platform probe. An agent's completion claim is never a
/// variant here, so it can never promote readiness through this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Platform observed the target resource ready.
    Ready(ResourceId),
    /// Platform observed the target resource degraded.
    Degraded(ResourceId),
    /// Accepted but not yet ready; the resource stays provisioning.
    InProgress,
    /// The action failed with a (redactable) reason; the job retries.
    Failed(String),
}

/// The injected provider/runtime execution boundary.
///
/// Implementations stand in for real container/registry/provider execution.
/// This change never wires a Docker daemon here; keeping execution behind this
/// trait is what stops the worker from turning a simulation into real runtime
/// work.
#[async_trait]
pub trait JobDispatcher: Send + Sync {
    async fn dispatch(&self, job: &Job) -> Result<DispatchOutcome>;
}

/// Per-tick counters, useful for structured worker attribution and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerStats {
    pub recovered: u64,
    pub claimed: u64,
    pub completed: u64,
    pub in_progress: u64,
    pub retried: u64,
    pub dead_lettered: u64,
    pub idle: u64,
}

/// Result of a single worker pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    Idle,
    Recovered(Vec<ResourceId>),
    Completed { job: JobId, ready: bool },
    Retried { job: JobId, attempts: u32 },
    DeadLettered { job: JobId },
}

use labrys_core::JobId;

/// A recoverable reconciliation worker over the durable job queue.
pub struct Worker {
    queue: PgJobQueue,
    resources: PgResourceStore,
    events: PgEventStore,
    dispatcher: Arc<dyn JobDispatcher>,
    worker_id: String,
    lease: Duration,
    poll_interval_ms: u64,
    drain_timeout_ms: u64,
    secrets: Vec<String>,
    stopping: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
}

impl Worker {
    pub fn new(pool: PgPool, dispatcher: Arc<dyn JobDispatcher>, config: &Config) -> Self {
        Self::with_secrets(pool, dispatcher, config, Vec::new())
    }

    /// Builds a worker that redacts every diagnostic it persists against the
    /// given known secret values, so a provider failure carrying a credential
    /// can never reach an event or the job record in plaintext.
    pub fn with_secrets(
        pool: PgPool,
        dispatcher: Arc<dyn JobDispatcher>,
        config: &Config,
        secrets: Vec<String>,
    ) -> Self {
        Self {
            queue: PgJobQueue::new(pool.clone()),
            resources: PgResourceStore::new(pool.clone()),
            events: PgEventStore::new(pool),
            dispatcher,
            worker_id: config.worker_id.clone(),
            lease: Duration::seconds(config.lease_seconds),
            poll_interval_ms: config.poll_interval_ms,
            drain_timeout_ms: config.drain_timeout_ms,
            secrets,
            stopping: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn queue(&self) -> &PgJobQueue {
        &self.queue
    }

    /// Signals graceful shutdown: no new claims, bounded in-flight drain.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Runs a bounded number of reconciliation passes, then stops. Used by the
    /// smoke path and tests; production drives [`Worker::run`] instead.
    pub async fn run_bounded(&self, max_passes: usize) -> Result<WorkerStats> {
        let mut stats = WorkerStats::default();
        for _ in 0..max_passes {
            if !self.tick_once(&mut stats).await? {
                break;
            }
        }
        Ok(stats)
    }

    /// Runs until [`Worker::stop`] is called, then drains in-flight work.
    pub async fn run(&self) -> Result<()> {
        let mut stats = WorkerStats::default();
        while !self.is_stopping() {
            let acted = self.tick_once(&mut stats).await?;
            if !acted {
                tokio::time::sleep(std::time::Duration::from_millis(self.poll_interval_ms)).await;
            }
        }
        self.drain().await;
        Ok(())
    }

    /// One pass: recover expired leases, claim a due job, dispatch, and finalize
    /// the durable transition with worker attribution. Returns `false` when the
    /// queue was idle.
    async fn tick_once(&self, stats: &mut WorkerStats) -> Result<bool> {
        let now = Utc::now();
        let recovered = self.queue.recover_expired_leases(now).await?;
        if !recovered.is_empty() {
            stats.recovered += recovered.len() as u64;
            self.record_event(
                now,
                EventAction::Reconciled,
                EventResult::Accepted,
                format!("recovered {} expired lease(s)", recovered.len()),
            )
            .await?;
        }
        if self.is_stopping() {
            return Ok(false);
        }
        let claimed = match self
            .queue
            .claim_next(&self.worker_id, now, self.lease)
            .await?
        {
            Some(claimed) => claimed,
            None => {
                stats.idle += 1;
                return Ok(false);
            }
        };
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let outcome = self.finalize(&claimed).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        stats.claimed += 1;
        match outcome? {
            TickOutcome::Completed { ready, .. } => {
                if ready {
                    stats.completed += 1;
                } else {
                    stats.in_progress += 1;
                }
            }
            TickOutcome::Retried { .. } => stats.retried += 1,
            TickOutcome::DeadLettered { .. } => stats.dead_lettered += 1,
            _ => {}
        }
        Ok(true)
    }

    async fn finalize(&self, claimed: &ClaimedJob) -> Result<TickOutcome> {
        let now = Utc::now();
        let job = claimed.job.clone();
        let outcome = self.dispatcher.dispatch(&job).await?;
        match outcome {
            DispatchOutcome::Ready(resource_id) => {
                self.observe(resource_id, ResourcePhase::Ready, &job, now)
                    .await?;
                self.queue.complete(&job.id, &self.worker_id, now).await?;
                self.record_event(
                    now,
                    EventAction::ResourceProvisioned,
                    EventResult::Succeeded,
                    format!("job {} ready via worker {}", job.id, self.worker_id),
                )
                .await?;
                Ok(TickOutcome::Completed {
                    job: job.id,
                    ready: true,
                })
            }
            DispatchOutcome::Degraded(resource_id) => {
                self.observe(resource_id, ResourcePhase::Degraded, &job, now)
                    .await?;
                self.queue.complete(&job.id, &self.worker_id, now).await?;
                self.record_event(
                    now,
                    EventAction::ResourceProvisioned,
                    EventResult::Succeeded,
                    format!("job {} degraded via worker {}", job.id, self.worker_id),
                )
                .await?;
                Ok(TickOutcome::Completed {
                    job: job.id,
                    ready: false,
                })
            }
            DispatchOutcome::InProgress => {
                self.queue.complete(&job.id, &self.worker_id, now).await?;
                self.record_event(
                    now,
                    EventAction::Reconciled,
                    EventResult::Accepted,
                    format!("job {} in progress via worker {}", job.id, self.worker_id),
                )
                .await?;
                Ok(TickOutcome::Completed {
                    job: job.id,
                    ready: false,
                })
            }
            DispatchOutcome::Failed(reason) => {
                let secret_refs: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
                let reason = crate::redact::redact(&reason, &secret_refs);
                let status = self
                    .queue
                    .fail(&job.id, &self.worker_id, &reason, now)
                    .await?;
                let result = EventResult::Failed { reason };
                self.record_event(
                    now,
                    EventAction::Reconciled,
                    result,
                    format!(
                        "job {} attempt {} failed via worker {}",
                        job.id, job.attempts, self.worker_id
                    ),
                )
                .await?;
                Ok(match status {
                    labrys_core::JobStatus::Dead => TickOutcome::DeadLettered { job: job.id },
                    _ => TickOutcome::Retried {
                        job: job.id,
                        attempts: job.attempts,
                    },
                })
            }
        }
    }

    async fn observe(
        &self,
        resource_id: ResourceId,
        phase: ResourcePhase,
        job: &Job,
        now: DateTime<Utc>,
    ) -> Result<()> {
        // Readiness is recorded only through a platform probe; the agent never
        // reaches this path.
        self.resources
            .observe_phase(
                &resource_id,
                phase,
                ObservationSource::PlatformProbe,
                Some(format!("observed by worker {}", self.worker_id)),
                now,
            )
            .await?;
        let _ = job;
        Ok(())
    }

    async fn record_event(
        &self,
        now: DateTime<Utc>,
        action: EventAction,
        result: EventResult,
        detail: String,
    ) -> Result<()> {
        let mut draft = EventDraft::new(
            now,
            EventActor::platform(self.worker_id.clone()),
            Correlation::new(format!("worker:{}", self.worker_id)),
            action,
            result,
        );
        draft.resource = Some(EventResource::new("worker", self.worker_id.clone()));
        draft.after = Some(detail);
        let secret_refs: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        self.events.append(&draft, &secret_refs).await?;
        Ok(())
    }

    /// Bounded in-flight drain during shutdown. Unfinished leases are left for
    /// expiry-based recovery by another worker.
    async fn drain(&self) {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(self.drain_timeout_ms);
        while self.in_flight.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

/// The safe default dispatcher: it performs no real provider or runtime work
/// and reports `InProgress`, so this change can never turn a simulation into
/// container or registry execution. Real execution arrives in later changes
/// behind [`JobDispatcher`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopDispatcher;

#[async_trait]
impl JobDispatcher for NoopDispatcher {
    async fn dispatch(&self, _job: &Job) -> Result<DispatchOutcome> {
        Ok(DispatchOutcome::InProgress)
    }
}

/// A dispatcher that replays a scripted sequence of outcomes, for integration
/// tests of the claim/dispatch/finalize path. When the script is exhausted it
/// reports `InProgress`.
#[derive(Debug, Default)]
pub struct ScriptedDispatcher {
    outcomes: std::sync::Mutex<Vec<DispatchOutcome>>,
    calls: AtomicUsize,
}

impl ScriptedDispatcher {
    pub fn new(outcomes: Vec<DispatchOutcome>) -> Self {
        Self {
            outcomes: std::sync::Mutex::new(outcomes),
            calls: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JobDispatcher for ScriptedDispatcher {
    async fn dispatch(&self, _job: &Job) -> Result<DispatchOutcome> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let outcomes = self
            .outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = outcomes
            .get(index)
            .cloned()
            .unwrap_or(DispatchOutcome::InProgress);
        Ok(next)
    }
}
