//! Activation authorization: trusted decision verification.
//!
//! The [`ActivationAuthorizationPort`] (application layer) returns a raw
//! [`ActivationDecision`] produced by infrastructure adapters (eligibility
//! service client, or the fail-closed default). This module is the ONLY
//! place a decision becomes domain [`ActivationAuthorization`] evidence —
//! and only after scope (agent, appointment, operation) and expiry are
//! verified. HTTP DTOs can never construct evidence directly.

use chrono::{DateTime, Utc};

use crate::agent::{ActivationAuthorization, ActivationOperation};
use crate::error::AgentError;
use crate::ids::{AgentId, AppointmentId};

/// Outcome of an activation-authorization check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationOutcome {
    /// Explicitly allowed by the policy source.
    Allow,
    /// Explicitly denied.
    Deny,
    /// Source could not decide (timeout, unavailable, ambiguous).
    Indeterminate,
}

/// Raw decision returned by an [`crate`] authorization port adapter.
/// Carries scope + validity so the domain can verify it is bound to the
/// exact agent/appointment/operation being attempted.
#[derive(Debug, Clone)]
pub struct ActivationDecision {
    pub outcome: AuthorizationOutcome,
    pub agent_id: AgentId,
    pub appointment_id: AppointmentId,
    pub operation: ActivationOperation,
    pub decision_ref: String,
    pub policy_ref: String,
    pub valid_until: DateTime<Utc>,
    /// Machine-readable reason for denials/indeterminate results.
    pub reason: Option<String>,
}

/// Converts a trusted [`ActivationDecision`] into domain evidence.
///
/// Fails closed: only `Allow` produces evidence; `Deny`/`Indeterminate`
/// and scope mismatches produce [`AgentError::ActivationNotAuthorized`].
pub fn verify_decision(
    decision: ActivationDecision,
    agent_id: AgentId,
    appointment_id: AppointmentId,
    operation: ActivationOperation,
    now: DateTime<Utc>,
) -> Result<ActivationAuthorization, AgentError> {
    match decision.outcome {
        AuthorizationOutcome::Allow => {}
        AuthorizationOutcome::Deny => {
            return Err(AgentError::ActivationNotAuthorized(format!(
                "denied by policy source: {}",
                decision.reason.unwrap_or_else(|| "no reason".into())
            )));
        }
        AuthorizationOutcome::Indeterminate => {
            return Err(AgentError::ActivationNotAuthorized(
                "authorization indeterminate: policy source unavailable".into(),
            ));
        }
    }
    if decision.agent_id != agent_id
        || decision.appointment_id != appointment_id
        || decision.operation != operation
        || now > decision.valid_until
    {
        return Err(AgentError::ActivationNotAuthorized(
            "authorization scope mismatch or expired".into(),
        ));
    }
    Ok(ActivationAuthorization::issue(
        decision.decision_ref,
        decision.policy_ref,
        decision.valid_until,
        agent_id,
        appointment_id,
        operation,
    ))
}
