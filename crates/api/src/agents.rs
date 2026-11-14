//! Agent registry HTTP handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::Deserialize;
use utoipa::IntoParams;

use iaop_agent_service_application::commands::{
    self, CreateAgentInput, LifecycleInput, LinkIdentityInput, UnlinkIdentityInput,
};
use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::permissions::{Permission, has_permission};
use iaop_agent_service_application::queries;
use iaop_agent_service_domain::{AgentId, IdentityLinkId};

use crate::auth::AuthPrincipal;
use crate::dto::*;
use crate::error_mapping::ApiError;
use crate::state::AppState;

const IDEMPOTENCY_HEADER: &str = "idempotency-key";

fn services(state: &AppState) -> Result<&std::sync::Arc<crate::state::ServicePorts>, ApiError> {
    state.services.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
            "Service unavailable",
            "persistence not configured",
        )
    })
}

fn require(
    _state: &AppState,
    p: &iaop_agent_service_application::Principal,
    perm: Permission,
) -> Result<(), ApiError> {
    if has_permission(p, perm) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Forbidden",
            format!("missing permission {}", perm.as_str()),
        ))
    }
}

/// Realm roles whose holders may read any agent record (staff scopes).
/// Self-service agents hold only `agent:*` client permissions and must
/// pass the self-or-scope check in [`authorize_agent_read`].
const STAFF_AGENT_READ_ROLES: &[&str] = &[
    "PLATFORM_ADMIN",
    "AGENT_ADMIN",
    "AGENT_OPERATIONS",
    "AGENT_LIFECYCLE_OFFICER",
    "AGENT_AUDITOR",
];

fn is_staff_reader(p: &iaop_agent_service_application::Principal) -> bool {
    p.roles
        .iter()
        .any(|r| STAFF_AGENT_READ_ROLES.contains(&r.as_str()))
}

/// Resource-level read authorization: Authentication + Permission +
/// Resource Scope. Staff roles read any agent; self-service callers may
/// only read their own agent record, proven by an ACTIVE identity link
/// binding issuer+subject to the target agent_id. Org-wide manager
/// scoping requires agency membership data and remains a documented
/// gap — non-staff callers are denied rather than assumed.
async fn authorize_agent_read(
    state: &AppState,
    principal: &iaop_agent_service_application::Principal,
    target_agent_id: uuid::Uuid,
) -> Result<(), ApiError> {
    require(state, principal, Permission::Read)?;
    if is_staff_reader(principal) {
        return Ok(());
    }
    let link = queries::resolve_identity_link(
        pool(state)?,
        &services(state)?.agent_store,
        &principal.issuer,
        &principal.subject,
    )
    .await?;
    match link {
        Some(l)
            if l.status.eq_ignore_ascii_case("ACTIVE")
                && l.unlinked_at.is_none()
                && l.agent_id == target_agent_id =>
        {
            Ok(())
        }
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Forbidden",
            "agent record is outside caller scope",
        )),
    }
}

fn idem_key(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 255)
        .map(str::to_string)
        .ok_or_else(|| ApiError::validation("Idempotency-Key header is required (1-255 chars)"))
}

fn expected_version(headers: &HeaderMap) -> Result<Option<u64>, ApiError> {
    match headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        None => Ok(None),
        Some(v) => {
            let v = v.trim().trim_matches('"');
            v.parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::validation("If-Match must be an integer aggregate version"))
        }
    }
}

fn correlation_id(headers: &HeaderMap) -> Option<uuid::Uuid> {
    headers
        .get("x-correlation-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
            "Service unavailable",
            "persistence not configured",
        )
    })
}

fn lifecycle_input(
    agent_id: uuid::Uuid,
    body: LifecycleCommandRequest,
    headers: &HeaderMap,
    key: String,
) -> Result<LifecycleInput, ApiError> {
    Ok(LifecycleInput {
        agent_id,
        appointment_id: body.appointment_id,
        reason_code: body.reason_code,
        reason_text: body.reason_text,
        expected_version: expected_version(headers)?,
        idempotency_key: key,
    })
}

async fn run_cmd(
    result: Result<(i32, serde_json::Value), AppError>,
) -> Result<impl IntoResponse, ApiError> {
    let (status, body) = result.map_err(ApiError::from)?;
    let status = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
    Ok((status, Json(body)))
}

