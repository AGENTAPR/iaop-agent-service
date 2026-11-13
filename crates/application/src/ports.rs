//! Ports (traits) that abstract infrastructure dependencies.
//!
//! The application layer depends ONLY on these traits, never on SQLx,
//! Axum, HTTP clients, or Keycloak. Implementations live in
//! `iaop_agent_service-infrastructure`.
//!
//! Mutating repository methods take `&mut Transaction` — the caller
//! (command handler) owns the commit boundary so that aggregate,
//! history, audit, outbox, and idempotency writes are atomic.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use iaop_agent_service_domain::{
    ActivationDecision, ActivationOperation, Agent, AgentAppointment, AgentCode, AgentId,
    AgentIdentityLink, AppointmentId, IdentityLinkId, LifecycleTransition,
};

use crate::error::AppError;
use crate::principal::Principal;

// ---------------------------------------------------------------------------
// Agent store port
// ---------------------------------------------------------------------------

/// Persistence port for the Agent aggregate and its owned records.
pub trait AgentStore: Send + Sync {
    // -- commands (transactional) -------------------------------------
    async fn insert_agent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        agent: &Agent,
    ) -> Result<(), AppError>;

    /// Loads the agent `FOR UPDATE` (row lock) inside `tx` — serializes
    /// concurrent lifecycle commands on the same aggregate.
    async fn load_agent_for_update(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: AgentId,
    ) -> Result<Option<Agent>, AppError>;

    /// Optimistic-lock update: succeeds only when stored `version`
    /// matches `agent.version() - 1`. Returns `Ok(false)` on conflict.
    async fn update_agent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        agent: &Agent,
        updated_by: &str,
    ) -> Result<bool, AppError>;

    async fn insert_lifecycle_history(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &LifecycleTransition,
        actor: &str,
    ) -> Result<(), AppError>;

    async fn insert_audit(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &AuditRecord,
    ) -> Result<(), AppError>;

    async fn insert_outbox(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        event: &IntegrationEvent,
    ) -> Result<(), AppError>;

    async fn insert_appointment(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        appointment: &AgentAppointment,
    ) -> Result<(), AppError>;

    async fn insert_identity_link(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        link: &AgentIdentityLink,
    ) -> Result<(), AppError>;

    async fn update_identity_link(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        link: &AgentIdentityLink,
    ) -> Result<(), AppError>;

    // -- queries (pool) ------------------------------------------------
    async fn find_agent_by_id(&self, pool: &PgPool, id: AgentId)
    -> Result<Option<Agent>, AppError>;

    async fn find_agent_by_code(
        &self,
        pool: &PgPool,
        code: &AgentCode,
    ) -> Result<Option<Agent>, AppError>;

    async fn find_appointment(
        &self,
        pool: &PgPool,
        id: AppointmentId,
    ) -> Result<Option<AgentAppointment>, AppError>;

    async fn find_identity_link(
        &self,
        pool: &PgPool,
        id: IdentityLinkId,
    ) -> Result<Option<AgentIdentityLink>, AppError>;

    async fn find_identity_link_by_subject(
        &self,
        pool: &PgPool,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<AgentIdentityLink>, AppError>;

    /// Keyset pagination over the UUIDv7 primary key (time-ordered).
    async fn list_agents(
        &self,
        pool: &PgPool,
        after: Option<AgentId>,
        limit: i64,
    ) -> Result<Vec<Agent>, AppError>;

    async fn list_appointments(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
    ) -> Result<Vec<AgentAppointment>, AppError>;

    async fn list_identity_links(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
    ) -> Result<Vec<AgentIdentityLink>, AppError>;

    async fn list_lifecycle_history(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
        limit: i64,
    ) -> Result<Vec<LifecycleHistoryRecord>, AppError>;
}

// ---------------------------------------------------------------------------
// Idempotency port
// ---------------------------------------------------------------------------

