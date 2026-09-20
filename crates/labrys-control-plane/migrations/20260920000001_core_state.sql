-- Core application aggregate and environment-scoped state.
-- The application is the aggregate root; environments, desired state, and
-- observed state are normalized so a preview/development write can never touch
-- production rows and so desired vs observed stay independently writable.

CREATE TABLE applications (
    id                text        PRIMARY KEY,
    name              text        NOT NULL,
    origin            jsonb       NOT NULL,
    status            text        NOT NULL,
    created_at        timestamptz NOT NULL,
    updated_at        timestamptz NOT NULL,
    created_by        text        NOT NULL,
    updated_by        text,
    audit_event_ids   jsonb       NOT NULL DEFAULT '[]'::jsonb,
    collections       jsonb       NOT NULL DEFAULT '{}'::jsonb,
    CONSTRAINT applications_status_check
        CHECK (status IN ('draft', 'active', 'archived'))
);

CREATE TABLE environments (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    kind            jsonb       NOT NULL,
    name            text        NOT NULL,
    refs            jsonb       NOT NULL DEFAULT '{}'::jsonb,
    -- Environment isolation: every environment belongs to exactly one
    -- application and its name is unique within that application.
    CONSTRAINT environments_ownership UNIQUE (application_id, name)
);

CREATE INDEX environments_application_idx ON environments (application_id);

-- Desired state is versioned and monotonic; optimistic updates guard the
-- generation so a stale writer can never overwrite newer state.
CREATE TABLE desired_states (
    application_id  text     PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    version         bigint   NOT NULL CHECK (version >= 1),
    profiles        jsonb    NOT NULL DEFAULT '{}'::jsonb,
    config          jsonb    NOT NULL DEFAULT 'null'::jsonb
);

-- Observed state lives in its own table so a failed observation can never
-- rewrite desired state.
CREATE TABLE observed_states (
    application_id   text        PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    desired_version  bigint      NOT NULL,
    status           text        NOT NULL,
    last_observed_at timestamptz NOT NULL,
    detail           text,
    CONSTRAINT observed_states_status_check
        CHECK (status IN ('unknown', 'healthy', 'degraded', 'unhealthy'))
);
