//! Controllers, reconciliation, jobs, resource lifecycle, and health.
//!
//! Covers requirement `reconciliation`: desired state and provider/runtime
//! observed state are persisted separately and compared by controllers that
//! keep converging after the initiating request has ended. Resources expose
//! the explicit lifecycle `requested → provisioning → ready | degraded |
//! failed → deleting → deleted` with transition evidence. Work is expressed
//! as idempotent jobs on a queue that models the MVP PostgreSQL jobs table
//! (retry with exponential backoff, dead-lettering, pause/resume); a job with
//! the same idempotency key is never duplicated while it is in flight.
//! Health is aggregated from platform-owned checks only — an agent tool
//! result is never treated as proof of ready or healthy state.
//!
//! This module is pure and deterministic; the SQL jobs table and the Rust
//! worker are infrastructure built on these contracts.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::{ApplicationId, JobId, PreviewId, ResourceId};
use crate::state::{DesiredState, ObservedState, ObservedStatus};

/// Version of the controller/reconciliation contract vocabulary.
pub const RECONCILIATION_CONTRACT_VERSION: u32 = 1;

/// Strips known secret values out of a failure reason before it is stored or
/// rendered. Reasons reaching events and logs are always redacted.
pub fn redact_reason(reason: &str, secrets: &[&str]) -> String {
    let mut out = reason.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "[redacted]");
    }
    out
}

// ---------------------------------------------------------------------------
// Resource lifecycle
// ---------------------------------------------------------------------------

/// Explicit lifecycle state of a platform-owned resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourcePhase {
    /// Creation has been requested but not started.
    Requested,
    /// A provider is working on it; not usable yet.
    Provisioning,
    /// Observed ready by a platform check.
    Ready,
    /// Present but partially unhealthy.
    Degraded,
    /// Terminal error until a retry converges it.
    Failed,
    /// Deletion requested; the provider is tearing it down.
    Deleting,
    /// Gone; terminal.
    Deleted,
}

impl ResourcePhase {
    /// True when no further transition is legal.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Deleted)
    }

    /// The explicit lifecycle graph. Anything not listed here is invalid.
    pub fn can_transition_to(self, next: Self) -> bool {
        use ResourcePhase::*;
        match self {
            Requested => matches!(next, Provisioning | Failed | Deleting),
            Provisioning => matches!(next, Ready | Degraded | Failed | Deleting),
            // A drift or update re-provisions an existing resource.
            Ready => matches!(next, Degraded | Failed | Provisioning | Deleting),
            Degraded => matches!(next, Ready | Failed | Deleting),
            Failed => matches!(next, Requested | Provisioning | Deleting),
            Deleting => matches!(next, Deleted),
            Deleted => false,
        }
    }

    /// Health a platform check would report for a resource in this phase.
    /// A non-Ready phase never yields [`HealthState::Healthy`].
    pub fn health(self) -> HealthState {
        match self {
            Self::Requested | Self::Provisioning => HealthState::Provisioning,
            Self::Ready => HealthState::Healthy,
            Self::Degraded => HealthState::Degraded,
            Self::Failed => HealthState::Failed,
            Self::Deleting | Self::Deleted => HealthState::Unknown,
        }
    }
}

/// Where a lifecycle observation came from. Agent tool results are not a
/// valid source: only platform probes and provider callbacks move lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationSource {
    /// A platform-owned probe or controller check.
    PlatformProbe,
    /// An asynchronous provider status callback.
    ProviderCallback,
}

/// One audited lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTransition {
    pub from: ResourcePhase,
    pub to: ResourcePhase,
    pub at: DateTime<Utc>,
    pub source: ObservationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Retry policy attached to a failed resource and its provisioning jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Total attempts before the job dead-letters.
    pub max_attempts: u32,
    /// Delay after the first failure.
    pub base_delay: Duration,
    /// Multiplier applied per additional attempt.
    pub factor: u32,
    /// Upper bound on the computed delay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::seconds(5),
            factor: 2,
            max_delay: Some(Duration::seconds(300)),
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff for the given 1-based attempt number.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let factor = u64::from(self.factor.max(1));
        let exponent = u64::from(attempt.saturating_sub(1));
        let multiplier = factor.saturating_pow(exponent.min(31) as u32).max(1);
        let seconds = self
            .base_delay
            .max(Duration::zero())
            .num_seconds()
            .saturating_mul(i64::try_from(multiplier).unwrap_or(i64::MAX));
        let delay = Duration::seconds(seconds);
        match self.max_delay {
            Some(cap) if delay > cap => cap,
            _ => delay,
        }
    }
}

