-- Bounded container execution and health-gated preview state.
--
-- Executions record one build or run under explicit sandbox limits; previews
-- record the health-gated access state for one isolated session workspace.
-- Free-text detail/logs are redacted by the writers before INSERT, never only
-- on read. These tables are the durable memory of the container and preview
-- runtime; events, logs, and evidence remain the attributable streams.

CREATE TABLE executions (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL,
    environment_id  text,
    workspace_id    text,
    session_id      text,
    trace_id        text        NOT NULL,
    kind            text        NOT NULL,
    image           text,
    container_id    text,
    container_name  text,
    limits          jsonb       NOT NULL DEFAULT '{}'::jsonb,
    status          text        NOT NULL,
    detail          text        NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT executions_kind_check CHECK (kind IN ('build', 'run')),
    CONSTRAINT executions_status_check CHECK (status IN
        ('running', 'succeeded', 'failed', 'cancelled'))
);

CREATE INDEX executions_application_idx ON executions (application_id);
CREATE INDEX executions_session_idx ON executions (session_id);
CREATE INDEX executions_trace_idx ON executions (trace_id);

CREATE TABLE previews (
    id              text        PRIMARY KEY,
    workspace_id    text        NOT NULL,
    session_id      text        NOT NULL,
    application_id  text        NOT NULL,
    environment_id  text,
    status          text        NOT NULL,
    url             text,
    container_id    text,
    container_name  text,
    expires_at      timestamptz,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT previews_status_check CHECK (status IN
        ('pending', 'building', 'available', 'unavailable', 'expired', 'revoked'))
);

CREATE INDEX previews_application_idx ON previews (application_id);
CREATE INDEX previews_session_idx ON previews (session_id);
CREATE INDEX previews_workspace_idx ON previews (workspace_id);
