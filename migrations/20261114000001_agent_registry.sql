-- Agent Registry & Identity foundation schema.
--
-- Conventions (IAOP_DATABASE_MIGRATION_STANDARD):
--   snake_case, uuid PKs, timestamptz, check constraints on enums,
--   forward-only. Agent Service owns this database exclusively —
--   no cross-service FKs.

-- ---------------------------------------------------------------- agents

CREATE TABLE agents (
    id              UUID PRIMARY KEY,
    person_id       UUID        NOT NULL,
    agent_code      VARCHAR(40) NOT NULL,
    lifecycle_status VARCHAR(32) NOT NULL
                    CHECK (lifecycle_status IN
                        ('DRAFT','PENDING_ACTIVATION','ACTIVE','SUSPENDED','TERMINATED')),
    version         BIGINT      NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      TEXT,
    updated_by      TEXT
);

-- Phase 1 assumption: agent codes are globally unique within the
-- registry namespace. If insurer/company scoping is introduced later,
-- replace with a composite unique index (scope, agent_code).
CREATE UNIQUE INDEX agents_agent_code_key ON agents (agent_code);
CREATE INDEX agents_person_id_idx ON agents (person_id);
CREATE INDEX agents_lifecycle_status_idx ON agents (lifecycle_status);
CREATE INDEX agents_created_at_idx ON agents (created_at, id);

-- ------------------------------------------------------- agent_appointments

CREATE TABLE agent_appointments (
    id                      UUID PRIMARY KEY,
    agent_id                UUID        NOT NULL REFERENCES agents (id),
    appointment_reference   VARCHAR(100),
    company_reference       VARCHAR(100),
    appointment_type        VARCHAR(40) NOT NULL,
    effective_from          DATE        NOT NULL,
    effective_to            DATE,
    status                  VARCHAR(32) NOT NULL DEFAULT 'DRAFT'
                            CHECK (status IN ('DRAFT','ACTIVE','ENDED','CANCELLED')),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (effective_to IS NULL OR effective_to >= effective_from)
);

CREATE INDEX agent_appointments_agent_id_idx ON agent_appointments (agent_id);
-- Active-appointment lookup for authorization checks.
CREATE INDEX agent_appointments_active_idx
    ON agent_appointments (agent_id, status, effective_from, effective_to);
CREATE UNIQUE INDEX agent_appointments_ref_key
    ON agent_appointments (appointment_reference)
    WHERE appointment_reference IS NOT NULL;

-- ------------------------------------------------- agent_lifecycle_history
-- Append-only audit of every lifecycle transition.

CREATE TABLE agent_lifecycle_history (
    id                  UUID PRIMARY KEY,
    agent_id            UUID        NOT NULL REFERENCES agents (id),
    from_status         VARCHAR(32),
    to_status           VARCHAR(32) NOT NULL
                        CHECK (to_status IN
                            ('DRAFT','PENDING_ACTIVATION','ACTIVE','SUSPENDED','TERMINATED')),
    reason_code         VARCHAR(80),
    reason_text         TEXT,
    effective_at        TIMESTAMPTZ NOT NULL,
    performed_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    performed_by        TEXT,
    correlation_id      UUID,
    command_id          UUID        NOT NULL,
    aggregate_version   BIGINT      NOT NULL CHECK (aggregate_version >= 0),
    -- One transition per (agent, command): idempotent command identity.
    UNIQUE (agent_id, command_id)
);

CREATE INDEX agent_lifecycle_history_agent_idx
    ON agent_lifecycle_history (agent_id, effective_at);
CREATE INDEX agent_lifecycle_history_command_idx
    ON agent_lifecycle_history (command_id);

-- ------------------------------------------------------- agent_audit_events
-- Append-only business audit trail (who did what, minimal state diff).

CREATE TABLE agent_audit_events (
    id              UUID PRIMARY KEY,
    agent_id        UUID        NOT NULL REFERENCES agents (id),
    actor           TEXT,
    action          VARCHAR(80) NOT NULL,
    before_state    JSONB,
    after_state     JSONB,
    reason          TEXT,
    source          VARCHAR(80) NOT NULL DEFAULT 'iaop-agent-service',
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    correlation_id  UUID,
    command_id      UUID
);

CREATE INDEX agent_audit_events_agent_idx
    ON agent_audit_events (agent_id, occurred_at);
CREATE INDEX agent_audit_events_command_idx
    ON agent_audit_events (command_id);

-- ------------------------------------------------------ agent_identity_links
-- Maps external IdP (issuer, subject) to an agent. Historical: unlink
-- sets unlinked_at; rows are never deleted. Active mapping is unique
-- per (issuer, subject) via partial index.

CREATE TABLE agent_identity_links (
    id                  UUID PRIMARY KEY,
    agent_id            UUID        NOT NULL REFERENCES agents (id),
    identity_provider   VARCHAR(80) NOT NULL,
    issuer              VARCHAR(512) NOT NULL,
    subject             VARCHAR(255) NOT NULL,
    status              VARCHAR(32) NOT NULL DEFAULT 'ACTIVE'
                        CHECK (status IN ('ACTIVE','UNLINKED')),
    linked_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    unlinked_at         TIMESTAMPTZ
);

-- One ACTIVE link per (issuer, subject): `sub` is only unique per issuer.
CREATE UNIQUE INDEX agent_identity_links_active_subject
    ON agent_identity_links (issuer, subject)
    WHERE status = 'ACTIVE';
CREATE INDEX agent_identity_links_agent_idx
    ON agent_identity_links (agent_id, status);

-- -------------------------------------------------------- idempotency_records
-- Durable idempotency for mutation endpoints. Key is scoped to
-- (principal, operation); fingerprint detects payload reuse conflicts.

CREATE TABLE idempotency_records (
    id                  UUID PRIMARY KEY,
    idempotency_key     VARCHAR(255) NOT NULL,
    principal_scope     VARCHAR(255) NOT NULL,
    operation           VARCHAR(120) NOT NULL,
    request_fingerprint VARCHAR(128) NOT NULL,
    response_status     INTEGER,
    response_body       JSONB,
    state               VARCHAR(32) NOT NULL DEFAULT 'IN_PROGRESS'
                        CHECK (state IN ('IN_PROGRESS','COMPLETED','FAILED')),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at          TIMESTAMPTZ,
    UNIQUE (principal_scope, operation, idempotency_key)
);

-- Cleanup of expired records (retention: 24h default, see config).
CREATE INDEX idempotency_records_expires_idx
    ON idempotency_records (expires_at)
    WHERE expires_at IS NOT NULL;