/// Failure evidence carried by a resource in [`ResourcePhase::Failed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecord {
    /// Already-redacted reason; never contains secret values.
    pub reason: String,
    pub policy: RetryPolicy,
    pub attempts: u32,
    pub next_retry_at: DateTime<Utc>,
}

/// A platform-owned resource moving through the explicit lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: ResourceId,
    /// Kind key, e.g. `database.postgres` or `preview`.
    pub kind: String,
    /// Desired-state version this resource is converging toward.
    pub desired_version: u64,
    pub phase: ResourcePhase,
    /// Bumped on every terminal failure so in-flight jobs stay idempotent
    /// while a fresh generation may be re-provisioned.
    #[serde(default)]
    pub generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureRecord>,
    /// Transition evidence, oldest first.
    #[serde(default)]
    pub transitions: Vec<ResourceTransition>,
}

impl Resource {
    /// Creates a resource in `requested` with its first evidence entry.
    pub fn request(
        kind: impl Into<String>,
        desired_version: u64,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        Ok(Self {
            id: ResourceId::new(),
            kind: kind.into(),
            desired_version,
            phase: ResourcePhase::Requested,
            generation: 0,
            failure: None,
            transitions: vec![ResourceTransition {
                from: ResourcePhase::Requested,
                to: ResourcePhase::Requested,
                at,
                source: ObservationSource::PlatformProbe,
                reason: None,
            }],
        })
    }

    /// Moves to `next`, rejecting lifecycle-graph violations.
    pub fn observe(
        &mut self,
        next: ResourcePhase,
        source: ObservationSource,
        reason: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<&ResourceTransition> {
        if !self.phase.can_transition_to(next) {
            return Err(CoreError::Reconciliation(format!(
                "illegal resource transition {:?} -> {:?} for resource {}",
                self.phase, next, self.id
            )));
        }
        if next == self.phase {
            return Err(CoreError::Reconciliation(format!(
                "resource {} is already in phase {:?}",
                self.id, next
            )));
        }
        self.phase = next;
        self.transitions.push(ResourceTransition {
            from: self
                .transitions
                .last()
                .expect("transitions are never empty")
                .to,
            to: next,
            at,
            source,
            reason,
        });
        Ok(self.transitions.last().expect("just pushed"))
    }

    /// Records a terminal provisioning failure with a redacted reason and the
    /// retry policy governing the next attempt.
    pub fn fail(
        &mut self,
        reason: &str,
        secrets: &[&str],
        policy: RetryPolicy,
        at: DateTime<Utc>,
    ) -> Result<()> {
        self.observe(
            ResourcePhase::Failed,
            ObservationSource::ProviderCallback,
            Some(redact_reason(reason, secrets)),
            at,
        )?;
        let attempts = self.failure.as_ref().map_or(1, |f| f.attempts + 1);
        self.generation += 1;
        self.failure = Some(FailureRecord {
            reason: redact_reason(reason, secrets),
            policy,
            attempts,
            next_retry_at: at + policy.delay_for(attempts),
        });
        Ok(())
    }

    /// Requests deletion; the provider then reports [`ResourcePhase::Deleted`].
    pub fn begin_delete(&mut self, at: DateTime<Utc>) -> Result<()> {
        self.observe(
            ResourcePhase::Deleting,
            ObservationSource::PlatformProbe,
            None,
            at,
        )?;
        Ok(())
    }

    /// Marks deletion complete after the provider confirms teardown.
    pub fn mark_deleted(&mut self, at: DateTime<Utc>) -> Result<()> {
        self.observe(
            ResourcePhase::Deleted,
            ObservationSource::ProviderCallback,
            None,
            at,
        )?;
        self.failure = None;
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.phase == ResourcePhase::Ready
    }

    pub fn is_gone(&self) -> bool {
        matches!(self.phase, ResourcePhase::Deleting | ResourcePhase::Deleted)
    }

    /// Version-scoped generation used to build idempotency keys: the same
    /// desired version plus the same failure generation always maps to one
    /// in-flight job, while a new failure allows a fresh attempt.
    pub fn convergence_version(&self) -> u64 {
        self.desired_version
            .saturating_mul(1000)
            .saturating_add(u64::from(self.generation))
    }

    /// Renders the resource for events/logs. Reasons are stored redacted.
    pub fn to_event_detail(&self) -> String {
        let failure = self
            .failure
            .as_ref()
            .map(|f| {
                format!(
                    " failure: {} (attempt {}/{})",
                    f.reason, f.attempts, f.policy.max_attempts
                )
            })
            .unwrap_or_default();
        format!(
            "resource {} ({}) phase {:?} desired v{}{}",
            self.id, self.kind, self.phase, self.desired_version, failure
        )
    }
}

// ---------------------------------------------------------------------------
// Idempotent jobs
// ---------------------------------------------------------------------------

/// Action a worker performs. Idempotency is scoped per (target, action,
/// version), so the same convergence step can never run twice concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum JobAction {
    Provision,
    Update,
    Rollout,
    Verify,
    Delete,
}

