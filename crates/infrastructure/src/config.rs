//! Typed application configuration loaded from environment variables.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    MissingVar(&'static str),
    #[error("invalid value for {0}: {1}")]
    InvalidValue(&'static str, String),
    #[error("unsafe configuration: {0}")]
    UnsafeConfig(String),
}

/// Authentication mode. `DevBypass` is refused in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Validate Bearer JWTs against OIDC issuer JWKS.
    Jwt,
    /// Fixed development principal — LOCAL/DEV ONLY, refused in production.
    DevBypass,
}

/// Activation authorization adapter selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationMode {
    /// Fail-closed: every authorization request returns INDETERMINATE.
    FailClosed,
    /// Test fixture returning ALLOW — refused in production.
    Fixture,
    /// Real HTTP eligibility validation calling external eligibility service.
    Http,
}

/// Outbox event transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTransportKind {
    /// Logs events instead of broker delivery (local / dev).
    Log,
    /// Real Kafka event transport publishing to topics.
    Kafka,
}

/// Service configuration. Secrets (e.g. DATABASE_URL) are never logged;
/// use [`AppConfig::redacted_summary`] for safe diagnostics.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
    pub bind_addr: String,
    /// Optional: services without persistence leave this unset and
    /// `/health/ready` does not probe a database.
    pub database_url: Option<String>,
    pub database_max_connections: u32,
    pub request_timeout: Duration,

    // -- authn/authz ---------------------------------------------------
    pub auth_mode: AuthMode,
    pub oidc_issuer: Option<String>,
    pub oidc_jwks_url: Option<String>,
    pub oidc_audience: Option<String>,

    // -- activation authorization ---------------------------------------
    pub activation_mode: ActivationMode,
    pub eligibility_service_url: Option<String>,

    // -- events ---------------------------------------------------------
    pub event_transport: EventTransportKind,
    pub kafka_brokers: Option<String>,
    pub kafka_topic_prefix: Option<String>,
    pub outbox_poll_interval: Duration,
    pub outbox_batch_size: i64,
    pub outbox_max_attempts: i32,

    // -- idempotency ----------------------------------------------------
    pub idempotency_ttl: Duration,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let environment = env_or("ENVIRONMENT", "local");

        let auth_mode = match env_or("AUTH_MODE", "jwt").as_str() {
            "jwt" => AuthMode::Jwt,
            "dev" => AuthMode::DevBypass,
            other => {
                return Err(ConfigError::InvalidValue(
                    "AUTH_MODE",
                    format!("unknown mode '{other}'"),
                ));
            }
        };

        if auth_mode == AuthMode::Jwt {
            for var in ["OIDC_ISSUER", "OIDC_JWKS_URL", "OIDC_AUDIENCE"] {
                if std::env::var(var).ok().filter(|s| !s.is_empty()).is_none() {
                    return Err(ConfigError::MissingVar(match var {
                        "OIDC_ISSUER" => "OIDC_ISSUER",
                        "OIDC_JWKS_URL" => "OIDC_JWKS_URL",
                        _ => "OIDC_AUDIENCE",
                    }));
                }
            }
        }

        let activation_mode = match env_or("ACTIVATION_MODE", "fail_closed").as_str() {
            "fail_closed" => ActivationMode::FailClosed,
            "fixture" => ActivationMode::Fixture,
            "http" => ActivationMode::Http,
            other => {
                return Err(ConfigError::InvalidValue(
                    "ACTIVATION_MODE",
                    format!("unknown mode '{other}'"),
                ));
            }
        };
        validate_mode_safety(&environment, auth_mode, activation_mode)?;

        let event_transport = match env_or("EVENT_TRANSPORT", "log").as_str() {
            "log" => EventTransportKind::Log,
            "kafka" => EventTransportKind::Kafka,
            other => {
                return Err(ConfigError::InvalidValue(
                    "EVENT_TRANSPORT",
                    format!("unknown transport '{other}'"),
                ));
            }
        };

        Ok(Self {
            service_name: env_or("SERVICE_NAME", "iaop-agent-service"),
            service_version: env_or("SERVICE_VERSION", env!("CARGO_PKG_VERSION")),
            environment,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            database_url: std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty()),
            database_max_connections: env_parse("DATABASE_MAX_CONNECTIONS", 10)?,
            request_timeout: Duration::from_secs(env_parse("REQUEST_TIMEOUT_SECS", 30)?),
            auth_mode,
            oidc_issuer: std::env::var("OIDC_ISSUER").ok().filter(|s| !s.is_empty()),
            oidc_jwks_url: std::env::var("OIDC_JWKS_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_audience: std::env::var("OIDC_AUDIENCE")
                .ok()
                .filter(|s| !s.is_empty()),
            activation_mode,
            eligibility_service_url: std::env::var("ELIGIBILITY_SERVICE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            event_transport,
            kafka_brokers: std::env::var("KAFKA_BROKERS")
                .ok()
                .filter(|s| !s.is_empty()),
            kafka_topic_prefix: std::env::var("KAFKA_TOPIC_PREFIX")
                .ok()
                .filter(|s| !s.is_empty()),
            outbox_poll_interval: Duration::from_millis(env_parse("OUTBOX_POLL_MS", 1000)?),
            outbox_batch_size: env_parse("OUTBOX_BATCH_SIZE", 50)?,
            outbox_max_attempts: env_parse("OUTBOX_MAX_ATTEMPTS", 10)?,
            idempotency_ttl: Duration::from_secs(env_parse("IDEMPOTENCY_TTL_SECS", 86400)?),
        })
    }

    /// Diagnostics-safe summary: never includes secrets.
    pub fn redacted_summary(&self) -> String {
        format!(
            "service={} version={} env={} bind={} db={} auth={:?} activation={:?} timeout={}s",
            self.service_name,
            self.service_version,
            self.environment,
            self.bind_addr,
            if self.database_url.is_some() {
                "configured"
            } else {
                "disabled"
            },
            self.auth_mode,
            self.activation_mode,
            self.request_timeout.as_secs(),
        )
    }
}

