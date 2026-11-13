//! Application commands: mutating use cases.
//!
//! Every command follows the same transaction shape:
//!
//! ```text
//! BEGIN
//!   reserve idempotency key (unique principal+op+key)
//!   load aggregate FOR UPDATE          -- serializes concurrent commands
//!   check expected version (If-Match)  -- optimistic precondition
//!   run domain behavior                -- validates transitions/invariants
//!   update aggregate (version-checked)
//!   insert lifecycle history + audit + outbox event(s)
//!   store idempotency response
//! COMMIT
//! ```
//!
//! Any failure rolls back everything — no partial business effect, no
//! orphan outbox rows, no stuck idempotency records.

use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use iaop_agent_service_domain::{
    ActivationAuthorization, ActivationDecision, ActivationOperation, Agent, AgentAppointment,
    AgentCode, AgentError, AgentId, AgentIdentityLink, AgentStatus, AppointmentId, CommandId,
    IdentityLinkId, LifecycleTransition, PersonId, SuspensionReason, TerminationReason,
    verify_decision,
};

use crate::error::AppError;
use crate::ports::{
    ActivationAuthorizationPort, AgentStore, AuditRecord, IdempotencyReserve, IdempotencyStore,
    IntegrationEvent,
};
use crate::principal::Principal;

const SERVICE_SOURCE: &str = "iaop-agent-service";
const IDEMPOTENCY_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

fn fingerprint(operation: &str, body: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(operation.as_bytes());
    h.update(b"\x00");
    h.update(body.to_string().as_bytes());
    hex::encode(h.finalize())
}

/// Serializable agent view returned by commands and queries.
#[derive(Debug, Clone, Serialize)]
pub struct AgentView {
    pub id: Uuid,
    pub person_id: Uuid,
    pub agent_code: String,
    pub lifecycle_status: String,
    pub version: u64,
    pub created_at: chrono::DateTime<Utc>,
}

