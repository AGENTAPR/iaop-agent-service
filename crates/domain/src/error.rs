//! Agent business error type.
//!
//! Typed domain errors carry business meaning only — never HTTP status
//! codes, SQL errors, or stack traces. The API layer maps these to
//! RFC 9457 Problem Details.

use thiserror::Error;

use crate::ids::AgentId;
use crate::status::AgentStatus;

/// Errors raised by Agent aggregate behavior and domain services.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent {0} not found")]
    NotFound(AgentId),

    #[error("invalid lifecycle transition: {from} -> {to}")]
    InvalidStateTransition { from: AgentStatus, to: AgentStatus },

    #[error("agent {0} is already active")]
    AgentAlreadyActive(AgentId),

    #[error("agent {0} is terminated and cannot transition")]
    AgentTerminated(AgentId),

    #[error("invalid agent code '{0}': {1}")]
    InvalidAgentCode(String, &'static str),

    #[error("invalid effective date: {0}")]
    InvalidEffectiveDate(String),

    #[error("suspension requires an approved reason")]
    MissingSuspensionReason,

    #[error("termination requires a reason and authorized actor")]
    MissingTerminationReason,

    #[error("activation not authorized: {0}")]
    ActivationNotAuthorized(String),

    #[error("appointment {0} is not valid for this operation")]
    AppointmentNotValid(String),

    #[error("identity link conflict: {0}")]
    IdentityLinkConflict(String),

    #[error("idempotency key reuse with different request")]
    IdempotencyConflict,

    #[error("concurrent modification: expected version {expected}, found {found}")]
    VersionConflict { expected: u64, found: u64 },

    #[error("validation failed: {0}")]
    Validation(String),
}

impl AgentError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }
}
