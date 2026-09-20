-- Durable, idempotent controller jobs and the worker claim protocol.
--
-- The idempotency key is globally unique so a duplicate request for work that
-- is already active or already terminal returns the existing job rather than
-- enqueueing a second side effect. Claim uses a lease plus FOR UPDATE SKIP
-- LOCKED so exactly one worker wins a ready job; an expired lease is made
-- claimable again by recovery.

CREATE TABLE jobs (
    id                text        PRIMARY KEY,
    target            text        NOT NULL,
    action            text        NOT NULL,
    idempotency_key   text        NOT NULL UNIQUE,
    status            text        NOT NULL,
    attempts          integer     NOT NULL DEFAULT 0,
    policy            jsonb       NOT NULL,
    next_attempt_at   timestamptz NOT NULL,
    last_error        text,
    lease_owner       text,
    lease_expires_at  timestamptz,
    enqueued_at       timestamptz NOT NULL,
    updated_at        timestamptz NOT NULL,
    CONSTRAINT jobs_status_check
        CHECK (status IN ('queued', 'paused', 'running', 'succeeded', 'dead')),
    CONSTRAINT jobs_action_check
        CHECK (action IN ('provision', 'update', 'rollout', 'verify', 'delete'))
);

-- Claim candidates: queued jobs whose backoff has elapsed.
CREATE INDEX jobs_claim_idx ON jobs (next_attempt_at, enqueued_at)
    WHERE status = 'queued';

-- Expired-lease recovery candidates.
CREATE INDEX jobs_lease_idx ON jobs (lease_expires_at)
    WHERE status = 'running';
