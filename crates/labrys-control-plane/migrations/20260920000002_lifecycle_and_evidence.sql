-- Lifecycle records and durable, attributable, redacted evidence.

CREATE TABLE resources (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    environment_id  text        REFERENCES environments(id) ON DELETE CASCADE,
    kind            text        NOT NULL,
    desired_version bigint      NOT NULL,
    phase           text        NOT NULL,
    generation      integer     NOT NULL DEFAULT 0,
    failure         jsonb,
    transitions     jsonb       NOT NULL DEFAULT '[]'::jsonb,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT resources_phase_check CHECK (phase IN
        ('requested', 'provisioning', 'ready', 'degraded', 'failed', 'deleting', 'deleted'))
);

CREATE INDEX resources_application_idx ON resources (application_id);
CREATE INDEX resources_environment_idx ON resources (environment_id);

CREATE TABLE deployments (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    environment_id  text        NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    phase           text        NOT NULL,
    record          jsonb       NOT NULL,
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT deployments_phase_check CHECK (phase IN
        ('pending', 'building', 'deploying', 'healthy', 'degraded', 'failed', 'superseded', 'rolled_back'))
);

CREATE INDEX deployments_application_idx ON deployments (application_id);
CREATE INDEX deployments_environment_idx ON deployments (environment_id);

CREATE TABLE capabilities (
    id                text        PRIMARY KEY,
    application_id    text        NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    environment_name  text        NOT NULL,
    capability        text        NOT NULL,
    provider          text        NOT NULL,
    mode              text        NOT NULL,
    binding           text        NOT NULL,
    record            jsonb       NOT NULL,
    updated_at        timestamptz NOT NULL,
    CONSTRAINT capabilities_mode_check CHECK (mode IN ('managed', 'external', 'adopted'))
);

CREATE INDEX capabilities_application_idx ON capabilities (application_id);

-- Attributable platform events, queryable by correlation and resource identity.
CREATE TABLE events (
    sequence        bigserial   PRIMARY KEY,
    at              timestamptz NOT NULL,
    actor           jsonb       NOT NULL,
    trace_id        text        NOT NULL,
    session_id      text,
    application_id  text,
    environment_id  text,
    action          text        NOT NULL,
    resource_kind   text,
    resource_id     text,
    before_state    text,
    after_state     text,
    result          jsonb       NOT NULL
);

CREATE INDEX events_application_idx ON events (application_id);
CREATE INDEX events_environment_idx ON events (environment_id);
CREATE INDEX events_session_idx ON events (session_id);
CREATE INDEX events_trace_idx ON events (trace_id);

-- Separated, redacted log streams.
CREATE TABLE logs (
    sequence        bigserial   PRIMARY KEY,
    at              timestamptz NOT NULL,
    source          text        NOT NULL,
    level           text        NOT NULL,
    trace_id        text        NOT NULL,
    session_id      text,
    application_id  text,
    environment_id  text,
    message         text        NOT NULL
);

CREATE INDEX logs_source_idx ON logs (source);
CREATE INDEX logs_application_idx ON logs (application_id);

-- Append-only, hash-chained audit trail. The database assigns one unambiguous
-- sequence; UPDATE and DELETE are refused so the chain cannot be rewritten.
CREATE TABLE audit_log (
    sequence    bigint      PRIMARY KEY,
    at          timestamptz NOT NULL,
    actor       text        NOT NULL,
    action      text        NOT NULL,
    target      text        NOT NULL,
    detail      text        NOT NULL,
    prev_hash   text        NOT NULL,
    hash        text        NOT NULL
);

CREATE OR REPLACE FUNCTION audit_log_is_append_only() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only: % is not permitted', TG_OP;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_log_append_only
    BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_is_append_only();

-- Verification evidence, queryable by stage and resource.
CREATE TABLE evidence (
    id              text        PRIMARY KEY,
    stage           text        NOT NULL,
    check_name      text        NOT NULL,
    passed          boolean     NOT NULL,
    summary         text        NOT NULL,
    resource        jsonb,
    recovery        text,
    at              timestamptz NOT NULL,
    application_id  text,
    environment_id  text
);

CREATE INDEX evidence_stage_idx ON evidence (stage);
CREATE INDEX evidence_application_idx ON evidence (application_id);

-- Per-application usage metering.
CREATE TABLE usage_records (
    id                     bigserial   PRIMARY KEY,
    application_id         text        NOT NULL,
    environment_id         text        NOT NULL,
    period_start           timestamptz NOT NULL,
    period_end             timestamptz NOT NULL,
    cpu_millicore_seconds  bigint      NOT NULL DEFAULT 0,
    memory_mb_seconds      bigint      NOT NULL DEFAULT 0,
    build_seconds          bigint      NOT NULL DEFAULT 0,
    requests               bigint      NOT NULL DEFAULT 0,
    egress_bytes           bigint      NOT NULL DEFAULT 0
);

CREATE INDEX usage_application_idx ON usage_records (application_id);