impl JobAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provision => "provision",
            Self::Update => "update",
            Self::Rollout => "rollout",
            Self::Verify => "verify",
            Self::Delete => "delete",
        }
    }
}

impl fmt::Display for JobAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Deterministic deduplication key for a convergence action.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(target: &str, action: JobAction, version: u64) -> Self {
        Self(format!("{target}:{}:v{version}", action.as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Queue status of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum JobStatus {
    Queued,
    /// Held out of claiming until explicitly resumed.
    Paused,
    Running,
    Succeeded,
    /// Exhausted retries; requires an explicit requeue.
    Dead,
}

impl JobStatus {
    /// True while the job may still run, so its idempotency key stays taken.
    pub fn is_in_flight(self) -> bool {
        matches!(self, Self::Queued | Self::Paused | Self::Running)
    }
}

/// One unit of convergence work on the MVP PostgreSQL jobs queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: JobId,
    pub target: String,
    pub action: JobAction,
    pub idempotency_key: IdempotencyKey,
    pub status: JobStatus,
    pub attempts: u32,
    pub policy: RetryPolicy,
    /// Earliest time a worker may claim the job.
    pub next_attempt_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub enqueued_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Outcome of an enqueue attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// A fresh job was created.
    Enqueued(JobId),
    /// An in-flight job with the same idempotency key already exists.
    Duplicate(JobId),
}

/// In-memory stand-in for the MVP PostgreSQL jobs table plus the worker
/// claim/retry boundary.
#[derive(Debug, Default)]
pub struct JobQueue {
    jobs: BTreeMap<String, Job>,
}

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues `action` for `target` unless an in-flight job with the same
    /// idempotency key exists, in which case the duplicate is suppressed.
    pub fn enqueue(
        &mut self,
        target: impl Into<String>,
        action: JobAction,
        version: u64,
        policy: RetryPolicy,
        at: DateTime<Utc>,
    ) -> EnqueueOutcome {
        let target = target.into();
        let key = IdempotencyKey::new(&target, action, version);
        if let Some(existing) = self
            .jobs
            .values()
            .find(|job| job.idempotency_key == key && job.status.is_in_flight())
        {
            return EnqueueOutcome::Duplicate(existing.id);
        }
        let job = Job {
            id: JobId::new(),
            target,
            action,
            idempotency_key: key,
            status: JobStatus::Queued,
            attempts: 0,
            policy,
            next_attempt_at: at,
            last_error: None,
            enqueued_at: at,
            updated_at: at,
        };
        let id = job.id;
        self.jobs.insert(id.to_string(), job);
        EnqueueOutcome::Enqueued(id)
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.get(&id.to_string())
    }

    pub fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.values()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Claims the earliest-due queued job, moving it to `running`.
    pub fn claim_next(&mut self, now: DateTime<Utc>) -> Option<JobId> {
        let candidate = self
            .jobs
            .values()
            .filter(|job| job.status == JobStatus::Queued && job.next_attempt_at <= now)
            .min_by_key(|job| (job.next_attempt_at, job.enqueued_at))
            .map(|job| job.id);
        match candidate {
            Some(id) => {
                let job = self
                    .jobs
                    .get_mut(&id.to_string())
                    .expect("candidate came from the map");
                job.status = JobStatus::Running;
                job.attempts += 1;
                job.updated_at = now;
                Some(id)
            }
            None => None,
        }
    }

    /// Marks the claimed job converged.
    pub fn complete(&mut self, id: JobId, now: DateTime<Utc>) -> Result<()> {
        let job = self.running_job(id)?;
        job.status = JobStatus::Succeeded;
        job.last_error = None;
        job.updated_at = now;
        Ok(())
    }

    /// Records an attempt failure: re-queues with exponential backoff, or
    /// dead-letters once the attempt budget is exhausted.
    pub fn fail(&mut self, id: JobId, error: &str, now: DateTime<Utc>) -> Result<JobStatus> {
        let job = self.running_job(id)?;
        let delay = job.policy.delay_for(job.attempts.max(1));
        job.last_error = Some(redact_reason(error, &[]));
        job.updated_at = now;
        job.status = if job.attempts >= job.policy.max_attempts {
            JobStatus::Dead
        } else {
            job.next_attempt_at = now + delay;
            JobStatus::Queued
        };
        Ok(job.status)
    }

    /// Holds a queued job out of claiming.
    pub fn pause(&mut self, id: JobId, now: DateTime<Utc>) -> Result<()> {
        let job = self
            .jobs
            .get_mut(&id.to_string())
            .ok_or_else(|| CoreError::NotFound(format!("job {id}")))?;
        if job.status != JobStatus::Queued {
            return Err(CoreError::Reconciliation(format!(
                "only queued jobs can be paused, job {id} is {:?}",
                job.status
            )));
        }
        job.status = JobStatus::Paused;
        job.updated_at = now;
        Ok(())
    }

    /// Returns a paused job to the queue.
    pub fn resume(&mut self, id: JobId, now: DateTime<Utc>) -> Result<()> {
        let job = self
            .jobs
            .get_mut(&id.to_string())
            .ok_or_else(|| CoreError::NotFound(format!("job {id}")))?;
        if job.status != JobStatus::Paused {
            return Err(CoreError::Reconciliation(format!(
                "only paused jobs can be resumed, job {id} is {:?}",
                job.status
            )));
        }
        job.status = JobStatus::Queued;
        job.next_attempt_at = now;
        job.updated_at = now;
        Ok(())
    }

    /// Explicitly requeues a dead-lettered job with a fresh attempt budget.
    pub fn requeue(&mut self, id: JobId, now: DateTime<Utc>) -> Result<()> {
        let job = self
            .jobs
            .get_mut(&id.to_string())
            .ok_or_else(|| CoreError::NotFound(format!("job {id}")))?;
        if job.status != JobStatus::Dead {
            return Err(CoreError::Reconciliation(format!(
                "only dead jobs can be requeued, job {id} is {:?}",
                job.status
            )));
        }
        job.status = JobStatus::Queued;
        job.attempts = 0;
        job.next_attempt_at = now;
        job.updated_at = now;
        Ok(())
    }

    /// Jobs that exhausted their retry budget.
    pub fn dead_letters(&self) -> Vec<JobId> {
        self.jobs
            .values()
            .filter(|job| job.status == JobStatus::Dead)
            .map(|job| job.id)
            .collect()
    }

    /// True when no job is queued, paused, or running.
    pub fn is_quiet(&self) -> bool {
        self.jobs.values().all(|job| !job.status.is_in_flight())
    }

    fn running_job(&mut self, id: JobId) -> Result<&mut Job> {
        let job = self
            .jobs
            .get_mut(&id.to_string())
            .ok_or_else(|| CoreError::NotFound(format!("job {id}")))?;
        if job.status != JobStatus::Running {
            return Err(CoreError::Reconciliation(format!(
                "job {id} is not running ({:?})",
                job.status
            )));
        }
        Ok(job)
    }
}

// ---------------------------------------------------------------------------
// Health aggregation (platform-owned checks only)
// ---------------------------------------------------------------------------

/// Shared health model across the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum HealthState {
    Unknown,
    Provisioning,
    Healthy,
    Degraded,
    Failed,
    Paused,
}

