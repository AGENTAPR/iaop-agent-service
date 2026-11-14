//! Authentication extractor: validates Bearer JWTs (or the gated dev
//! bypass) and produces a [`Principal`].
//!
//! Health/docs paths are public (see `is_public_path`); every
//! `/api/v1/agents*` route requires a valid token.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use iaop_agent_service_application::Principal;
use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::TokenValidator;

use crate::state::AppState;

/// Authenticated principal extracted from the request.
pub struct AuthPrincipal(pub Principal);

impl FromRequestParts<AppState> for AuthPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let services = state
            .services
            .as_ref()
            .ok_or_else(service_unavailable)?;

        let token = iaop_agent_service_infrastructure::security::bearer_token(
            parts
                .headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
        )
        .ok_or_else(unauthorized)?;

        let principal = services
            .token_validator
            .validate(token)
            .await
            .map_err(|e| match e {
                AppError::Forbidden(_) => unauthorized(),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "identity provider unavailable",
                )
                    .into_response(),
            })?;

        Ok(AuthPrincipal(principal))
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        r#"{"type":"https://api.iaop.internal/problems/unauthenticated","title":"Unauthenticated","status":401,"code":"UNAUTHENTICATED","detail":"A valid Bearer token is required"}"#,
    )
        .into_response()
}

fn service_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"type":"https://api.iaop.internal/problems/service_unavailable","title":"Service unavailable","status":503,"code":"SERVICE_UNAVAILABLE","detail":"Agent registry persistence is not configured"}"#,
    )
        .into_response()
}
