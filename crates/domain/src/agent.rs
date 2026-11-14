//! Agent aggregate — the consistency boundary for agent identity and
//! administrative lifecycle.
//!
//! Invariants enforced here (never by callers):
//! - identity (`id`, `person_id`) is immutable after creation
//! - `agent_code` is immutable once issued
//! - lifecycle transitions follow [`AgentStatus::can_transition_to`]
//! - activation/reinstatement require scoped [`ActivationAuthorization`]
//! - every transition increments `version` and emits a lifecycle record
//! - `TERMINATED` is final
//!
//! Infrastructure concerns (persistence, clocks beyond the injected
//! `effective_at`, auth) never appear in this module.

use chrono::{DateTime, Utc};

use crate::error::AgentError;
use crate::ids::{AgentId, AppointmentId, CommandId, IdentityLinkId, PersonId};
use crate::status::AgentStatus;

/// Business actor performing a command (from the authenticated principal).
#[derive(Debug, Clone)]
pub struct Actor {
    /// Stable actor identifier (Keycloak `sub` or service account id).
    pub subject: String,
    /// Optional UUID actor reference for audit columns.
    pub actor_id: Option<uuid::Uuid>,
}

/// Agent business code — immutable once issued.
///
/// Phase 1 assumption: codes are **globally unique** within the agent
/// registry namespace (the `agents.agent_code` unique constraint). If a
/// future insurer/company scoping requirement emerges, uniqueness moves
/// to a composite `(scope, agent_code)` — documented in DATABASE_DESIGN.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentCode(String);

impl AgentCode {
    pub const MAX_LEN: usize = 40;

    /// Normalizes and validates a candidate code.
    ///
    /// Rules: 3..=40 chars; uppercase ASCII letters, digits, hyphen;
    /// must start with a letter; no leading/trailing hyphen.
    pub fn new(raw: &str) -> Result<Self, AgentError> {
        let code = raw.trim().to_uppercase();
        let valid = (3..=Self::MAX_LEN).contains(&code.len())
            && code.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && !code.starts_with('-')
            && !code.ends_with('-')
            && code
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(code))
        } else {
            Err(AgentError::InvalidAgentCode(
                raw.to_string(),
                "must be 3-40 chars: uppercase A-Z, 0-9, hyphen; start with a letter",
            ))
        }
    }

    /// Generates a registry-issued code `AGT-XXXXXXXXXX`.
    pub fn generate() -> Self {
        let raw = uuid::Uuid::now_v7().simple().to_string();
        // Safe: hex chars only, fixed length.
        Self(format!("AGT-{}", &raw[..10]).to_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reason attached to a suspension. An approved reason is mandatory.
#[derive(Debug, Clone)]
pub struct SuspensionReason {
    pub code: String,
    pub text: String,
}

/// Reason attached to a termination plus the authorizing actor.
#[derive(Debug, Clone)]
pub struct TerminationReason {
    pub code: String,
    pub text: String,
    /// Actor authorized to terminate (e.g. lifecycle officer).
    pub authorized_by: String,
}

/// Scoped evidence that activation was authorized by a trusted source.
///
/// Instances are produced ONLY by [`crate::authorization`] verification
/// of an [`crate::authorization::ActivationDecision`] — there is no
/// public constructor that accepts caller-supplied booleans.
#[derive(Debug, Clone)]
pub struct ActivationAuthorization {
    pub decision_ref: String,
    pub policy_ref: String,
    pub valid_until: DateTime<Utc>,
    agent_id: AgentId,
    appointment_id: AppointmentId,
    operation: ActivationOperation,
}

/// Which lifecycle operation the authorization covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationOperation {
    Activate,
    Reinstate,
}

impl ActivationAuthorization {
    /// Trusted constructor — called only by domain-internal verification
    /// after a decision has been scope-checked. `pub` for the
    /// `authorization` module; not reachable from API DTOs.
    #[doc(hidden)]
    pub fn issue(
        decision_ref: String,
        policy_ref: String,
        valid_until: DateTime<Utc>,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Self {
        Self {
            decision_ref,
            policy_ref,
            valid_until,
            agent_id,
            appointment_id,
            operation,
        }
    }

    /// Verifies the evidence is scoped to this agent/appointment/operation
    /// and still valid at `now`.
    pub fn verify_for(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        if self.agent_id != agent_id
            || self.appointment_id != appointment_id
            || self.operation != operation
        {
            return Err(AgentError::ActivationNotAuthorized(
                "authorization scope mismatch".into(),
            ));
        }
        if now > self.valid_until {
            return Err(AgentError::ActivationNotAuthorized(
                "authorization expired".into(),
            ));
        }
        Ok(())
    }
}

/// Immutable record of one lifecycle transition (audit/history row).
#[derive(Debug, Clone)]
pub struct LifecycleTransition {
    pub agent_id: AgentId,
    pub from_status: Option<AgentStatus>,
    pub to_status: AgentStatus,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub effective_at: DateTime<Utc>,
    pub performed_by: Option<String>,
    pub correlation_id: Option<uuid::Uuid>,
    pub command_id: CommandId,
    pub aggregate_version: u64,
}

/// A domain event raised inside the aggregate. Translated to an
/// integration event at the application boundary.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Created {
        agent_id: AgentId,
        person_id: PersonId,
        agent_code: AgentCode,
    },
    ActivationRequested {
        agent_id: AgentId,
        appointment_id: AppointmentId,
    },
    Activated {
        agent_id: AgentId,
        appointment_id: AppointmentId,
        decision_ref: String,
    },
    Suspended {
        agent_id: AgentId,
        reason_code: String,
    },
    Reinstated {
        agent_id: AgentId,
        appointment_id: AppointmentId,
        decision_ref: String,
    },
    Terminated {
        agent_id: AgentId,
        reason_code: String,
    },
    IdentityLinked {
        agent_id: AgentId,
        link_id: IdentityLinkId,
        issuer: String,
    },
    IdentityUnlinked {
        agent_id: AgentId,
        link_id: IdentityLinkId,
    },
}

