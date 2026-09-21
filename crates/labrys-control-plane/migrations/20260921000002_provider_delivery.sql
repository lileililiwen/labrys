-- Provider operations, registry deliveries, and domain deliveries.
--
-- Provider operations record one scoped adapter call behind a controller job:
-- the idempotency key is globally unique so duplicate requests return the
-- existing outcome instead of repeating the side effect. Registry deliveries
-- record the recorded vs reported digest evidence for one OCI push. Domain
-- deliveries record the separately observable DNS, TLS, and traffic state for
-- one hostname. Free-text detail/failure fields are redacted by the writers
-- before INSERT, never only on read; credential values never reach these rows,
-- only secret reference ids.

CREATE TABLE provider_operations (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL,
    environment_id  text,
    resource_id     text        NOT NULL,
    capability      text        NOT NULL,
    provider_key    text        NOT NULL,
    provider_kind   text        NOT NULL,
    action          text        NOT NULL,
    idempotency_key text        NOT NULL UNIQUE,
    trace_id        text        NOT NULL,
    secret_refs     jsonb       NOT NULL DEFAULT '[]'::jsonb,
    phase           text        NOT NULL,
    failure         text        NOT NULL DEFAULT '',
    usage_units     bigint      NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT provider_operations_action_check CHECK (action IN
        ('provision', 'readiness-check', 'replace', 'migrate', 'adopt', 'delete')),
    CONSTRAINT provider_operations_phase_check CHECK (phase IN
        ('requested', 'provisioning', 'ready', 'degraded', 'failed', 'deleting', 'deleted'))
);

CREATE INDEX provider_operations_application_idx ON provider_operations (application_id);
CREATE INDEX provider_operations_resource_idx ON provider_operations (resource_id);
CREATE INDEX provider_operations_trace_idx ON provider_operations (trace_id);

CREATE TABLE registry_deliveries (
    id              text        PRIMARY KEY,
    application_id  text        NOT NULL,
    reference       text        NOT NULL,
    recorded_digest text        NOT NULL,
    reported_digest text        NOT NULL DEFAULT '',
    revision_sha    text        NOT NULL,
    status          text        NOT NULL,
    detail          text        NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    CONSTRAINT registry_deliveries_status_check CHECK (status IN
        ('pending', 'verified', 'mismatch', 'refused'))
);

CREATE INDEX registry_deliveries_application_idx ON registry_deliveries (application_id);

CREATE TABLE domain_deliveries (
    id              text        PRIMARY KEY,
    domain_id       text        NOT NULL,
    hostname        text        NOT NULL,
    deployment_id   text,
    dns             text        NOT NULL,
    tls             text        NOT NULL,
    traffic_attached boolean    NOT NULL DEFAULT FALSE,
    detail          text        NOT NULL DEFAULT '',
    updated_at      timestamptz NOT NULL,
    CONSTRAINT domain_deliveries_dns_check CHECK (dns IN
        ('pending', 'propagated', 'failed')),
    CONSTRAINT domain_deliveries_tls_check CHECK (tls IN
        ('pending', 'issued', 'renewing', 'failed'))
);

CREATE INDEX domain_deliveries_hostname_idx ON domain_deliveries (hostname);