/// Unsafe-mode guard: `AUTH_MODE=dev` and `ACTIVATION_MODE=fixture` are
/// test-only adapters permitted exclusively under `ENVIRONMENT=local`.
/// Shared DEV/UAT/production must always run JWT auth and a real
/// (fail-closed or HTTP) activation authorization path.
fn validate_mode_safety(
    environment: &str,
    auth_mode: AuthMode,
    activation_mode: ActivationMode,
) -> Result<(), ConfigError> {
    let isolated = environment.eq_ignore_ascii_case("local");
    if auth_mode == AuthMode::DevBypass && !isolated {
        return Err(ConfigError::UnsafeConfig(format!(
            "AUTH_MODE=dev is forbidden when ENVIRONMENT={environment} (allowed only for ENVIRONMENT=local)"
        )));
    }
    if activation_mode == ActivationMode::Fixture && !isolated {
        return Err(ConfigError::UnsafeConfig(format!(
            "ACTIVATION_MODE=fixture is forbidden when ENVIRONMENT={environment} (allowed only for ENVIRONMENT=local)"
        )));
    }
    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T: std::str::FromStr>(key: &'static str, default: T) -> Result<T, ConfigError> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|_| ConfigError::InvalidValue(key, v)),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_modes_rejected_outside_local() {
        for env in ["development", "uat", "production", "staging"] {
            assert!(
                validate_mode_safety(env, AuthMode::DevBypass, ActivationMode::FailClosed).is_err(),
                "AUTH_MODE=dev must be rejected for ENVIRONMENT={env}"
            );
            assert!(
                validate_mode_safety(env, AuthMode::Jwt, ActivationMode::Fixture).is_err(),
                "ACTIVATION_MODE=fixture must be rejected for ENVIRONMENT={env}"
            );
        }
    }

    #[test]
    fn unsafe_modes_allowed_only_in_local() {
        assert!(
            validate_mode_safety("local", AuthMode::DevBypass, ActivationMode::Fixture).is_ok()
        );
        assert!(
            validate_mode_safety("LOCAL", AuthMode::DevBypass, ActivationMode::FailClosed).is_ok()
        );
    }

    #[test]
    fn safe_modes_allowed_everywhere() {
        for env in ["local", "development", "uat", "production"] {
            assert!(validate_mode_safety(env, AuthMode::Jwt, ActivationMode::FailClosed).is_ok());
            assert!(validate_mode_safety(env, AuthMode::Jwt, ActivationMode::Http).is_ok());
        }
    }
}