// ---- endpoints -----------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/v1/agents",
    operation_id = "Agent:register",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:create — register a new agent (DRAFT). Requires Idempotency-Key."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn create_agent(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    headers: HeaderMap,
    Json(body): Json<CreateAgentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = services(&state)?;
    require(&state, &principal, Permission::Create)?;
    let key = idem_key(&headers)?;
    let input = CreateAgentInput {
        person_id: body.person_id,
        agent_code: body.agent_code,
        idempotency_key: key,
    };
    let result = commands::create_agent(
        pool(&state)?,
        &svc.agent_store,
        &svc.idempotency,
        &principal,
        input,
        correlation_id(&headers),
    )
    .await;
    run_cmd(result).await
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListParams {
    /// Page size (1-100, default 20).
    #[serde(default = "default_limit")]
    #[param(default = 20, minimum = 1, maximum = 100)]
    pub limit: i64,
    /// Cursor: last agent ID from previous page.
    pub cursor: Option<uuid::Uuid>,
}
fn default_limit() -> i64 {
    20
}

#[utoipa::path(
    get,
    path = "/api/v1/agents",
    operation_id = "Agent:list",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:read — list agents (keyset pagination via cursor/limit)."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn list_agents(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, ApiError> {
    require(&state, &principal, Permission::Read)?;
    if !is_staff_reader(&principal) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Forbidden",
            "agent registry listing requires a staff scope",
        ));
    }
    let limit = params.limit.clamp(1, 100);
    let mut agents = queries::list_agents(
        pool(&state)?,
        &services(&state)?.agent_store,
        params.cursor,
        limit + 1,
    )
    .await?;
    let next_cursor = if agents.len() as i64 > limit {
        agents.pop().map(|a| a.id)
    } else {
        None
    };
    let data: Vec<AgentViewDto> = agents
        .into_iter()
        .map(|v| AgentViewDto {
            id: v.id,
            person_id: v.person_id,
            agent_code: v.agent_code,
            lifecycle_status: v.lifecycle_status,
            version: v.version,
            created_at: v.created_at,
        })
        .collect();
    Ok(Json(AgentListResponse { data, next_cursor }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}",
    operation_id = "Agent:getById",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:read — get agent by ID."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn get_agent(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_agent_read(&state, &principal, agent_id).await?;
    let agent = queries::get_agent_by_id(
        pool(&state)?,
        &services(&state)?.agent_store,
        AgentId::from_uuid(agent_id),
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(Json(AgentViewDto {
        id: agent.id,
        person_id: agent.person_id,
        agent_code: agent.agent_code,
        lifecycle_status: agent.lifecycle_status,
        version: agent.version,
        created_at: agent.created_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/code/{code}",
    operation_id = "Agent:getByCode",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:read — get agent by code."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn get_agent_by_code(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    require(&state, &principal, Permission::Read)?;
    let agent = queries::get_agent_by_code(pool(&state)?, &services(&state)?.agent_store, &code)
        .await?
        .ok_or_else(ApiError::not_found)?;
    authorize_agent_read(&state, &principal, agent.id).await?;
    Ok(Json(AgentViewDto {
        id: agent.id,
        person_id: agent.person_id,
        agent_code: agent.agent_code,
        lifecycle_status: agent.lifecycle_status,
        version: agent.version,
        created_at: agent.created_at,
    }))
}

macro_rules! lifecycle_handler {
    ($name:ident, $cmd:ident, $perm:expr) => {
        async fn $name(
            State(state): State<AppState>,
            AuthPrincipal(principal): AuthPrincipal,
            Path(agent_id): Path<uuid::Uuid>,
            headers: HeaderMap,
            Json(body): Json<LifecycleCommandRequest>,
        ) -> Result<impl IntoResponse, ApiError> {
            let svc = services(&state)?;
            require(&state, &principal, $perm)?;
            let key = idem_key(&headers)?;
            let input = lifecycle_input(agent_id, body, &headers, key)?;
            let result = commands::$cmd(
                pool(&state)?,
                &svc.agent_store,
                &svc.idempotency,
                &svc.activation,
                &principal,
                input,
                correlation_id(&headers),
            )
            .await;
            run_cmd(result).await
        }
    };
}

lifecycle_handler!(
    submit_for_activation_inner,
    submit_for_activation,
    Permission::Activate
);
lifecycle_handler!(activate_agent_inner, activate_agent, Permission::Activate);
lifecycle_handler!(suspend_agent_inner, suspend_agent, Permission::Suspend);
lifecycle_handler!(
    reinstate_agent_inner,
    reinstate_agent,
    Permission::Reinstate
);
lifecycle_handler!(
    terminate_agent_inner,
    terminate_agent,
    Permission::Terminate
);

macro_rules! lifecycle_wrapper {
    ($wrapper:ident, $inner:ident, $path:literal, $opid:literal, $desc:literal) => {
        #[utoipa::path(
                    post,
                    path = $path,
                    operation_id = $opid,
                    tag = "agents",
                    security(("bearerAuth" = [])),
                    request_body = LifecycleCommandRequest,
                    responses(
                        (status = 200, description = $desc, body = serde_json::Value),
                        (status = 400, description = "Bad request", body = ProblemDetails),
                        (status = 401, description = "Unauthenticated", body = ProblemDetails),
                        (status = 403, description = "Forbidden", body = ProblemDetails),
                        (status = 404, description = "Not found", body = ProblemDetails),
                        (status = 409, description = "Conflict", body = ProblemDetails),
                        (status = 422, description = "Validation failed", body = ProblemDetails),
                    )
                )]
        pub async fn $wrapper(
            State(state): State<AppState>,
            AuthPrincipal(principal): AuthPrincipal,
            Path(agent_id): Path<uuid::Uuid>,
            headers: HeaderMap,
            Json(body): Json<LifecycleCommandRequest>,
        ) -> Result<impl IntoResponse, ApiError> {
            $inner(
                State(state),
                AuthPrincipal(principal),
                Path(agent_id),
                headers,
                Json(body),
            )
            .await
        }
    };
}

lifecycle_wrapper!(
    submit_for_activation,
    submit_for_activation_inner,
    "/api/v1/agents/{id}/submit-for-activation",
    "Agent:activate:submit",
    "Submit agent for activation (appointment required)"
);
lifecycle_wrapper!(
    activate_agent,
    activate_agent_inner,
    "/api/v1/agents/{id}/activate",
    "Agent:activate:execute",
    "Activate a submitted agent"
);
lifecycle_wrapper!(
    suspend_agent,
    suspend_agent_inner,
    "/api/v1/agents/{id}/suspend",
    "Agent:suspend",
    "Suspend an active agent (reason required)"
);
lifecycle_wrapper!(
    reinstate_agent,
    reinstate_agent_inner,
    "/api/v1/agents/{id}/reinstate",
    "Agent:reinstate",
    "Reactivate a suspended agent"
);
lifecycle_wrapper!(
    terminate_agent,
    terminate_agent_inner,
    "/api/v1/agents/{id}/terminate",
    "Agent:terminate",
    "Terminate an agent (reason required)"
);

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/history",
    operation_id = "Agent:history:read",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent lifecycle:history — lifecycle transition history."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn lifecycle_history(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent_id): Path<uuid::Uuid>,
    Query(params): Query<HistoryParams>,
) -> Result<impl IntoResponse, ApiError> {
    require(&state, &principal, Permission::HistoryRead)?;
    authorize_agent_read(&state, &principal, agent_id).await?;
    let items = queries::get_lifecycle_history(
        pool(&state)?,
        &services(&state)?.agent_store,
        AgentId::from_uuid(agent_id),
        params.limit.clamp(1, 500),
    )
    .await?;
    let data: Vec<LifecycleHistoryItemDto> = items
        .into_iter()
        .map(|i| LifecycleHistoryItemDto {
            id: i.id,
            from_status: i.from_status,
            to_status: i.to_status,
            reason_code: i.reason_code,
            reason_text: i.reason_text,
            effective_at: i.effective_at,
            performed_at: i.performed_at,
            performed_by: i.performed_by,
            correlation_id: i.correlation_id,
            command_id: i.command_id,
            aggregate_version: i.aggregate_version,
        })
        .collect();
    Ok(Json(serde_json::json!({ "data": data })))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct HistoryParams {
    #[serde(default = "default_history_limit")]
    #[param(default = 100, minimum = 1, maximum = 500)]
    pub limit: i64,
}
fn default_history_limit() -> i64 {
    100
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/appointments",
    operation_id = "Agent:getAppointments",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:read — appointments for an agent."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn agent_appointments(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_agent_read(&state, &principal, agent_id).await?;
    let rows = queries::get_agent_appointments(
        pool(&state)?,
        &services(&state)?.agent_store,
        AgentId::from_uuid(agent_id),
    )
    .await?;
    let data: Vec<AppointmentDto> = rows
        .into_iter()
        .map(|a| AppointmentDto {
            id: a.id,
            agent_id: a.agent_id,
            appointment_reference: a.appointment_reference,
            company_reference: a.company_reference,
            appointment_type: a.appointment_type,
            effective_from: a.effective_from,
            effective_to: a.effective_to,
            status: a.status,
        })
        .collect();
    Ok(Json(serde_json::json!({ "data": data })))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/identity-links",
    operation_id = "Agent:getIdentityLinks",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent registry:read — identity links for an agent."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn agent_identity_links(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_agent_read(&state, &principal, agent_id).await?;
    let rows = queries::get_agent_identity_links(
        pool(&state)?,
        &services(&state)?.agent_store,
        AgentId::from_uuid(agent_id),
    )
    .await?;
    let data: Vec<IdentityLinkDto> = rows
        .into_iter()
        .map(|l| IdentityLinkDto {
            id: l.id,
            agent_id: l.agent_id,
            identity_provider: l.identity_provider,
            issuer: l.issuer,
            subject: l.subject,
            status: l.status,
            linked_at: l.linked_at,
            unlinked_at: l.unlinked_at,
        })
        .collect();
    Ok(Json(serde_json::json!({ "data": data })))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/identity-links",
    operation_id = "Agent:linkIdentity",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent identity:link — link an IdP identity subject to an agent."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn link_identity(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent_id): Path<uuid::Uuid>,
    headers: HeaderMap,
    Json(body): Json<LinkIdentityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = services(&state)?;
    require(&state, &principal, Permission::IdentityLink)?;
    let key = idem_key(&headers)?;
    let input = LinkIdentityInput {
        agent_id,
        identity_provider: body.identity_provider,
        issuer: body.issuer,
        subject: body.subject,
        expected_version: expected_version(&headers)?,
        idempotency_key: key,
    };
    let result = commands::link_identity(
        pool(&state)?,
        &svc.agent_store,
        &svc.idempotency,
        &principal,
        input,
        correlation_id(&headers),
    )
    .await;
    run_cmd(result).await
}

#[utoipa::path(
    delete,
    path = "/api/v1/agents/{id}/identity-links/{link_id}",
    operation_id = "Agent:unlinkIdentity",
    tag = "agents",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Agent identity:unlink — unlink an identity link."),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict", body = ProblemDetails),
        (status = 422, description = "Validation failed", body = ProblemDetails),
    )
)]
pub async fn unlink_identity(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path((agent_id, link_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let svc = services(&state)?;
    require(&state, &principal, Permission::IdentityLink)?;
    let key = idem_key(&headers)?;
    let input = UnlinkIdentityInput {
        agent_id,
        link_id: IdentityLinkId::from_uuid(link_id).as_uuid(),
        expected_version: expected_version(&headers)?,
        idempotency_key: key,
    };
    let result = commands::unlink_identity(
        pool(&state)?,
        &svc.agent_store,
        &svc.idempotency,
        &principal,
        input,
        correlation_id(&headers),
    )
    .await;
    run_cmd(result).await
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ResolveIdentityQuery {
    pub issuer: String,
    pub subject: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/identity-links/resolve",
    operation_id = "Agent:resolveIdentity",
    tag = "agents",
    params(ResolveIdentityQuery),
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Resolved active identity link", body = IdentityLinkDto),
        (status = 401, description = "Unauthenticated", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails)
    )
)]
pub async fn resolve_identity(
    State(state): State<AppState>,
    AuthPrincipal(principal): AuthPrincipal,
    Query(q): Query<ResolveIdentityQuery>,
) -> Result<Json<IdentityLinkDto>, ApiError> {
    let svc = services(&state)?;
    // Allow if caller has read permission OR caller is resolving their own subject
    if !has_permission(&principal, Permission::Read)
        && !has_permission(&principal, Permission::IdentityLink)
        && principal.subject != q.subject
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Forbidden",
            "insufficient permission to resolve identity",
        ));
    }
    let opt =
        queries::resolve_identity_link(pool(&state)?, &svc.agent_store, &q.issuer, &q.subject)
            .await?;

    match opt {
        Some(l) => Ok(Json(IdentityLinkDto {
            id: l.id,
            agent_id: l.agent_id,
            identity_provider: l.identity_provider,
            issuer: l.issuer,
            subject: l.subject,
            status: l.status,
            linked_at: l.linked_at,
            unlinked_at: l.unlinked_at,
        })),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Not found",
            "active identity link not found for issuer and subject",
        )),
    }
}