impl AgentEvent {
    /// Integration event type per `iaop-event-contracts` naming.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "agent.created",
            Self::ActivationRequested { .. } => "agent.activation-requested",
            Self::Activated { .. } => "agent.activated",
            Self::Suspended { .. } => "agent.suspended",
            Self::Reinstated { .. } => "agent.reinstated",
            Self::Terminated { .. } => "agent.terminated",
            Self::IdentityLinked { .. } => "agent.identity-linked",
            Self::IdentityUnlinked { .. } => "agent.identity-unlinked",
        }
    }
}

/// The Agent aggregate root.
///
/// All fields are private; mutation happens only through the behavior
/// methods below, each of which validates preconditions, enforces the
/// transition table, bumps `version`, and records a [`LifecycleTransition`].
#[derive(Debug)]
pub struct Agent {
    id: AgentId,
    person_id: PersonId,
    agent_code: AgentCode,
    status: AgentStatus,
    version: u64,
    created_at: DateTime<Utc>,
    created_by: Option<String>,
    pending_events: Vec<AgentEvent>,
    history: Vec<LifecycleTransition>,
}

impl Agent {
    /// Creates a new agent in `DRAFT` status.
    ///
    /// `agent_code` may be supplied (e.g. from an upstream registry
    /// decision) or generated when `None`.
    pub fn create(
        person_id: PersonId,
        agent_code: Option<AgentCode>,
        actor: &Actor,
        now: DateTime<Utc>,
    ) -> Result<(Self, AgentEvent), AgentError> {
        let id = AgentId::new();
        let code = agent_code.unwrap_or_else(AgentCode::generate);
        let agent = Self {
            id,
            person_id,
            agent_code: code.clone(),
            status: AgentStatus::Draft,
            version: 0,
            created_at: now,
            created_by: Some(actor.subject.clone()),
            pending_events: Vec::new(),
            history: Vec::new(),
        };
        let event = AgentEvent::Created {
            agent_id: id,
            person_id,
            agent_code: code,
        };
        Ok((agent, event))
    }