/// Outcome of reserving an idempotency key.
#[derive(Debug)]
pub enum IdempotencyReserve {
    /// This request owns the key — proceed and `complete` it.
    Acquired { record_id: Uuid },
    /// Identical request already completed — replay stored response.
    Replay { status: i32, body: Value },
    /// Same key, different payload, or in-flight duplicate.
    Conflict,
}

/// Durable idempotency store. Reservation happens INSIDE the business
/// transaction so a rollback never leaves a phantom record.
pub trait IdempotencyStore: Send + Sync {
    async fn reserve(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal_scope: &str,
        operation: &str,
        key: &str,
        fingerprint: &str,
        ttl: Duration,
    ) -> Result<IdempotencyReserve, AppError>;

    async fn complete(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record_id: Uuid,
        status: i32,
        body: Value,
    ) -> Result<(), AppError>;
}

// ---------------------------------------------------------------------------
// Activation authorization port
// ---------------------------------------------------------------------------

/// External authorization port for activation decisions.
///
/// Implementations MUST NOT be bypassable: production wiring uses a
/// fail-closed adapter until real policy services (Eligibility,
/// Onboarding, Certification) are integrated. A fixture adapter exists
/// ONLY for local/E2E testing and is refused in production config.
pub trait ActivationAuthorizationPort: Send + Sync {
    async fn authorize(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Result<ActivationDecision, AppError>;
}

// ---------------------------------------------------------------------------
// Token validation port
// ---------------------------------------------------------------------------

/// OIDC token claims needed by the application layer.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TokenClaims {
    pub sub: String,
    pub iss: String,
    pub exp: usize,
    pub iat: Option<usize>,
    pub nbf: Option<usize>,
    /// Space-separated scopes (Keycloak `scope` claim).
    pub scope: Option<String>,
    /// Keycloak realm roles.
    pub realm_access: Option<RealmAccess>,
    /// Optional agent-scope claim for future delegated access.
    pub agent_scope: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RealmAccess {
    pub roles: Vec<String>,
}

/// Validates bearer tokens and produces an application [`Principal`].
pub trait TokenValidator: Send + Sync {
    async fn validate(&self, token: &str) -> Result<Principal, AppError>;
}

// ---------------------------------------------------------------------------
// Event transport port (outbox publisher)
// ---------------------------------------------------------------------------

/// Canonical integration event written to the transactional outbox.
///
/// Field set mirrors `iaop-event-contracts` `EventEnvelope`
/// (camelCase on the wire).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationEvent {
    pub event_id: Uuid,
    pub event_type: String,
    pub event_version: u32,
    pub source: String,
    pub aggregate_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_version: Option<u64>,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub payload: Value,
}

/// Delivers a single outbox event to the outside world.
///
/// Phase 1 ships `LogTransport` (marks delivery in logs only — NOT a
/// broker). A real broker adapter plugs in here once the org broker
/// decision is made. Delivery semantics: at-least-once.
pub trait EventTransport: Send + Sync {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Supporting records
// ---------------------------------------------------------------------------

/// Row shape for lifecycle history reads.
#[derive(Debug)]
pub struct LifecycleHistoryRecord {
    pub id: Uuid,
    pub agent_id: AgentId,
    pub from_status: Option<String>,
    pub to_status: String,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub effective_at: DateTime<Utc>,
    pub performed_at: DateTime<Utc>,
    pub performed_by: Option<String>,
    pub correlation_id: Option<Uuid>,
    pub command_id: Uuid,
    pub aggregate_version: i64,
}

/// Audit record persisted in `agent_audit_events`.
#[derive(Debug)]
pub struct AuditRecord {
    pub id: Uuid,
    pub agent_id: AgentId,
    pub actor: Option<String>,
    pub action: String,
    pub before_state: Option<Value>,
    pub after_state: Option<Value>,
    pub reason: Option<String>,
    pub source: String,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
    pub command_id: Option<Uuid>,
}