/// Where a health signal came from. Only platform probes feed aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckSource {
    PlatformProbe,
    AgentTool,
}

/// One health check result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    pub name: String,
    pub source: CheckSource,
    pub state: HealthState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl HealthCheck {
    pub fn probe(name: impl Into<String>, state: HealthState) -> Self {
        Self {
            name: name.into(),
            source: CheckSource::PlatformProbe,
            state,
            detail: None,
        }
    }

    /// An agent tool result. Aggregation never consumes these.
    pub fn agent_claim(name: impl Into<String>, state: HealthState) -> Self {
        Self {
            name: name.into(),
            source: CheckSource::AgentTool,
            state,
            detail: None,
        }
    }
}

/// Aggregated health plus the audit trail of what fed (and was excluded from)
/// the aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthReport {
    pub state: HealthState,
    pub checks: Vec<HealthCheck>,
    /// Names of agent-sourced claims that were excluded as non-proof.
    #[serde(default)]
    pub ignored_agent_claims: Vec<String>,
    pub at: DateTime<Utc>,
}

impl HealthReport {
    /// Aggregates platform-owned checks. Precedence is
    /// Failed > Degraded > Unknown > Provisioning > Paused > Healthy, so a
    /// single failed or missing check can never be outvoted by claims of
    /// health, and an empty check set is Unknown.
    pub fn from_checks(checks: Vec<HealthCheck>, at: DateTime<Utc>) -> Self {
        let mut platform = Vec::new();
        let mut ignored = Vec::new();
        for check in checks {
            if check.source == CheckSource::PlatformProbe {
                platform.push(check);
            } else {
                ignored.push(check.name);
            }
        }
        let state = aggregate_health(platform.iter().map(|c| c.state));
        Self {
            state,
            checks: platform,
            ignored_agent_claims: ignored,
            at,
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.state == HealthState::Healthy
    }
}

/// Folds platform check states with the documented precedence.
pub fn aggregate_health<I: Iterator<Item = HealthState>>(states: I) -> HealthState {
    let states: Vec<HealthState> = states.collect();
    if states.is_empty() {
        return HealthState::Unknown;
    }
    let has = |needle: HealthState| states.contains(&needle);
    if has(HealthState::Failed) {
        HealthState::Failed
    } else if has(HealthState::Degraded) {
        HealthState::Degraded
    } else if has(HealthState::Unknown) {
        HealthState::Unknown
    } else if has(HealthState::Provisioning) {
        HealthState::Provisioning
    } else if has(HealthState::Paused) {
        HealthState::Paused
    } else {
        HealthState::Healthy
    }
}

// ---------------------------------------------------------------------------
// Reconciliation results and controller contracts
// ---------------------------------------------------------------------------

/// How desired and observed state differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DriftKind {
    /// Desired entity is absent from observed state.
    Missing,
    /// Observed lags the desired version.
    Stale,
    /// Observed present but not healthy.
    Unhealthy,
    /// Observed terminal failure.
    Terminated,
}

