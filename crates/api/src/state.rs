use std::sync::Arc;

use iaop_agent_service_application::ports::{
    ActivationAuthorizationPort, AgentStore, IdempotencyStore, TokenValidator,
};
use iaop_agent_service_infrastructure::activation::ActivationAuthzImpl;
use iaop_agent_service_infrastructure::agent_store::PgAgentStore;
use iaop_agent_service_infrastructure::config::AppConfig;
use iaop_agent_service_infrastructure::idempotency::PgIdempotencyStore;
use iaop_agent_service_infrastructure::jwt::TokenValidatorImpl;
use sqlx::PgPool;

/// Shared application state injected into handlers via `State`.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    /// `None` when DATABASE_URL is not configured; `/health/ready`
    /// reports accordingly instead of failing.
    pub db: Option<PgPool>,
    /// Wired service ports. `None` when no database is configured —
    /// agent endpoints return 503 in that case.
    pub services: Option<Arc<ServicePorts>>,
}

/// Concrete port implementations wired at startup. Traits are object-
/// safe via concrete enum/struct types (RPITIT ports are not dyn-safe).
pub struct ServicePorts {
    pub agent_store: PgAgentStore,
    pub idempotency: PgIdempotencyStore,
    pub activation: ActivationAuthzImpl,
    pub token_validator: TokenValidatorImpl,
}

// Trait bounds kept for documentation; ports are used via concrete types.
#[allow(dead_code)]
fn _assert_impls(p: &ServicePorts) {
    fn _is_agent_store<T: AgentStore>(_: &T) {}
    fn _is_idem<T: IdempotencyStore>(_: &T) {}
    fn _is_act<T: ActivationAuthorizationPort>(_: &T) {}
    fn _is_tok<T: TokenValidator>(_: &T) {}
    _is_agent_store(&p.agent_store);
    _is_idem(&p.idempotency);
    _is_act(&p.activation);
    _is_tok(&p.token_validator);
}
