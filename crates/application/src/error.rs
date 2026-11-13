//! Application-level error type.
//!
//! Separates domain errors (business rules) from infrastructure failures
//! so the API layer can map them to distinct problem details without
//! leaking internals.

use thiserror::Error;

use iaop_agent_service_domain::AgentError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Domain(#[from] AgentError),

    #[error("agent not found")]
    AgentNotFound,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

    #[error("idempotency key conflict")]
    IdempotencyConflict,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("infrastructure error")]
    Infrastructure(#[source] anyhow::Error),
}

impl AppError {
    pub fn infra<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Infrastructure(anyhow::Error::new(e))
    }

    pub fn infra_msg(msg: impl Into<String>) -> Self {
        Self::Infrastructure(anyhow::anyhow!(msg.into()))
    }
}