impl From<&Agent> for AgentView {
    fn from(a: &Agent) -> Self {
        Self {
            id: a.id().as_uuid(),
            person_id: a.person_id().as_uuid(),
            agent_code: a.agent_code().to_string(),
            lifecycle_status: a.status().as_str().to_string(),
            version: a.version(),
            created_at: a.created_at(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AppointmentView {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub appointment_reference: Option<String>,
    pub company_reference: Option<String>,
    pub appointment_type: String,
    pub effective_from: chrono::NaiveDate,
    pub effective_to: Option<chrono::NaiveDate>,
    pub status: String,
}

impl From<&AgentAppointment> for AppointmentView {
    fn from(a: &AgentAppointment) -> Self {
        Self {
            id: a.id().as_uuid(),
            agent_id: a.agent_id().as_uuid(),
            appointment_reference: a.appointment_reference().map(str::to_string),
            company_reference: a.company_reference().map(str::to_string),
            appointment_type: a.appointment_type().to_string(),
            effective_from: a.effective_from(),
            effective_to: a.effective_to(),
            status: a.status().as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentityLinkView {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub identity_provider: String,
    pub issuer: String,
    pub subject: String,
    pub status: String,
    pub linked_at: chrono::DateTime<Utc>,
    pub unlinked_at: Option<chrono::DateTime<Utc>>,
}

impl From<&AgentIdentityLink> for IdentityLinkView {
    fn from(l: &AgentIdentityLink) -> Self {
        Self {
            id: l.id().as_uuid(),
            agent_id: l.agent_id().as_uuid(),
            identity_provider: l.identity_provider().to_string(),
            issuer: l.issuer().to_string(),
            subject: l.subject().to_string(),
            status: l.status().as_str().to_string(),
            linked_at: l.linked_at(),
            unlinked_at: l.unlinked_at(),
        }
    }
}

/// Maps a domain event to the canonical integration-event envelope.
pub fn to_integration_event(
    ev: &iaop_agent_service_domain::AgentEvent,
    agent_version: u64,
    agent_id: AgentId,
    correlation_id: Option<Uuid>,
) -> IntegrationEvent {
    let payload = match ev {
        iaop_agent_service_domain::AgentEvent::Created {
            person_id,
            agent_code,
            ..
        } => json!({
            "agentId": agent_id.as_uuid(),
            "personId": person_id.as_uuid(),
            "agentCode": agent_code.as_str(),
        }),
        iaop_agent_service_domain::AgentEvent::ActivationRequested { appointment_id, .. } => {
            json!({
                "agentId": agent_id.as_uuid(),
                "appointmentId": appointment_id.as_uuid(),
            })
        }
        iaop_agent_service_domain::AgentEvent::Activated {
            appointment_id,
            decision_ref,
            ..
        } => json!({
            "agentId": agent_id.as_uuid(),
            "appointmentId": appointment_id.as_uuid(),
            "status": "ACTIVE",
            "decisionRef": decision_ref,
        }),
        iaop_agent_service_domain::AgentEvent::Suspended { reason_code, .. } => json!({
            "agentId": agent_id.as_uuid(),
            "status": "SUSPENDED",
            "reasonCode": reason_code,
        }),
        iaop_agent_service_domain::AgentEvent::Reinstated {
            appointment_id,
            decision_ref,
            ..
        } => json!({
            "agentId": agent_id.as_uuid(),
            "appointmentId": appointment_id.as_uuid(),
            "status": "ACTIVE",
            "decisionRef": decision_ref,
        }),
        iaop_agent_service_domain::AgentEvent::Terminated { reason_code, .. } => json!({
            "agentId": agent_id.as_uuid(),
            "status": "TERMINATED",
            "reasonCode": reason_code,
        }),
        iaop_agent_service_domain::AgentEvent::IdentityLinked {
            link_id, issuer, ..
        } => json!({
            "agentId": agent_id.as_uuid(),
            "linkId": link_id.as_uuid(),
            "issuer": issuer,
        }),
        iaop_agent_service_domain::AgentEvent::IdentityUnlinked { link_id, .. } => json!({
            "agentId": agent_id.as_uuid(),
            "linkId": link_id.as_uuid(),
        }),
    };
    IntegrationEvent {
        event_id: Uuid::now_v7(),
        event_type: ev.event_type().to_string(),
        event_version: 1,
        source: SERVICE_SOURCE.to_string(),
        aggregate_id: agent_id.as_uuid(),
        aggregate_version: Some(agent_version),
        occurred_at: Utc::now(),
        correlation_id,
        causation_id: None,
        payload,
    }
}

/// Persists lifecycle history, outbox events, and the audit record for
/// the transitions/events accumulated during this command.
#[allow(clippy::too_many_arguments)]
async fn persist_transition_effects(
    tx: &mut Transaction<'_, Postgres>,
    store: &impl AgentStore,
    agent: &mut Agent,
    actor: &str,
    action: &str,
    reason: Option<String>,
    correlation_id: Option<Uuid>,
    command_id: CommandId,
) -> Result<(), AppError> {
    // `from_status` of the first new transition is the pre-command state.
    let before_status = agent
        .history()
        .first()
        .and_then(|t| t.from_status)
        .unwrap_or(agent.status());
    let before = json!({ "status": before_status.as_str() });
    let after = json!({ "status": agent.status().as_str(), "version": agent.version() });

    // `Agent::history` contains only transitions since rehydration —
    // exactly the ones produced by this command.
    for t in agent.history() {
        store.insert_lifecycle_history(tx, t, actor).await?;
    }

    let events: Vec<_> = agent.drain_events();
    for ev in &events {
        let ie = to_integration_event(ev, agent.version(), agent.id(), correlation_id);
        store.insert_outbox(tx, &ie).await?;
    }

    store
        .insert_audit(
            tx,
            &AuditRecord {
                id: Uuid::now_v7(),
                agent_id: agent.id(),
                actor: Some(actor.to_string()),
                action: action.to_string(),
                before_state: Some(before),
                after_state: Some(after),
                reason,
                source: SERVICE_SOURCE.to_string(),
                occurred_at: now(),
                correlation_id,
                command_id: Some(command_id.as_uuid()),
            },
        )
        .await?;
    Ok(())
}

async fn load_agent_tx(
    tx: &mut Transaction<'_, Postgres>,
    store: &impl AgentStore,
    id: AgentId,
) -> Result<Agent, AppError> {
    store
        .load_agent_for_update(tx, id)
        .await?
        .ok_or(AppError::AgentNotFound)
}

fn check_expected_version(agent: &Agent, expected: Option<u64>) -> Result<(), AppError> {
    if let Some(exp) = expected {
        if exp != agent.version() {
            return Err(AppError::PreconditionFailed(format!(
                "expected version {exp}, current {}",
                agent.version()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command inputs
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CreateAgentInput {
    pub person_id: Uuid,
    pub agent_code: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug)]
pub struct LifecycleInput {
    pub agent_id: Uuid,
    pub appointment_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

// ---------------------------------------------------------------------------
// create_agent
// ---------------------------------------------------------------------------

pub async fn create_agent(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    principal: &Principal,
    input: CreateAgentInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let operation = "create_agent";
    let body = json!({ "personId": input.person_id, "agentCode": input.agent_code });
    let fp = fingerprint(operation, &body);
    let mut tx = pool.begin().await.map_err(AppError::infra)?;

    let record_id = match idem
        .reserve(
            &mut tx,
            &principal.idempotency_scope(),
            operation,
            &input.idempotency_key,
            &fp,
            IDEMPOTENCY_TTL,
        )
        .await?
    {
        IdempotencyReserve::Replay { status, body } => return Ok((status, body)),
        IdempotencyReserve::Conflict => return Err(AppError::IdempotencyConflict),
        IdempotencyReserve::Acquired { record_id } => record_id,
    };

    let person_id = PersonId::from_uuid(input.person_id);
    let code = input
        .agent_code
        .as_deref()
        .map(AgentCode::new)
        .transpose()
        .map_err(AppError::Domain)?;
    let actor = principal.actor();
    let (agent, created_event) =
        Agent::create(person_id, code, &actor, now()).map_err(AppError::Domain)?;

    // Initial DRAFT history row (from_status NULL).
    let creation = LifecycleTransition {
        agent_id: agent.id(),
        from_status: None,
        to_status: AgentStatus::Draft,
        reason_code: None,
        reason_text: None,
        effective_at: now(),
        performed_by: Some(actor.subject.clone()),
        correlation_id,
        command_id: CommandId::new(),
        aggregate_version: 0,
    };

    store.insert_agent(&mut tx, &agent).await?;
    store
        .insert_lifecycle_history(&mut tx, &creation, &actor.subject)
        .await?;

    let ie = to_integration_event(&created_event, agent.version(), agent.id(), correlation_id);
    store.insert_outbox(&mut tx, &ie).await?;

    let view = AgentView::from(&agent);
    store
        .insert_audit(
            &mut tx,
            &AuditRecord {
                id: Uuid::now_v7(),
                agent_id: agent.id(),
                actor: Some(actor.subject.clone()),
                action: "agent.create".into(),
                before_state: None,
                after_state: Some(json!({ "status": "DRAFT", "version": 0 })),
                reason: None,
                source: SERVICE_SOURCE.into(),
                occurred_at: now(),
                correlation_id,
                command_id: None,
            },
        )
        .await?;

    let resp = serde_json::to_value(&view).map_err(|e| AppError::infra_msg(e.to_string()))?;
    idem.complete(&mut tx, record_id, 201, resp.clone()).await?;
    tx.commit().await.map_err(AppError::infra)?;
    Ok((201, resp))
}

// ---------------------------------------------------------------------------
// Lifecycle commands
// ---------------------------------------------------------------------------

/// Resolves activation authorization through the trusted port and turns
/// the raw decision into domain evidence (scope + expiry verified).
async fn resolve_authorization(
    authz: &impl ActivationAuthorizationPort,
    agent_id: AgentId,
    appointment_id: AppointmentId,
    operation: ActivationOperation,
) -> Result<ActivationAuthorization, AppError> {
    let decision: ActivationDecision = authz.authorize(agent_id, appointment_id, operation).await?;
    verify_decision(decision, agent_id, appointment_id, operation, now()).map_err(AppError::Domain)
}

/// Verifies the appointment exists, belongs to the agent, and is
/// currently valid before activation-type operations proceed.
async fn require_valid_appointment(
    pool: &PgPool,
    store: &impl AgentStore,
    agent_id: AgentId,
    appointment_id: AppointmentId,
) -> Result<(), AppError> {
    let appt = store
        .find_appointment(pool, appointment_id)
        .await?
        .ok_or_else(|| AppError::Domain(AgentError::AppointmentNotValid("not found".into())))?;
    if appt.agent_id() != agent_id {
        return Err(AppError::Domain(AgentError::AppointmentNotValid(
            "belongs to a different agent".into(),
        )));
    }
    let today = Utc::now().date_naive();
    if !appt.is_currently_valid(today) {
        return Err(AppError::Domain(AgentError::AppointmentNotValid(
            "appointment is not active or not currently effective".into(),
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_command(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    operation: &'static str,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
    needs_authorization: Option<ActivationOperation>,
    domain_op: impl for<'b> FnOnce(
        &'b mut Agent,
        Option<&'b ActivationAuthorization>,
        &'b iaop_agent_service_domain::Actor,
        Option<Uuid>,
        CommandId,
        chrono::DateTime<Utc>,
    ) -> Result<(), AgentError>,
) -> Result<(i32, serde_json::Value), AppError> {
    let body = json!({
        "agentId": input.agent_id,
        "appointmentId": input.appointment_id,
        "reasonCode": input.reason_code,
        "reasonText": input.reason_text,
        "expectedVersion": input.expected_version,
    });
    let fp = fingerprint(operation, &body);
    let mut tx = pool.begin().await.map_err(AppError::infra)?;

    let record_id = match idem
        .reserve(
            &mut tx,
            &principal.idempotency_scope(),
            operation,
            &input.idempotency_key,
            &fp,
            IDEMPOTENCY_TTL,
        )
        .await?
    {
        IdempotencyReserve::Replay { status, body } => return Ok((status, body)),
        IdempotencyReserve::Conflict => return Err(AppError::IdempotencyConflict),
        IdempotencyReserve::Acquired { record_id } => record_id,
    };

    let agent_id = AgentId::from_uuid(input.agent_id);
    let mut agent = load_agent_tx(&mut tx, store, agent_id).await?;
    check_expected_version(&agent, input.expected_version)?;

    // Resolve + verify authorization evidence for activate/reinstate.
    let authorization = match needs_authorization {
        Some(op_kind) => {
            let appt_uuid = input.appointment_id.ok_or_else(|| {
                AppError::Domain(AgentError::validation("appointmentId required"))
            })?;
            let appt_id = AppointmentId::from_uuid(appt_uuid);
            require_valid_appointment(pool, store, agent_id, appt_id).await?;
            Some(resolve_authorization(authz, agent_id, appt_id, op_kind).await?)
        }
        None => None,
    };

    let actor = principal.actor();
    let command_id = CommandId::new();
    let t = now();

    domain_op(
        &mut agent,
        authorization.as_ref(),
        &actor,
        correlation_id,
        command_id,
        t,
    )
    .map_err(AppError::Domain)?;

    if !store.update_agent(&mut tx, &agent, &actor.subject).await? {
        return Err(AppError::PreconditionFailed(
            "concurrent version conflict".into(),
        ));
    }

    persist_transition_effects(
        &mut tx,
        store,
        &mut agent,
        &actor.subject,
        operation,
        input.reason_code.clone(),
        correlation_id,
        command_id,
    )
    .await?;

    let view = AgentView::from(&agent);
    let resp = serde_json::to_value(&view).map_err(|e| AppError::infra_msg(e.to_string()))?;
    idem.complete(&mut tx, record_id, 200, resp.clone()).await?;
    tx.commit().await.map_err(AppError::infra)?;
    Ok((200, resp))
}

pub async fn submit_for_activation(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let appt = input
        .appointment_id
        .ok_or_else(|| AppError::Domain(AgentError::validation("appointmentId required")))?;
    run_agent_command(
        pool,
        store,
        idem,
        authz,
        principal,
        "submit_for_activation",
        input,
        correlation_id,
        None,
        |agent, _, actor, corr, cmd, t| {
            agent.submit_for_activation(AppointmentId::from_uuid(appt), actor, corr, cmd, t)
        },
    )
    .await
}

pub async fn activate_agent(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let appt = input
        .appointment_id
        .ok_or_else(|| AppError::Domain(AgentError::validation("appointmentId required")))?;
    run_agent_command(
        pool,
        store,
        idem,
        authz,
        principal,
        "activate_agent",
        input,
        correlation_id,
        Some(ActivationOperation::Activate),
        move |agent, auth, actor, corr, cmd, t| {
            agent.activate(
                AppointmentId::from_uuid(appt),
                auth.expect("authorization resolved for activate"),
                actor,
                corr,
                cmd,
                t,
            )
        },
    )
    .await
}

pub async fn suspend_agent(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let reason = SuspensionReason {
        code: input.reason_code.clone().unwrap_or_default(),
        text: input.reason_text.clone().unwrap_or_default(),
    };
    run_agent_command(
        pool,
        store,
        idem,
        authz,
        principal,
        "suspend_agent",
        input,
        correlation_id,
        None,
        move |agent, _, actor, corr, cmd, t| agent.suspend(reason, actor, corr, cmd, t),
    )
    .await
}

pub async fn reinstate_agent(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let appt = input
        .appointment_id
        .ok_or_else(|| AppError::Domain(AgentError::validation("appointmentId required")))?;
    run_agent_command(
        pool,
        store,
        idem,
        authz,
        principal,
        "reinstate_agent",
        input,
        correlation_id,
        Some(ActivationOperation::Reinstate),
        move |agent, auth, actor, corr, cmd, t| {
            agent.reinstate(
                AppointmentId::from_uuid(appt),
                auth.expect("authorization resolved for reinstate"),
                actor,
                corr,
                cmd,
                t,
            )
        },
    )
    .await
}

pub async fn terminate_agent(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    authz: &impl ActivationAuthorizationPort,
    principal: &Principal,
    input: LifecycleInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    // authorized_by is derived from the authenticated principal — callers
    // cannot assert a different authorizing actor.
    let authorized_by = principal.actor().subject.clone();
    let reason = TerminationReason {
        code: input.reason_code.clone().unwrap_or_default(),
        text: input.reason_text.clone().unwrap_or_default(),
        authorized_by,
    };
    run_agent_command(
        pool,
        store,
        idem,
        authz,
        principal,
        "terminate_agent",
        input,
        correlation_id,
        None,
        move |agent, _, actor, corr, cmd, t| agent.terminate(reason, actor, corr, cmd, t),
    )
    .await
}

// ---------------------------------------------------------------------------
// Identity link commands
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct LinkIdentityInput {
    pub agent_id: Uuid,
    pub identity_provider: String,
    pub issuer: String,
    pub subject: String,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

#[derive(Debug)]
pub struct UnlinkIdentityInput {
    pub agent_id: Uuid,
    pub link_id: Uuid,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

pub async fn link_identity(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    principal: &Principal,
    input: LinkIdentityInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let operation = "link_identity";
    let body = json!({
        "agentId": input.agent_id,
        "identityProvider": input.identity_provider,
        "issuer": input.issuer,
        "subject": input.subject,
        "expectedVersion": input.expected_version,
    });
    let fp = fingerprint(operation, &body);
    let mut tx = pool.begin().await.map_err(AppError::infra)?;

    let record_id = match idem
        .reserve(
            &mut tx,
            &principal.idempotency_scope(),
            operation,
            &input.idempotency_key,
            &fp,
            IDEMPOTENCY_TTL,
        )
        .await?
    {
        IdempotencyReserve::Replay { status, body } => return Ok((status, body)),
        IdempotencyReserve::Conflict => return Err(AppError::IdempotencyConflict),
        IdempotencyReserve::Acquired { record_id } => record_id,
    };

    let agent_id = AgentId::from_uuid(input.agent_id);
    let agent = load_agent_tx(&mut tx, store, agent_id).await?;
    check_expected_version(&agent, input.expected_version)?;

    // Prevent linking an identity already actively bound elsewhere.
    if let Some(existing) = store
        .find_identity_link_by_subject(pool, &input.issuer, &input.subject)
        .await?
    {
        if existing.agent_id() != agent_id {
            return Err(AppError::Domain(AgentError::IdentityLinkConflict(
                "identity subject already linked to another agent".into(),
            )));
        }
    }

    let link = AgentIdentityLink::link(
        agent_id,
        input.identity_provider,
        input.issuer.clone(),
        input.subject,
        now(),
    )
    .map_err(AppError::Domain)?;

    store.insert_identity_link(&mut tx, &link).await?;

    // Outbox event + audit (no lifecycle transition).
    let ev = iaop_agent_service_domain::AgentEvent::IdentityLinked {
        agent_id,
        link_id: link.id(),
        issuer: input.issuer.clone(),
    };
    let ie = to_integration_event(&ev, agent.version(), agent_id, correlation_id);
    store.insert_outbox(&mut tx, &ie).await?;

    let actor = principal.actor();
    store
        .insert_audit(
            &mut tx,
            &AuditRecord {
                id: Uuid::now_v7(),
                agent_id,
                actor: Some(actor.subject.clone()),
                action: "agent.identity.link".into(),
                before_state: None,
                after_state: Some(json!({ "linkId": link.id().as_uuid() })),
                reason: None,
                source: SERVICE_SOURCE.into(),
                occurred_at: now(),
                correlation_id,
                command_id: None,
            },
        )
        .await?;

    let view = IdentityLinkView::from(&link);
    let resp = serde_json::to_value(&view).map_err(|e| AppError::infra_msg(e.to_string()))?;
    idem.complete(&mut tx, record_id, 201, resp.clone()).await?;
    tx.commit().await.map_err(AppError::infra)?;
    Ok((201, resp))
}

pub async fn unlink_identity(
    pool: &PgPool,
    store: &impl AgentStore,
    idem: &impl IdempotencyStore,
    principal: &Principal,
    input: UnlinkIdentityInput,
    correlation_id: Option<Uuid>,
) -> Result<(i32, serde_json::Value), AppError> {
    let operation = "unlink_identity";
    let body = json!({
        "agentId": input.agent_id,
        "linkId": input.link_id,
        "expectedVersion": input.expected_version,
    });
    let fp = fingerprint(operation, &body);
    let mut tx = pool.begin().await.map_err(AppError::infra)?;

    let record_id = match idem
        .reserve(
            &mut tx,
            &principal.idempotency_scope(),
            operation,
            &input.idempotency_key,
            &fp,
            IDEMPOTENCY_TTL,
        )
        .await?
    {
        IdempotencyReserve::Replay { status, body } => return Ok((status, body)),
        IdempotencyReserve::Conflict => return Err(AppError::IdempotencyConflict),
        IdempotencyReserve::Acquired { record_id } => record_id,
    };

    let agent_id = AgentId::from_uuid(input.agent_id);
    let link_id = IdentityLinkId::from_uuid(input.link_id);
    let agent = load_agent_tx(&mut tx, store, agent_id).await?;
    check_expected_version(&agent, input.expected_version)?;

    let mut link = store
        .find_identity_link(pool, link_id)
        .await?
        .ok_or(AppError::AgentNotFound)?;
    if link.agent_id() != agent_id {
        return Err(AppError::AgentNotFound);
    }
    link.unlink(now()).map_err(AppError::Domain)?;
    store.update_identity_link(&mut tx, &link).await?;

    let ev = iaop_agent_service_domain::AgentEvent::IdentityUnlinked {
        agent_id,
        link_id: link.id(),
    };
    let ie = to_integration_event(&ev, agent.version(), agent_id, correlation_id);
    store.insert_outbox(&mut tx, &ie).await?;

    let actor = principal.actor();
    store
        .insert_audit(
            &mut tx,
            &AuditRecord {
                id: Uuid::now_v7(),
                agent_id,
                actor: Some(actor.subject.clone()),
                action: "agent.identity.unlink".into(),
                before_state: Some(json!({ "linkStatus": "ACTIVE" })),
                after_state: Some(json!({ "linkStatus": "UNLINKED" })),
                reason: None,
                source: SERVICE_SOURCE.into(),
                occurred_at: now(),
                correlation_id,
                command_id: None,
            },
        )
        .await?;

    let view = IdentityLinkView::from(&link);
    let resp = serde_json::to_value(&view).map_err(|e| AppError::infra_msg(e.to_string()))?;
    idem.complete(&mut tx, record_id, 200, resp.clone()).await?;
    tx.commit().await.map_err(AppError::infra)?;
    Ok((200, resp))
}