    /// Rehydrates an aggregate from persistence. Does not emit events.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: AgentId,
        person_id: PersonId,
        agent_code: AgentCode,
        status: AgentStatus,
        version: u64,
        created_at: DateTime<Utc>,
        created_by: Option<String>,
    ) -> Self {
        Self {
            id,
            person_id,
            agent_code,
            status,
            version,
            created_at,
            created_by,
            pending_events: Vec::new(),
            history: Vec::new(),
        }
    }

    // -- accessors ----------------------------------------------------

    pub fn id(&self) -> AgentId {
        self.id
    }
    pub fn person_id(&self) -> PersonId {
        self.person_id
    }
    pub fn agent_code(&self) -> &AgentCode {
        &self.agent_code
    }
    pub fn status(&self) -> AgentStatus {
        self.status
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    pub fn created_by(&self) -> Option<&str> {
        self.created_by.as_deref()
    }

    /// Domain events raised since last drain (for outbox translation).
    pub fn drain_events(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Lifecycle audit records accumulated during this instance's life
    /// (rehydrated instances start empty; the repository loads history
    /// separately for queries).
    pub fn history(&self) -> &[LifecycleTransition] {
        &self.history
    }

    // -- behavior -----------------------------------------------------

    /// DRAFT -> PENDING_ACTIVATION.
    pub fn submit_for_activation(
        &mut self,
        appointment_id: AppointmentId,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        self.transition(
            AgentStatus::PendingActivation,
            None,
            None,
            actor,
            correlation_id,
            command_id,
            now,
        )?;
        let event = AgentEvent::ActivationRequested {
            agent_id: self.id,
            appointment_id,
        };
        self.pending_events.push(event);
        Ok(())
    }

    /// PENDING_ACTIVATION -> ACTIVE. Requires scoped authorization.
    pub fn activate(
        &mut self,
        appointment_id: AppointmentId,
        authorization: &ActivationAuthorization,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        authorization.verify_for(self.id, appointment_id, ActivationOperation::Activate, now)?;
        if self.status == AgentStatus::Active {
            return Err(AgentError::AgentAlreadyActive(self.id));
        }
        self.transition(
            AgentStatus::Active,
            None,
            None,
            actor,
            correlation_id,
            command_id,
            now,
        )?;
        self.pending_events.push(AgentEvent::Activated {
            agent_id: self.id,
            appointment_id,
            decision_ref: authorization.decision_ref.clone(),
        });
        Ok(())
    }

    /// ACTIVE -> SUSPENDED. Requires an approved reason.
    pub fn suspend(
        &mut self,
        reason: SuspensionReason,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        if reason.code.trim().is_empty() || reason.text.trim().is_empty() {
            return Err(AgentError::MissingSuspensionReason);
        }
        self.transition(
            AgentStatus::Suspended,
            Some(reason.code.clone()),
            Some(reason.text.clone()),
            actor,
            correlation_id,
            command_id,
            now,
        )?;
        self.pending_events.push(AgentEvent::Suspended {
            agent_id: self.id,
            reason_code: reason.code,
        });
        Ok(())
    }

    /// SUSPENDED -> ACTIVE. Controlled reinstatement — requires the same
    /// scoped authorization evidence as a fresh activation.
    pub fn reinstate(
        &mut self,
        appointment_id: AppointmentId,
        authorization: &ActivationAuthorization,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        authorization.verify_for(self.id, appointment_id, ActivationOperation::Reinstate, now)?;
        self.transition(
            AgentStatus::Active,
            None,
            None,
            actor,
            correlation_id,
            command_id,
            now,
        )?;
        self.pending_events.push(AgentEvent::Reinstated {
            agent_id: self.id,
            appointment_id,
            decision_ref: authorization.decision_ref.clone(),
        });
        Ok(())
    }

    /// Any non-terminal status -> TERMINATED. Reason + authorized actor
    /// are mandatory. There is no automatic reinstatement.
    pub fn terminate(
        &mut self,
        reason: TerminationReason,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        if reason.code.trim().is_empty()
            || reason.text.trim().is_empty()
            || reason.authorized_by.trim().is_empty()
        {
            return Err(AgentError::MissingTerminationReason);
        }
        if self.status == AgentStatus::Terminated {
            return Err(AgentError::AgentTerminated(self.id));
        }
        self.transition(
            AgentStatus::Terminated,
            Some(reason.code.clone()),
            Some(reason.text.clone()),
            actor,
            correlation_id,
            command_id,
            now,
        )?;
        self.pending_events.push(AgentEvent::Terminated {
            agent_id: self.id,
            reason_code: reason.code,
        });
        Ok(())
    }

    /// DRAFT -> TERMINATED shortcut used when a draft is abandoned before
    /// activation is ever requested. Routed through `terminate` semantics:
    /// callers should prefer [`Agent::terminate`] which covers DRAFT.
    /// This method exists for explicitness in tests/docs.
    pub fn abandon_draft(
        &mut self,
        reason: TerminationReason,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        self.terminate(reason, actor, correlation_id, command_id, now)
    }

    // -- internals ---------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn transition(
        &mut self,
        to: AgentStatus,
        reason_code: Option<String>,
        reason_text: Option<String>,
        actor: &Actor,
        correlation_id: Option<uuid::Uuid>,
        command_id: CommandId,
        now: DateTime<Utc>,
    ) -> Result<(), AgentError> {
        let from = self.status;
        if self.status == AgentStatus::Terminated {
            return Err(AgentError::AgentTerminated(self.id));
        }
        if !from.can_transition_to(to) {
            return Err(AgentError::InvalidStateTransition { from, to });
        }
        if now < self.created_at {
            return Err(AgentError::InvalidEffectiveDate(format!(
                "effective time {now} precedes agent creation {}",
                self.created_at
            )));
        }
        self.status = to;
        self.version += 1;
        self.history.push(LifecycleTransition {
            agent_id: self.id,
            from_status: Some(from),
            to_status: to,
            reason_code,
            reason_text,
            effective_at: now,
            performed_by: Some(actor.subject.clone()),
            correlation_id,
            command_id,
            aggregate_version: self.version,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn actor() -> Actor {
        Actor {
            subject: "ops-user".into(),
            actor_id: None,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap()
    }

    fn cmd() -> CommandId {
        CommandId::new()
    }

    fn auth(
        agent: &Agent,
        appt: AppointmentId,
        op: ActivationOperation,
    ) -> ActivationAuthorization {
        ActivationAuthorization::issue(
            "dec-1".into(),
            "policy-v1".into(),
            now() + chrono::Duration::hours(1),
            agent.id(),
            appt,
            op,
        )
    }

    #[test]
    fn create_starts_as_draft_v0() {
        let (agent, ev) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        assert_eq!(agent.status(), AgentStatus::Draft);
        assert_eq!(agent.version(), 0);
        assert!(matches!(ev, AgentEvent::Created { .. }));
        assert!(agent.agent_code().as_str().starts_with("AGT-"));
    }

    #[test]
    fn full_happy_path() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let appt = AppointmentId::new();
        a.submit_for_activation(appt, &actor(), None, cmd(), now())
            .unwrap();
        assert_eq!(a.status(), AgentStatus::PendingActivation);
        assert_eq!(a.version(), 1);
        let auth_act = auth(&a, appt, ActivationOperation::Activate);
        a.activate(appt, &auth_act, &actor(), None, cmd(), now())
            .unwrap();
        assert_eq!(a.status(), AgentStatus::Active);
        assert_eq!(a.version(), 2);
        let rsn = SuspensionReason {
            code: "COMPLIANCE".into(),
            text: "review".into(),
        };
        a.suspend(rsn, &actor(), None, cmd(), now()).unwrap();
        assert_eq!(a.status(), AgentStatus::Suspended);
        let auth2 = auth(&a, appt, ActivationOperation::Reinstate);
        a.reinstate(appt, &auth2, &actor(), None, cmd(), now())
            .unwrap();
        assert_eq!(a.status(), AgentStatus::Active);
        let term = TerminationReason {
            code: "CONTRACT_END".into(),
            text: "contract ended".into(),
            authorized_by: "lifecycle-officer".into(),
        };
        a.terminate(term, &actor(), None, cmd(), now()).unwrap();
        assert_eq!(a.status(), AgentStatus::Terminated);
        assert_eq!(a.history().len(), 5);
        let types: Vec<_> = a
            .drain_events()
            .iter()
            .map(AgentEvent::event_type)
            .collect();
        // Created event was returned separately; drained has 4 transitions? submit+activate+suspend+reinstate+terminate = 5 events
        assert_eq!(types.len(), 5);
    }

    #[test]
    fn cannot_activate_twice() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let appt = AppointmentId::new();
        a.submit_for_activation(appt, &actor(), None, cmd(), now())
            .unwrap();
        let auth1 = auth(&a, appt, ActivationOperation::Activate);
        a.activate(appt, &auth1, &actor(), None, cmd(), now())
            .unwrap();
        let auth2 = auth(&a, appt, ActivationOperation::Activate);
        let err = a
            .activate(appt, &auth2, &actor(), None, cmd(), now())
            .unwrap_err();
        assert!(matches!(err, AgentError::AgentAlreadyActive(_)));
    }

    #[test]
    fn terminated_cannot_activate() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let term = TerminationReason {
            code: "X".into(),
            text: "y".into(),
            authorized_by: "z".into(),
        };
        a.terminate(term, &actor(), None, cmd(), now()).unwrap();
        let appt = AppointmentId::new();
        let auth = auth(&a, appt, ActivationOperation::Activate);
        let err = a
            .activate(appt, &auth, &actor(), None, cmd(), now())
            .unwrap_err();
        assert!(matches!(err, AgentError::AgentTerminated(_)));
    }

    #[test]
    fn invalid_transition_rejected() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let appt = AppointmentId::new();
        let auth = auth(&a, appt, ActivationOperation::Activate);
        // DRAFT -> ACTIVE is illegal
        let err = a
            .activate(appt, &auth, &actor(), None, cmd(), now())
            .unwrap_err();
        assert!(matches!(err, AgentError::InvalidStateTransition { .. }));
    }

    #[test]
    fn suspend_requires_reason() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let appt = AppointmentId::new();
        a.submit_for_activation(appt, &actor(), None, cmd(), now())
            .unwrap();
        let auth = auth(&a, appt, ActivationOperation::Activate);
        a.activate(appt, &auth, &actor(), None, cmd(), now())
            .unwrap();
        let err = a
            .suspend(
                SuspensionReason {
                    code: " ".into(),
                    text: String::new(),
                },
                &actor(),
                None,
                cmd(),
                now(),
            )
            .unwrap_err();
        assert!(matches!(err, AgentError::MissingSuspensionReason));
    }

    #[test]
    fn authorization_scope_is_enforced() {
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let appt = AppointmentId::new();
        a.submit_for_activation(appt, &actor(), None, cmd(), now())
            .unwrap();
        // Authorization issued for a DIFFERENT appointment must fail.
        let wrong = auth(&a, AppointmentId::new(), ActivationOperation::Activate);
        let err = a
            .activate(appt, &wrong, &actor(), None, cmd(), now())
            .unwrap_err();
        assert!(matches!(err, AgentError::ActivationNotAuthorized(_)));
    }

    #[test]
    fn agent_code_validation() {
        assert!(AgentCode::new("agt-1234").is_ok());
        assert_eq!(AgentCode::new("agt-1234").unwrap().as_str(), "AGT-1234");
        assert!(AgentCode::new("1ABC").is_err());
        assert!(AgentCode::new("ab").is_err());
        assert!(AgentCode::new("HAS SPACE").is_err());
        assert!(AgentCode::new(&"X".repeat(41)).is_err());
    }

    #[test]
    fn effective_date_before_creation_rejected() {
        let early = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let (mut a, _) = Agent::create(PersonId::new(), None, &actor(), now()).unwrap();
        let err = a
            .submit_for_activation(AppointmentId::new(), &actor(), None, cmd(), early)
            .unwrap_err();
        assert!(matches!(err, AgentError::InvalidEffectiveDate(_)));
    }
}