/// One drift entry with a redacted detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drift {
    pub kind: DriftKind,
    pub detail: String,
}

/// Outcome of one controller pass. Reconciliation is repeatable: a controller
/// keeps returning mismatch results until a platform observation converges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciliation {
    pub controller: String,
    pub target: String,
    pub desired_version: u64,
    pub observed_version: u64,
    pub in_sync: bool,
    #[serde(default)]
    pub drift: Vec<Drift>,
    /// Idempotency keys of the actions this pass converged on (fresh or
    /// already in flight).
    #[serde(default)]
    pub actions: Vec<IdempotencyKey>,
    pub at: DateTime<Utc>,
}

impl Reconciliation {
    pub fn is_mismatch(&self) -> bool {
        !self.in_sync
    }

    /// Renders the pass for events/logs; contains no secret values.
    pub fn to_event_detail(&self) -> String {
        format!(
            "{} '{}' desired v{} observed v{} {} ({} drift, {} actions)",
            self.controller,
            self.target,
            self.desired_version,
            self.observed_version,
            if self.in_sync { "in sync" } else { "MISMATCH" },
            self.drift.len(),
            self.actions.len()
        )
    }
}

/// A controller compares persisted desired state with observed state and
/// enqueues idempotent jobs. Reconciliation is independent of any request or
/// agent session that triggered the desired-state change.
pub trait Controller {
    /// Stable controller name for events and results.
    fn name(&self) -> &'static str;

