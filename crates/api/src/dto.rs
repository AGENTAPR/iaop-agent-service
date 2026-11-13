//! Request/response DTOs for the agent registry API.
//!
//! DTOs are transport shapes only — they never carry domain internals
//! and cannot construct authorization evidence.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ---- shared ------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemInfoResponse {
    pub service: String,
    pub version: String,
    pub environment: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    #[schema(example = "https://api.iaop.internal/problems/validation_error")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
}

// ---- agent views --------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentViewDto {
    pub id: Uuid,
    pub person_id: Uuid,
    pub agent_code: String,
    pub lifecycle_status: String,
    pub version: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentListResponse {
    pub data: Vec<AgentViewDto>,
    /// Pass as `cursor` query param for the next page; absent when done.
    pub next_cursor: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAgentRequest {
    /// Person identity this agent record belongs to.
    pub person_id: Uuid,
    /// Optional caller-supplied code; generated `AGT-…` when omitted.
    #[serde(default)]
    pub agent_code: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LifecycleCommandRequest {
    /// Appointment the lifecycle operation is scoped to.
    /// Required for submit-for-activation, activate, reinstate.
    #[serde(default)]
    pub appointment_id: Option<Uuid>,
    /// Reason code (required for suspend/terminate).
    #[serde(default)]
    pub reason_code: Option<String>,
    /// Reason text (required for suspend/terminate).
    #[serde(default)]
    pub reason_text: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppointmentDto {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub appointment_reference: Option<String>,
    pub company_reference: Option<String>,
    pub appointment_type: String,
    pub effective_from: chrono::NaiveDate,
    pub effective_to: Option<chrono::NaiveDate>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IdentityLinkDto {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub identity_provider: String,
    pub issuer: String,
    pub subject: String,
    pub status: String,
    pub linked_at: chrono::DateTime<chrono::Utc>,
    pub unlinked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkIdentityRequest {
    /// Identity provider name (e.g. `keycloak`).
    pub identity_provider: String,
    /// Token issuer URL.
    pub issuer: String,
    /// Token subject (`sub` claim) — unique only within `issuer`.
    pub subject: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LifecycleHistoryItemDto {
    pub id: Uuid,
    pub from_status: Option<String>,
    pub to_status: String,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub effective_at: chrono::DateTime<chrono::Utc>,
    pub performed_at: chrono::DateTime<chrono::Utc>,
    pub performed_by: Option<String>,
    pub correlation_id: Option<Uuid>,
    pub command_id: Uuid,
    pub aggregate_version: u64,
}
