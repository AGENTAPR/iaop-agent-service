//! RFC 9457 Problem Details error mapping.
//!
//! Never expose stack traces, SQL errors, or internal details to clients.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_domain::AgentError;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub correlation_id: Option<Uuid>,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        title: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            title,
            detail: detail.into(),
            correlation_id: None,
        }
    }

    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            "Validation failed",
            detail,
        )
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Resource not found",
            "The requested resource does not exist",
        )
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        // Detail is logged server-side; clients get a generic message.
        tracing::error!(error = %detail.into(), "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "Internal error",
            "An unexpected error occurred",
        )
    }

    pub fn with_correlation(mut self, id: Option<Uuid>) -> Self {
        self.correlation_id = id;
        self
    }
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Domain(d) => domain_error(d),
            AppError::AgentNotFound => ApiError::not_found(),
            AppError::Conflict(msg) => {
                ApiError::new(StatusCode::CONFLICT, "CONFLICT", "Conflict", msg)
            }
            AppError::PreconditionFailed(msg) => ApiError::new(
                StatusCode::PRECONDITION_FAILED,
                "PRECONDITION_FAILED",
                "Precondition failed",
                msg,
            ),
            AppError::IdempotencyConflict => ApiError::new(
                StatusCode::CONFLICT,
                "IDEMPOTENCY_CONFLICT",
                "Idempotency conflict",
                "Idempotency-Key was already used with a different request",
            ),
            AppError::Forbidden(msg) => {
                ApiError::new(StatusCode::FORBIDDEN, "FORBIDDEN", "Forbidden", msg)
            }
            AppError::Infrastructure(err) => {
                tracing::error!(error = %err, "infrastructure error");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR",
                    "Internal error",
                    "An unexpected error occurred",
                )
            }
        }
    }
}

fn domain_error(e: AgentError) -> ApiError {
    match e {
        AgentError::NotFound(_) => ApiError::not_found(),
        AgentError::AgentAlreadyActive(_) | AgentError::AgentTerminated(_) => ApiError::new(
            StatusCode::CONFLICT,
            "LIFECYCLE_CONFLICT",
            "Lifecycle conflict",
            e.to_string(),
        ),
        AgentError::InvalidStateTransition { .. } => ApiError::new(
            StatusCode::CONFLICT,
            "INVALID_LIFECYCLE_TRANSITION",
            "Invalid lifecycle transition",
            e.to_string(),
        ),
        AgentError::InvalidAgentCode(..)
        | AgentError::InvalidEffectiveDate(_)
        | AgentError::MissingSuspensionReason
        | AgentError::MissingTerminationReason
        | AgentError::AppointmentNotValid(_)
        | AgentError::Validation(_) => ApiError::validation(e.to_string()),
        AgentError::ActivationNotAuthorized(_) => ApiError::new(
            StatusCode::FORBIDDEN,
            "ACTIVATION_NOT_AUTHORIZED",
            "Activation not authorized",
            e.to_string(),
        ),
        AgentError::IdentityLinkConflict(_) => ApiError::new(
            StatusCode::CONFLICT,
            "IDENTITY_LINK_CONFLICT",
            "Identity link conflict",
            e.to_string(),
        ),
        AgentError::IdempotencyConflict => ApiError::new(
            StatusCode::CONFLICT,
            "IDEMPOTENCY_CONFLICT",
            "Idempotency conflict",
            "duplicate command",
        ),
        AgentError::VersionConflict { .. } => ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "VERSION_CONFLICT",
            "Version conflict",
            e.to_string(),
        ),
    }
}

#[derive(Serialize)]
struct ProblemBody {
    #[serde(rename = "type")]
    problem_type: String,
    title: String,
    status: u16,
    detail: String,
    code: String,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    correlation_id: Option<Uuid>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ProblemBody {
            problem_type: format!(
                "https://api.iaop.internal/problems/{}",
                self.code.to_lowercase()
            ),
            title: self.title.to_string(),
            status: self.status.as_u16(),
            detail: self.detail,
            code: self.code.to_string(),
            correlation_id: self.correlation_id,
        };
        (self.status, Json(body)).into_response()
    }
}