    /// One reconciliation pass against the shared job queue.
    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation>;
}

#[allow(clippy::too_many_arguments)]
fn result(
    controller: &dyn Controller,
    target: impl Into<String>,
    desired_version: u64,
    observed_version: u64,
    in_sync: bool,
    drift: Vec<Drift>,
    actions: Vec<IdempotencyKey>,
    at: DateTime<Utc>,
) -> Reconciliation {
    Reconciliation {
        controller: controller.name().to_string(),
        target: target.into(),
        desired_version,
        observed_version,
        in_sync,
        drift,
        actions,
        at,
    }
}

/// Converges the application's desired state with its observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationController {
    pub application_id: ApplicationId,
    pub desired: DesiredState,
    pub observed: ObservedState,
}

impl ApplicationController {
    pub fn new(
        application_id: ApplicationId,
        desired: DesiredState,
        observed: ObservedState,
    ) -> Self {
        Self {
            application_id,
            desired,
            observed,
        }
    }

    /// Records a fresh platform observation.
    pub fn observe(&mut self, observed: ObservedState) {
        self.observed = observed;
    }
}

impl Controller for ApplicationController {
    fn name(&self) -> &'static str {
        "application"
    }

    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation> {
        let mut drift = Vec::new();
        if self.observed.is_stale(&self.desired) {
            drift.push(Drift {
                kind: DriftKind::Stale,
                detail: format!(
                    "observation at v{} lags desired v{}",
                    self.observed.desired_version, self.desired.version
                ),
            });
        }
        if self.observed.status != ObservedStatus::Healthy {
            drift.push(Drift {
                kind: DriftKind::Unhealthy,
                detail: format!("observed status {:?}", self.observed.status),
            });
        }
        let actions = if drift.is_empty() {
            Vec::new()
        } else {
            let target = self.application_id.to_string();
            queue.enqueue(
                target.clone(),
                JobAction::Update,
                self.desired.version,
                RetryPolicy::default(),
                now,
            );
            vec![IdempotencyKey::new(
                &target,
                JobAction::Update,
                self.desired.version,
            )]
        };
        let in_sync = drift.is_empty();
        Ok(result(
            &*self,
            self.application_id.to_string(),
            self.desired.version,
            self.observed.desired_version,
            in_sync,
            drift,
            actions,
            now,
        ))
    }
}

/// Converges one platform-owned resource toward its desired presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceController {
    pub resource: Resource,
    /// Whether the desired state wants this resource present.
    pub desired_present: bool,
    /// Latest provider observation of existence; `None` means never observed.
    pub observed_present: Option<bool>,
}

impl ResourceController {
    pub fn new(resource: Resource) -> Self {
        Self {
            resource,
            desired_present: true,
            observed_present: None,
        }
    }

