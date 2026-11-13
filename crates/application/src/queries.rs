//! Application queries: read-only use cases.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use iaop_agent_service_domain::{AgentCode, AgentId};

use crate::commands::{AgentView, AppointmentView, IdentityLinkView};
use crate::error::AppError;
use crate::ports::{AgentStore, LifecycleHistoryRecord};

/// Loads an agent by ID; `None` when absent.
pub async fn get_agent_by_id(
    pool: &PgPool,
    store: &impl AgentStore,
    id: AgentId,
) -> Result<Option<AgentView>, AppError> {
    Ok(store
        .find_agent_by_id(pool, id)
        .await?
        .as_ref()
        .map(AgentView::from))
}

/// Loads an agent by business code.
pub async fn get_agent_by_code(
    pool: &PgPool,
    store: &impl AgentStore,
    code: &str,
) -> Result<Option<AgentView>, AppError> {
    let code = AgentCode::new(code).map_err(AppError::Domain)?;
    Ok(store
        .find_agent_by_code(pool, &code)
        .await?
        .as_ref()
        .map(AgentView::from))
}

/// Cursor-paginated agent list. `cursor` is the last seen agent ID;
/// ordering is by `(created_at, id)` — deterministic.
pub async fn list_agents(
    pool: &PgPool,
    store: &impl AgentStore,
    cursor: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AgentView>, AppError> {
    let agents = store
        .list_agents(pool, cursor.map(AgentId::from_uuid), limit)
        .await?;
    Ok(agents.iter().map(AgentView::from).collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleHistoryItem {
    pub id: Uuid,
    pub from_status: Option<String>,
    pub to_status: String,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub effective_at: DateTime<Utc>,
    pub performed_at: DateTime<Utc>,
    pub performed_by: Option<String>,
    pub correlation_id: Option<Uuid>,
    pub command_id: Uuid,
    pub aggregate_version: u64,
}

impl From<&LifecycleHistoryRecord> for LifecycleHistoryItem {
    fn from(t: &LifecycleHistoryRecord) -> Self {
        Self {
            id: t.id,
            from_status: t.from_status.clone(),
            to_status: t.to_status.clone(),
            reason_code: t.reason_code.clone(),
            reason_text: t.reason_text.clone(),
            effective_at: t.effective_at,
            performed_at: t.performed_at,
            performed_by: t.performed_by.clone(),
            correlation_id: t.correlation_id,
            command_id: t.command_id,
            aggregate_version: t.aggregate_version as u64,
        }
    }
}

/// Lifecycle history for an agent, newest first, bounded.
pub async fn get_lifecycle_history(
    pool: &PgPool,
    store: &impl AgentStore,
    agent_id: AgentId,
    limit: i64,
) -> Result<Vec<LifecycleHistoryItem>, AppError> {
    // Ensure the agent exists before listing history.
    if store.find_agent_by_id(pool, agent_id).await?.is_none() {
        return Err(AppError::AgentNotFound);
    }
    let rows = store.list_lifecycle_history(pool, agent_id, limit).await?;
    Ok(rows.iter().map(LifecycleHistoryItem::from).collect())
}

/// Appointments for an agent.
pub async fn get_agent_appointments(
    pool: &PgPool,
    store: &impl AgentStore,
    agent_id: AgentId,
) -> Result<Vec<AppointmentView>, AppError> {
    if store.find_agent_by_id(pool, agent_id).await?.is_none() {
        return Err(AppError::AgentNotFound);
    }
    let rows = store.list_appointments(pool, agent_id).await?;
    Ok(rows.iter().map(AppointmentView::from).collect())
}

/// Identity links for an agent.
pub async fn get_agent_identity_links(
    pool: &PgPool,
    store: &impl AgentStore,
    agent_id: AgentId,
) -> Result<Vec<IdentityLinkView>, AppError> {
    if store.find_agent_by_id(pool, agent_id).await?.is_none() {
        return Err(AppError::AgentNotFound);
    }
    let rows = store.list_identity_links(pool, agent_id).await?;
    Ok(rows.iter().map(IdentityLinkView::from).collect())
}
