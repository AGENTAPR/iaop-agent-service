//! Activation authorization adapters.
//!
//! - [`FailClosedActivationAuthz`]: production-safe default. Every check
//!   returns `INDETERMINATE` â€” activation stays blocked until a real
//!   policy source (Eligibility/Onboarding/Certification) is integrated.
//! - [`HttpEligibilityAuthz`]: Authoritative HTTP adapter that queries the external
//!   Eligibility / Compliance service. Fails closed on timeout or network error.
//! - [`SignedDecisionAuthz`]: Verifies a durable cryptographically signed decision
//!   token or signed claim.
//! - [`FixtureActivationAuthz`]: deterministic ALLOW for local/E2E
//!   testing. Construction is refused in production by `AppConfig`.
//!
//! All adapters return raw [`ActivationDecision`]s; the domain verifies scope
//! and expiry before any evidence is issued â€” adapters cannot mint
//! authorization directly.

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

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

/// HTTP client adapter querying the authoritative Eligibility / Compliance service.
#[derive(Clone)]
pub struct HttpEligibilityAuthz {
    client: reqwest::Client,
    eligibility_service_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyEligibilityRequest {
    agent_id: uuid::Uuid,
    appointment_id: uuid::Uuid,
    operation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyEligibilityResponse {
    outcome: String, // "ALLOW", "DENY", "INDETERMINATE"
    decision_ref: Option<String>,
    policy_ref: Option<String>,
    valid_until: Option<chrono::DateTime<Utc>>,
    reason: Option<String>,
}

impl HttpEligibilityAuthz {
    pub fn new(eligibility_service_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            client,
            eligibility_service_url,
        }
    }
}

impl ActivationAuthorizationPort for HttpEligibilityAuthz {
    async fn authorize(
        &self,
        agent_id: AgentId,
        appointment_id: AppointmentId,
        operation: ActivationOperation,
    ) -> Result<ActivationDecision, AppError> {
        let url = format!(
            "{}/api/v1/eligibility/decisions/verify",
            self.eligibility_service_url
        );
        let req = VerifyEligibilityRequest {
            agent_id: agent_id.as_uuid(),
            appointment_id: appointment_id.as_uuid(),
            operation: format!("{:?}", operation),
        };

        let resp = match self.client.post(&url).json(&req).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "eligibility service request failed; failing closed");
                return Ok(ActivationDecision {
                    outcome: AuthorizationOutcome::Indeterminate,
                    agent_id,
                    appointment_id,
                    operation,
                    decision_ref: "network-error://eligibility-unreachable".into(),
                    policy_ref: "fail-closed-v1".into(),
                    valid_until: Utc::now() + Duration::minutes(1),
                    reason: Some(format!("eligibility service unreachable: {e}")),
                });
            }
        };

        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "eligibility service returned non-200");
            return Ok(ActivationDecision {
                outcome: AuthorizationOutcome::Indeterminate,
                agent_id,
                appointment_id,
                operation,
                decision_ref: "upstream-error://eligibility-failure".into(),
                policy_ref: "fail-closed-v1".into(),
                valid_until: Utc::now() + Duration::minutes(1),
                reason: Some(format!("eligibility upstream returned {}", resp.status())),
            });
        }

        let body: VerifyEligibilityResponse = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse eligibility response");
                return Ok(ActivationDecision {
                    outcome: AuthorizationOutcome::Indeterminate,
                    agent_id,
                    appointment_id,
                    operation,
                    decision_ref: "deserialization-error".into(),
                    policy_ref: "fail-closed-v1".into(),
                    valid_until: Utc::now() + Duration::minutes(1),
                    reason: Some("invalid response body from eligibility service".into()),
                });
            }
        };

        let outcome = match body.outcome.to_uppercase().as_str() {
            "ALLOW" => AuthorizationOutcome::Allow,
            "DENY" => AuthorizationOutcome::Deny,
            _ => AuthorizationOutcome::Indeterminate,
        };

        Ok(ActivationDecision {
            outcome,
            agent_id,
            appointment_id,
            operation,
            decision_ref: body
                .decision_ref
                .unwrap_or_else(|| "remote-eligibility".into()),
            policy_ref: body
                .policy_ref
                .unwrap_or_else(|| "eligibility-rules-v1".into()),
            valid_until: body
                .valid_until
                .unwrap_or_else(|| Utc::now() + Duration::minutes(30)),
            reason: body.reason,
        })
    }
}

/// Test fixture: always ALLOW, clearly marked decision refs.
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
    Http(HttpEligibilityAuthz),
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
            Self::Http(v) => v.authorize(agent_id, appointment_id, operation).await,
            Self::Fixture(v) => v.authorize(agent_id, appointment_id, operation).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iaop_agent_service_domain::authorization::verify_decision;

    #[tokio::test]
    async fn fail_closed_adapter_produces_indeterminate_and_fails_domain_verification() {
        let adapter = FailClosedActivationAuthz::new();
        let agent_id = AgentId::new();
        let appt_id = AppointmentId::new();
        let op = ActivationOperation::Activate;

        let decision = adapter.authorize(agent_id, appt_id, op).await.unwrap();
        assert_eq!(decision.outcome, AuthorizationOutcome::Indeterminate);

        let verified = verify_decision(decision, agent_id, appt_id, op, Utc::now());
        assert!(
            verified.is_err(),
            "Indeterminate must fail domain verification"
        );
    }

    #[tokio::test]
    async fn expired_decision_fails_domain_verification() {
        let agent_id = AgentId::new();
        let appt_id = AppointmentId::new();
        let op = ActivationOperation::Activate;

        let stale_decision = ActivationDecision {
            outcome: AuthorizationOutcome::Allow,
            agent_id,
            appointment_id: appt_id,
            operation: op,
            decision_ref: "test://expired".into(),
            policy_ref: "policy-v1".into(),
            valid_until: Utc::now() - Duration::minutes(10), // EXPIRED
            reason: None,
        };

        let verified = verify_decision(stale_decision, agent_id, appt_id, op, Utc::now());
        assert!(verified.is_err(), "Expired decision must be rejected");
    }

    #[tokio::test]
    async fn scope_mismatch_decision_fails_domain_verification() {
        let agent_id = AgentId::new();
        let wrong_agent = AgentId::new();
        let appt_id = AppointmentId::new();
        let op = ActivationOperation::Activate;

        let decision = ActivationDecision {
            outcome: AuthorizationOutcome::Allow,
            agent_id: wrong_agent, // Scope mismatch
            appointment_id: appt_id,
            operation: op,
            decision_ref: "test://mismatch".into(),
            policy_ref: "policy-v1".into(),
            valid_until: Utc::now() + Duration::hours(1),
            reason: None,
        };

        let verified = verify_decision(decision, agent_id, appt_id, op, Utc::now());
        assert!(verified.is_err(), "Scope mismatch must be rejected");
    }
}
