//! Activation authorization adapters.
//!
//! - [`FailClosedActivationAuthz`]: production-safe default. Every check
//!   returns `INDETERMINATE` — activation stays blocked until a real
//!   policy source (Eligibility/Onboarding/Certification) is integrated.
//! - [`FixtureActivationAuthz`]: deterministic ALLOW for local/E2E
//!   testing. Construction is refused in production by `AppConfig`.
//!
//! Both return raw [`ActivationDecision`]s; the domain verifies scope
//! and expiry before any evidence is issued — adapters cannot mint
//! authorization directly.

use chrono::{Duration, Utc};

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::ActivationAuthorizationPort;
use iaop_agent_service_domain::{
    ActivationDecision, ActivationOperation, AgentId, AppointmentId, AuthorizationOutcome,
};

/// Fail-closed adapter: no external policy source configured.
pub struct FailClosedActivationAuthz;

impl FailClosedActivationAuthz {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FailClosedActivationAuthz {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivationAuthorizationPort for FailClosedActivationAuthz {
    async fn authorize(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Result<ActivationDecision, AppError> {
        Ok(ActivationDecision {
            outcome: AuthorizationOutcome::Indeterminate,
            agent_id,
            appointment_id,
            operation,
            decision_ref: "fail-closed://no-policy-source".into(),
            policy_ref: "none".into(),
            valid_until: Utc::now() + Duration::minutes(5),
            reason: Some("no activation policy source configured".into()),
        })
    }
}

/// Test fixture: always ALLOW, clearly marked decision refs.
///
/// # Safety
/// Must never be constructed when `ENVIRONMENT=production` — enforced
/// by `AppConfig::from_env` (startup refuses `ACTIVATION_MODE=fixture`
/// in production).
pub struct FixtureActivationAuthz;

impl FixtureActivationAuthz {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FixtureActivationAuthz {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivationAuthorizationPort for FixtureActivationAuthz {
    async fn authorize(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Result<ActivationDecision, AppError> {
        Ok(ActivationDecision {
            outcome: AuthorizationOutcome::Allow,
            agent_id,
            appointment_id,
            operation,
            decision_ref: "fixture://local-test-allow".into(),
            policy_ref: "fixture-policy-v1".into(),
            valid_until: Utc::now() + Duration::hours(1),
            reason: None,
        })
    }
}

/// Concrete authorization adapter selected by configuration.
pub enum ActivationAuthzImpl {
    FailClosed(FailClosedActivationAuthz),
    Fixture(FixtureActivationAuthz),
}

impl ActivationAuthorizationPort for ActivationAuthzImpl {
    async fn authorize(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Result<ActivationDecision, AppError> {
        match self {
            Self::FailClosed(v) => v.authorize(agent_id, appointment_id, operation).await,
            Self::Fixture(v) => v.authorize(agent_id, appointment_id, operation).await,
        }
    }
}
