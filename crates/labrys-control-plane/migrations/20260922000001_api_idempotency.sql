-- Durable idempotency records for API/CLI mutations.
--
-- Every mutating API command stores its response under the caller's
-- idempotency key before returning. A retried request with the same key
-- replays the recorded response instead of enqueueing a second job, so the
-- side effect executes once. Free-text responses are redacted by the writers
-- before INSERT, never only on read.

CREATE TABLE idempotency_records (
    key         text        PRIMARY KEY,
    command     text        NOT NULL,
    response    jsonb       NOT NULL,
    created_at  timestamptz NOT NULL
);
