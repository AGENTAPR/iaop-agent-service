//! JWT access-token validation against an OIDC provider's JWKS.
//!
//! - RS256 only (algorithm pinned — `alg: none` and HS* are rejected)
//! - issuer, audience, expiry, not-before verified by `jsonwebtoken`
//! - JWKS fetched over HTTPS and cached; refreshed on unknown `kid`
//!   or after TTL
//! - tokens are never logged

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tracing::warn;

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::TokenValidator;
use iaop_agent_service_application::{Principal, TokenClaims};

const JWKS_TTL: Duration = Duration::from_secs(300);
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(rename = "use")]
    #[allow(dead_code)]
    usage: Option<String>,
    n: String,
    e: String,
    #[allow(dead_code)]
    alg: Option<String>,
}

struct CachedKey {
    n: String,
    e: String,
}

/// JWKS-backed JWT validator.
pub struct JwksTokenValidator {
    issuer: String,
    audience: String,
    jwks_url: String,
    http: reqwest::Client,
    cache: Mutex<(Instant, HashMap<String, CachedKey>)>,
}

impl JwksTokenValidator {
    pub fn new(issuer: String, jwks_url: String, audience: String) -> Self {
        Self {
            issuer,
            audience,
            jwks_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            cache: Mutex::new((Instant::now() - JWKS_TTL, HashMap::new())),
        }
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, CachedKey>, AppError> {
        let jwks: Jwks = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(AppError::infra)?
            .error_for_status()
            .map_err(AppError::infra)?
            .json()
            .await
            .map_err(AppError::infra)?;
        let map = jwks
            .keys
            .into_iter()
            .filter(|k| k.kty == "RSA")
            .map(|k| (k.kid, CachedKey { n: k.n, e: k.e }))
            .collect();
        Ok(map)
    }

    /// Returns the decoding key for `kid`, refreshing the cache when the
    /// key is unknown or the cache is stale. The mutex is never held
    /// across an `.await`.
    async fn key_for_kid(&self, kid: &str) -> Result<DecodingKey, AppError> {
        let (fetched_at, has_key) = {
            let guard = self
                .cache
                .lock()
                .map_err(|_| AppError::infra_msg("jwks cache poisoned"))?;
            (guard.0, guard.1.contains_key(kid))
        };
        if !has_key || fetched_at.elapsed() > JWKS_TTL {
            // Rate-limit refreshes to avoid hammering the IdP on bad tokens.
            let last_fetch = {
                let guard = self
                    .cache
                    .lock()
                    .map_err(|_| AppError::infra_msg("jwks cache poisoned"))?;
                guard.0
            };
            if last_fetch.elapsed() > JWKS_MIN_REFRESH || !has_key {
                match self.fetch_jwks().await {
                    Ok(keys) => {
                        let mut guard = self
                            .cache
                            .lock()
                            .map_err(|_| AppError::infra_msg("jwks cache poisoned"))?;
                        *guard = (Instant::now(), keys);
                    }
                    Err(e) => {
                        warn!(error = %e, "JWKS refresh failed; using stale cache if possible");
                    }
                }
            }
        }
        let guard = self
            .cache
            .lock()
            .map_err(|_| AppError::infra_msg("jwks cache poisoned"))?;
        let key = guard
            .1
            .get(kid)
            .ok_or_else(|| AppError::infra_msg("unknown signing key (kid)"))?;
        DecodingKey::from_rsa_components(&key.n, &key.e).map_err(AppError::infra)
    }
}

impl TokenValidator for JwksTokenValidator {
    async fn validate(&self, token: &str) -> Result<Principal, AppError> {
        let header = decode_header(token).map_err(|_| unauthorized("malformed token"))?;
        if header.alg != Algorithm::RS256 {
            return Err(unauthorized("unsupported token algorithm"));
        }
        let kid = header
            .kid
            .ok_or_else(|| unauthorized("token missing kid"))?;
        let decoding_key = self.key_for_kid(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        let data = decode::<TokenClaims>(token, &decoding_key, &validation).map_err(|e| {
            // Log the failure class only — never the token.
            warn!(error_kind = ?e.kind(), "token validation failed");
            unauthorized("invalid token")
        })?;
        let c = data.claims;

        Ok(Principal {
            subject: c.sub,
            issuer: c.iss,
            scopes: c
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            roles: c.realm_access.map(|r| r.roles).unwrap_or_default(),
            agent_scope: c.agent_scope,
        })
    }
}

fn unauthorized(msg: &str) -> AppError {
    AppError::Forbidden(format!("unauthenticated: {msg}"))
}

/// Development bypass validator — produces a fixed privileged principal.
/// Only constructed when `AUTH_MODE=dev` and `ENVIRONMENT != production`
/// (enforced in `AppConfig::from_env`).
pub struct DevBypassValidator;

impl TokenValidator for DevBypassValidator {
    async fn validate(&self, _token: &str) -> Result<Principal, AppError> {
        Ok(Principal {
            subject: "dev-local-user".into(),
            issuer: "local-dev".into(),
            scopes: vec![],
            roles: vec!["AGENT_ADMIN".into()],
            agent_scope: None,
        })
    }
}

/// Concrete validator selected by configuration (dev bypass vs JWKS).
pub enum TokenValidatorImpl {
    Dev(DevBypassValidator),
    Jwks(JwksTokenValidator),
}

impl TokenValidator for TokenValidatorImpl {
    async fn validate(&self, token: &str) -> Result<Principal, AppError> {
        match self {
            Self::Dev(v) => v.validate(token).await,
            Self::Jwks(v) => v.validate(token).await,
        }
    }
}
