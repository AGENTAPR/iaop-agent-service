//! JWT access-token validation delegated to the shared `iaop-security`
//! crate (JWKS cache, RS256 pinning, iss/aud/exp/nbf).
//! This module only maps validated claims to the service `Principal`.
//! Tokens are never logged.

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::TokenValidator;
use iaop_agent_service_application::{Principal, TokenClaims};
use iaop_security::jwks::{JwtValidator, RemoteJwksProvider};
use tracing::warn;

/// JWKS-backed JWT validator (shared implementation).
pub struct JwksTokenValidator {
    inner: JwtValidator<RemoteJwksProvider>,
}

impl JwksTokenValidator {
    pub fn new(issuer: String, jwks_url: String, audience: String) -> Self {
        Self {
            inner: JwtValidator::new(RemoteJwksProvider::new(jwks_url), issuer, audience),
        }
    }
}

fn unauthorized(msg: &str) -> AppError {
    // auth.rs maps Forbidden from the token validator to HTTP 401.
    AppError::Forbidden(msg.to_string())
}

impl TokenValidator for JwksTokenValidator {
    async fn validate(&self, token: &str) -> Result<Principal, AppError> {
        let c = self
            .inner
            .validate::<TokenClaims>(token)
            .await
            .map_err(|e| {
                warn!(error = %e, "token validation failed");
                unauthorized("invalid token")
            })?;

        Ok(Principal {
            subject: c.sub,
            issuer: c.iss,
            scopes: c
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            roles: {
                let mut roles = c.realm_access.map(|r| r.roles).unwrap_or_default();
                // Merge client roles only for clients present in `aud`
                // (resource_access can list clients the token isn't for).
                let aud: Vec<String> = match &c.aud {
                    Some(serde_json::Value::String(a)) => vec![a.clone()],
                    Some(serde_json::Value::Array(a)) => a
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                    _ => vec![],
                };
                if let Some(ra) = &c.resource_access {
                    for (client, cr) in ra {
                        if aud.iter().any(|a| a == client) {
                            for r in &cr.roles {
                                if !roles.contains(r) {
                                    roles.push(r.clone());
                                }
                            }
                        }
                    }
                }
                roles
            },
            agent_scope: c.agent_scope,
        })
    }
}

/// Dev-mode validator: accepts a fixed principal. Gated by config —
/// rejected in production (`AUTH_MODE=dev` is refused there).
pub struct DevBypassValidator;

/// Selects the active token validator implementation.
pub enum TokenValidatorImpl {
    Jwks(JwksTokenValidator),
    Dev(DevBypassValidator),
}

impl TokenValidator for DevBypassValidator {
    async fn validate(&self, _token: &str) -> Result<Principal, AppError> {
        Ok(Principal {
            subject: "dev-user".into(),
            issuer: "dev".into(),
            scopes: vec![],
            roles: vec![
                "agent:create".into(),
                "agent:read".into(),
                "agent:activate".into(),
                "agent:suspend".into(),
                "agent:reinstate".into(),
                "agent:terminate".into(),
                "agent:identity:link".into(),
                "agent:history:read".into(),
            ],
            agent_scope: None,
        })
    }
}

impl TokenValidator for TokenValidatorImpl {
    async fn validate(&self, token: &str) -> Result<Principal, AppError> {
        match self {
            Self::Jwks(v) => v.validate(token).await,
            Self::Dev(v) => v.validate(token).await,
        }
    }
}
