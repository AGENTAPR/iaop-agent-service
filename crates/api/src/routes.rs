//! HTTP route handlers for system endpoints.
//!
//! Agent registry endpoints live in `crate::agents`; both are mounted
//! by [`build_router`].

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use iaop_agent_service_application::service_info::GetServiceInfoQuery;
use iaop_agent_service_infrastructure::persistence;
use utoipa::OpenApi;

use crate::dto::{HealthResponse, SystemInfoResponse};
use crate::middleware;
use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Builds the service router: system endpoints + agent registry.
pub fn build_router(state: AppState) -> Router {
    let agent_routes = axum::Router::new()
        .route(
            "/api/v1/agents",
            axum::routing::post(crate::agents::create_agent).get(crate::agents::list_agents),
        )
        .route(
            "/api/v1/agents/code/{code}",
            get(crate::agents::get_agent_by_code),
        )
        .route("/api/v1/agents/{id}", get(crate::agents::get_agent))
        .route(
            "/api/v1/agents/{id}/submit-for-activation",
            axum::routing::post(crate::agents::submit_for_activation),
        )
        .route(
            "/api/v1/agents/{id}/activate",
            axum::routing::post(crate::agents::activate_agent),
        )
        .route(
            "/api/v1/agents/{id}/suspend",
            axum::routing::post(crate::agents::suspend_agent),
        )
        .route(
            "/api/v1/agents/{id}/reinstate",
            axum::routing::post(crate::agents::reinstate_agent),
        )
        .route(
            "/api/v1/agents/{id}/terminate",
            axum::routing::post(crate::agents::terminate_agent),
        )
        .route(
            "/api/v1/agents/{id}/history",
            get(crate::agents::lifecycle_history),
        )
        .route(
            "/api/v1/agents/{id}/appointments",
            get(crate::agents::agent_appointments),
        )
        .route(
            "/api/v1/agents/{id}/identity-links",
            get(crate::agents::agent_identity_links).post(crate::agents::link_identity),
        )
        .route(
            "/api/v1/agents/{id}/identity-links/{link_id}",
            axum::routing::delete(crate::agents::unlink_identity),
        );

    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system/info", get(system_info))
        .route("/api/docs/openapi.json", get(openapi_json))
        .merge(agent_routes)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::request_context,
        ))
        .with_state(state)
}

/// Liveness: process is up. Must NOT depend on external systems.
#[utoipa::path(
    get,
    path = "/health/live",
    tag = "system",
    responses((status = 200, description = "Process is alive", body = HealthResponse))
)]
pub async fn live() -> impl IntoResponse {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

/// Readiness: dependencies required to serve traffic.
/// - No DB configured: ready (DB intentionally disabled).
/// - DB configured: ready only if a query succeeds.
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "system",
    responses((status = 200, description = "Ready", body = HealthResponse), (status = 503, description = "Degraded", body = HealthResponse))
)]
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        Some(pool) => {
            if persistence::ping(pool).await {
                (StatusCode::OK, Json(HealthResponse { status: "ready" }))
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(HealthResponse { status: "degraded" }),
                )
            }
        }
        None => (StatusCode::OK, Json(HealthResponse { status: "ready" })),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/system/info",
    tag = "system",
    responses((status = 200, description = "Service metadata", body = SystemInfoResponse))
)]
pub async fn system_info(State(state): State<AppState>) -> impl IntoResponse {
    let info = GetServiceInfoQuery {
        service_name: state.config.service_name.clone(),
        service_version: state.config.service_version.clone(),
        environment: state.config.environment.clone(),
    }
    .execute();
    Json(SystemInfoResponse {
        service: info.service,
        version: info.version,
        environment: info.environment,
        status: info.status,
    })
}

async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}