    /// Applies a provider/platform observation of the resource's existence
    /// and phase. Only platform sources may move lifecycle.
    pub fn observe(
        &mut self,
        present: bool,
        phase: Option<ResourcePhase>,
        source: ObservationSource,
        reason: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<()> {
        self.observed_present = Some(present);
        if let Some(phase) = phase {
            if phase != self.resource.phase {
                self.resource.observe(phase, source, reason, at)?;
            }
        }
        Ok(())
    }

    /// Health contributed by this resource; never Healthy unless a platform
    /// observation put it in `ready`.
    pub fn health(&self) -> HealthState {
        if self.observed_present == Some(false) {
            return HealthState::Unknown;
        }
        self.resource.phase.health()
    }
}

impl Controller for ResourceController {
    fn name(&self) -> &'static str {
        "resource"
    }

    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation> {
        let target = self.resource.id.to_string();
        let mut drift = Vec::new();
        let mut actions = Vec::new();
        let missing = self.desired_present && self.observed_present != Some(true);
        if missing {
            drift.push(Drift {
                kind: DriftKind::Missing,
                detail: format!(
                    "desired {} present at v{} but observed {:?}",
                    self.resource.kind, self.resource.desired_version, self.observed_present
                ),
            });
            let version = self.resource.convergence_version();
            queue.enqueue(
                target.clone(),
                JobAction::Provision,
                version,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&target, JobAction::Provision, version));
        }
        if let Some(failure) = &self.resource.failure {
            drift.push(Drift {
                kind: DriftKind::Terminated,
                detail: format!(
                    "attempt {}/{} failed: {}",
                    failure.attempts, failure.policy.max_attempts, failure.reason
                ),
            });
        }
        if self.desired_present
            && self.observed_present == Some(true)
            && self.resource.phase != ResourcePhase::Ready
            && !missing
        {
            drift.push(Drift {
                kind: DriftKind::Unhealthy,
                detail: format!("resource phase {:?}", self.resource.phase),
            });
        }
        if !self.desired_present && self.resource.phase != ResourcePhase::Deleted {
            drift.push(Drift {
                kind: DriftKind::Stale,
                detail: format!(
                    "resource should be deleted but is {:?}",
                    self.resource.phase
                ),
            });
            let version = self.resource.convergence_version();
            queue.enqueue(
                target.clone(),
                JobAction::Delete,
                version,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&target, JobAction::Delete, version));
        }
        let in_sync = drift.is_empty();
        Ok(result(
            &*self,
            target,
            self.resource.desired_version,
            self.observed_present
                .map_or(0, |_| self.resource.desired_version),
            in_sync,
            drift,
            actions,
            now,
        ))
    }
}

/// Converges one bound capability. A capability stays `provisioning` until a
/// controller observes readiness — an agent tool result never flips it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityController {
    pub capability: String,
    pub resource_id: ResourceId,
    pub desired_ready: bool,
    pub phase: ResourcePhase,
    /// Count of agent completion claims received (recorded, never trusted).
    pub agent_claims_ignored: u32,
}

impl CapabilityController {
    pub fn new(capability: impl Into<String>, resource_id: ResourceId) -> Self {
        Self {
            capability: capability.into(),
            resource_id,
            desired_ready: true,
            phase: ResourcePhase::Provisioning,
            agent_claims_ignored: 0,
        }
    }

    /// Platform observation of the backing resource's readiness.
    pub fn observe_readiness(&mut self, phase: ResourcePhase) {
        self.phase = phase;
    }

    /// An agent reports its provisioning tool returned an accepted request.
    /// Accepted as an event only: readiness still requires a platform
    /// observation, so this never changes the phase.
    pub fn accept_agent_completion(&mut self) {
        self.agent_claims_ignored += 1;
    }

    pub fn health(&self) -> HealthState {
        self.phase.health()
    }
}

impl Controller for CapabilityController {
    fn name(&self) -> &'static str {
        "capability"
    }

    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation> {
        let target = self.resource_id.to_string();
        let mut drift = Vec::new();
        let mut actions = Vec::new();
        if self.desired_ready && self.phase != ResourcePhase::Ready {
            drift.push(Drift {
                kind: DriftKind::Unhealthy,
                detail: format!("capability '{}' phase {:?}", self.capability, self.phase),
            });
            queue.enqueue(
                target.clone(),
                JobAction::Provision,
                1,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&target, JobAction::Provision, 1));
        }
        let in_sync = drift.is_empty();
        Ok(result(
            &*self,
            target,
            u64::from(self.desired_ready),
            self.phase as u64,
            in_sync,
            drift,
            actions,
            now,
        ))
    }
}

