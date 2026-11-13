//! Authenticated principal and actor representation.
//!
//! A `Principal` is produced ONLY by the authentication layer (JWT
//! validation in `infrastructure::security`) or by explicitly gated
//! development/test providers. API DTOs cannot construct one.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Authenticated caller of the API.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Token subject (`sub` claim) — the external identity, NOT an
    /// agent or person ID.
    pub subject: String,
    /// Token issuer.
    pub issuer: String,
    /// Scopes from the `scope` claim.
    pub scopes: Vec<String>,
    /// Realm roles from `realm_access.roles`.
    pub roles: Vec<String>,
    /// Optional agent-scope restriction claim (future org scoping).
    pub agent_scope: Option<String>,
}

impl Principal {
    /// Converts to the domain [`iaop_agent_service_domain::Actor`] used for audit
    /// `performed_by` fields.
    pub fn actor(&self) -> iaop_agent_service_domain::Actor {
        iaop_agent_service_domain::Actor {
            subject: format!("{}:{}", self.issuer, self.subject),
            actor_id: Uuid::parse_str(&self.subject).ok(),
        }
    }

    /// Stable scope string for idempotency partitioning.
    pub fn idempotency_scope(&self) -> String {
        format!("{}:{}", self.issuer, self.subject)
    }
}

/// Serializable principal claims extracted from a validated JWT.
/// Kept separate from `jsonwebtoken` types so the application layer has
/// no JWT dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    pub sub: String,
    pub iss: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub exp: Option<i64>,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(default)]
    pub realm_access: Option<RealmAccess>,
    #[serde(default)]
    pub agent_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}
