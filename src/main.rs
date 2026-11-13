//! Composition root for Agent Lifecycle.
//!
//! Wires configuration, persistence, domain services, the HTTP router,
//! and the outbox publisher, then serves with graceful shutdown on
//! SIGINT/SIGTERM.

use std::sync::Arc;

use anyhow::Context;
use iaop_agent_service_api::{AppState, ServicePorts, build_router};
use iaop_agent_service_infrastructure::activation::{
    FailClosedActivationAuthz, FixtureActivationAuthz,
};
use iaop_agent_service_infrastructure::agent_store::PgAgentStore;
use iaop_agent_service_infrastructure::config::AppConfig;
use iaop_agent_service_infrastructure::idempotency::PgIdempotencyStore;
use iaop_agent_service_infrastructure::jwt::{DevBypassValidator, JwksTokenValidator};
use iaop_agent_service_infrastructure::outbox;
use iaop_agent_service_infrastructure::persistence;
use iaop_agent_service_infrastructure::transport::LogTransport;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::from_env().context("failed to load configuration")?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    tracing::info!(config = %config.redacted_summary(), "starting service");

    let db = match &config.database_url {
        Some(url) => {
            let pool = persistence::connect_pool(url, config.database_max_connections)
                .await
                .context("failed to connect to database")?;
            persistence::run_migrations(&pool)
                .await
                .context("failed to run migrations")?;
            tracing::info!("database migrations applied");
            Some(pool)
        }
        None => {
            tracing::warn!("DATABASE_URL not set; running without persistence");
            None
        }
    };

    // ---- wire domain services -------------------------------------------
    let services = db.as_ref().map(|_pool| {
        let activation = match config.activation_mode {
            iaop_agent_service_infrastructure::config::ActivationMode::Fixture => {
                iaop_agent_service_infrastructure::activation::ActivationAuthzImpl::Fixture(
                    FixtureActivationAuthz::new(),
                )
            }
            _ => iaop_agent_service_infrastructure::activation::ActivationAuthzImpl::FailClosed(
                FailClosedActivationAuthz::new(),
            ),
        };
        let validator = match config.auth_mode {
            iaop_agent_service_infrastructure::config::AuthMode::DevBypass => {
                iaop_agent_service_infrastructure::jwt::TokenValidatorImpl::Dev(DevBypassValidator)
            }
            _ => iaop_agent_service_infrastructure::jwt::TokenValidatorImpl::Jwks(
                JwksTokenValidator::new(
                    config.oidc_issuer.clone().unwrap_or_default(),
                    config.oidc_jwks_url.clone().unwrap_or_else(|| {
                        format!(
                            "{}/protocol/openid-connect/certs",
                            config.oidc_issuer.clone().unwrap_or_default()
                        )
                    }),
                    config.oidc_audience.clone().unwrap_or_default(),
                ),
            ),
        };
        Arc::new(ServicePorts {
            agent_store: PgAgentStore::new(),
            idempotency: PgIdempotencyStore::new(),
            activation,
            token_validator: validator,
        })
    });

    // ---- outbox publisher (only with DB) --------------------------------
    if let Some(pool) = &db {
        let publisher_pool = pool.clone();
        let transport = Arc::new(LogTransport);
        let poll = config.outbox_poll_interval;
        let batch = config.outbox_batch_size;
        let max_attempts = config.outbox_max_attempts;
        tokio::spawn(async move {
            let shutdown = shutdown_signal();
            outbox::run_publisher(
                publisher_pool,
                transport,
                poll,
                batch,
                max_attempts,
                shutdown,
            )
            .await;
        });
        tracing::info!("outbox publisher started (log transport)");
    }

    let state = AppState {
        config: Arc::new(config.clone()),
        db,
        services,
    };
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