/// Converges one preview: desired live state plus health-gated availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewController {
    pub preview_id: PreviewId,
    pub desired_live: bool,
    pub observed_live: bool,
    pub observed_healthy: bool,
}

impl PreviewController {
    pub fn new(preview_id: PreviewId) -> Self {
        Self {
            preview_id,
            desired_live: true,
            observed_live: false,
            observed_healthy: false,
        }
    }

    pub fn observe(&mut self, live: bool, healthy: bool) {
        self.observed_live = live;
        self.observed_healthy = healthy;
    }

    pub fn health(&self) -> HealthState {
        if !self.desired_live {
            return HealthState::Paused;
        }
        match (self.observed_live, self.observed_healthy) {
            (false, _) => HealthState::Provisioning,
            (true, true) => HealthState::Healthy,
            (true, false) => HealthState::Degraded,
        }
    }
}

impl Controller for PreviewController {
    fn name(&self) -> &'static str {
        "preview"
    }

    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation> {
        let target = self.preview_id.to_string();
        let mut drift = Vec::new();
        let mut actions = Vec::new();
        if self.desired_live && !self.observed_live {
            drift.push(Drift {
                kind: DriftKind::Missing,
                detail: "preview desired live but not running".to_string(),
            });
            queue.enqueue(
                target.clone(),
                JobAction::Provision,
                1,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&target, JobAction::Provision, 1));
        }
        if self.desired_live && self.observed_live && !self.observed_healthy {
            drift.push(Drift {
                kind: DriftKind::Unhealthy,
                detail: "preview failing health checks".to_string(),
            });
            queue.enqueue(
                target.clone(),
                JobAction::Verify,
                1,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&target, JobAction::Verify, 1));
        }
        let in_sync = drift.is_empty();
        Ok(result(
            &*self,
            target,
            u64::from(self.desired_live),
            u64::from(self.observed_live),
            in_sync,
            drift,
            actions,
            now,
        ))
    }
}

/// Converges a deployment to its desired artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentController {
    pub key: String,
    pub desired_artifact: String,
    pub observed_artifact: Option<String>,
    pub observed_ready: bool,
}

impl DeploymentController {
    pub fn new(key: impl Into<String>, desired_artifact: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            desired_artifact: desired_artifact.into(),
            observed_artifact: None,
            observed_ready: false,
        }
    }

    pub fn observe(&mut self, artifact: impl Into<String>, ready: bool) {
        self.observed_artifact = Some(artifact.into());
        self.observed_ready = ready;
    }

    pub fn health(&self) -> HealthState {
        match (&self.observed_artifact, self.observed_ready) {
            (Some(artifact), true) if *artifact == self.desired_artifact => HealthState::Healthy,
            (Some(_), true) => HealthState::Degraded,
            (Some(_), false) => HealthState::Provisioning,
            (None, _) => HealthState::Unknown,
        }
    }
}

impl Controller for DeploymentController {
    fn name(&self) -> &'static str {
        "deployment"
    }

    fn reconcile(&mut self, queue: &mut JobQueue, now: DateTime<Utc>) -> Result<Reconciliation> {
        let mut drift = Vec::new();
        let mut actions = Vec::new();
        if self.observed_artifact.as_deref() != Some(self.desired_artifact.as_str()) {
            drift.push(Drift {
                kind: DriftKind::Stale,
                detail: format!(
                    "observed {:?} != desired '{}'",
                    self.observed_artifact, self.desired_artifact
                ),
            });
            queue.enqueue(
                self.key.clone(),
                JobAction::Rollout,
                1,
                RetryPolicy::default(),
                now,
            );
            actions.push(IdempotencyKey::new(&self.key, JobAction::Rollout, 1));
        }
        let in_sync = drift.is_empty();
        Ok(result(
            &*self,
            self.key.clone(),
            1,
            u64::from(self.observed_artifact.is_some()),
            in_sync,
            drift,
            actions,
            now,
        ))
    }
}
