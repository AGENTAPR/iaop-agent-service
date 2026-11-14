//! Transactional outbox publisher.
//!
//! Claims unpublished rows with `FOR UPDATE SKIP LOCKED`, delivers via
//! the configured [`EventTransport`], and marks `published_at` or
//! schedules a retry with exponential backoff + jitter. Delivery is
//! at-least-once — consumers must deduplicate (inbox table).
//!
//! No broker is locked in yet: `LogTransport` is the default. The
//! publisher never holds a business transaction open across network
//! calls — claiming uses a short dedicated transaction per batch.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{PgPool, Postgres, Row, Transaction};
use tracing::{debug, error, info, warn};

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::{EventTransport, IntegrationEvent};

/// One claimed outbox row ready for delivery.
struct ClaimedEvent {
    event_id: uuid::Uuid,
    attempts: i32,
    event: IntegrationEvent,
}

/// Claims up to `batch_size` due events inside `tx`.
async fn claim_batch(
    tx: &mut Transaction<'_, Postgres>,
    batch_size: i64,
) -> Result<Vec<ClaimedEvent>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT event_id, aggregate_id, aggregate_type, aggregate_version, event_type, \
         event_version, payload, occurred_at, source, correlation_id, causation_id, publish_attempts \
         FROM outbox_events \
         WHERE published_at IS NULL \
           AND (next_attempt_at IS NULL OR next_attempt_at <= now()) \
         ORDER BY created_at \
         LIMIT $1 \
         FOR UPDATE SKIP LOCKED",
    )
    .bind(batch_size)
    .fetch_all(&mut **tx)
    .await?;

    rows.iter()
        .map(|r| {
            let payload: serde_json::Value =
                r.try_get("payload").unwrap_or(serde_json::Value::Null);
            let event = IntegrationEvent {
                event_id: r.try_get("event_id")?,
                event_type: r.try_get("event_type")?,
                event_version: r.try_get::<i32, _>("event_version")? as u32,
                source: r
                    .try_get::<Option<String>, _>("source")?
                    .unwrap_or_default(),
                aggregate_id: r.try_get("aggregate_id")?,
                aggregate_version: r
                    .try_get::<Option<i64>, _>("aggregate_version")?
                    .map(|v| v as u64),
                occurred_at: r.try_get("occurred_at")?,
                correlation_id: r.try_get("correlation_id")?,
                causation_id: r.try_get("causation_id")?,
                payload,
            };
            Ok(ClaimedEvent {
                event_id: event.event_id,
                attempts: r.try_get::<i32, _>("publish_attempts")?,
                event,
            })
        })
        .collect()
}

/// Marks an event published.
async fn mark_published(
    tx: &mut Transaction<'_, Postgres>,
    event_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE outbox_events SET published_at=now() WHERE event_id=$1")
        .bind(event_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Schedules a retry with exponential backoff + jitter, or leaves the
/// row unpublished after `max_attempts` (terminal failure — visible via
/// backlog metrics, never silently dropped).
async fn mark_retry(
    tx: &mut Transaction<'_, Postgres>,
    event_id: uuid::Uuid,
    attempts: i32,
    max_attempts: i32,
    error: &str,
) -> Result<(), sqlx::Error> {
    if attempts >= max_attempts {
        // Terminal: stop scheduling; row stays unpublished for operators.
        sqlx::query(
            "UPDATE outbox_events SET last_error=$2, publish_attempts=$3, \
             next_attempt_at = now() + interval '100 years' WHERE event_id=$1",
        )
        .bind(event_id)
        .bind(error)
        .bind(attempts)
        .execute(&mut **tx)
        .await?;
        return Ok(());
    }
    // backoff = 2^attempts seconds, capped at 5min, + up to 1s jitter
    let base = 2u64.saturating_pow(attempts.max(0) as u32).min(300);
    let jitter_ms = (uuid::Uuid::now_v7().as_u128() % 1000) as u64;
    let delay = Duration::from_secs(base) + Duration::from_millis(jitter_ms);
    sqlx::query(
        "UPDATE outbox_events SET last_error=$2, publish_attempts=$3, next_attempt_at=$4 \
         WHERE event_id=$1",
    )
    .bind(event_id)
    .bind(error)
    .bind(attempts)
    .bind(chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Runs the publisher loop until `shutdown` resolves. Each iteration:
/// claim a batch in a short tx, deliver outside any tx, then record
/// outcomes in a second short tx.
pub async fn run_publisher<T: EventTransport + Send + Sync + 'static>(
    pool: PgPool,
    transport: Arc<T>,
    poll_interval: Duration,
    batch_size: i64,
    max_attempts: i32,
    shutdown: impl std::future::Future<Output = ()>,
) {
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("outbox publisher shutting down");
                return;
            }
            _ = tokio::time::sleep(poll_interval) => {}
        }

        // 1) Claim batch in a short transaction.
        let claimed = {
            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    error!(error = %e, "outbox claim failed to begin tx");
                    continue;
                }
            };
            match claim_batch(&mut tx, batch_size).await {
                Ok(events) => {
                    // Commit releases row locks; delivery happens after.
                    if let Err(e) = tx.commit().await {
                        error!(error = %e, "outbox claim commit failed");
                        continue;
                    }
                    events
                }
                Err(e) => {
                    error!(error = %e, "outbox claim failed");
                    let _ = tx.rollback().await;
                    continue;
                }
            }
        };

        if claimed.is_empty() {
            debug!("outbox empty");
            continue;
        }

        // 2) Deliver each event outside any DB transaction.
        for c in claimed {
            match transport.deliver(&c.event).await {
                Ok(()) => {
                    let mut tx = match pool.begin().await {
                        Ok(t) => t,
                        Err(e) => {
                            error!(error = %e, "outbox mark-published begin failed");
                            continue;
                        }
                    };
                    if let Err(e) = mark_published(&mut tx, c.event_id).await {
                        error!(error = %e, event_id = %c.event_id, "mark published failed");
                    } else if let Err(e) = tx.commit().await {
                        error!(error = %e, "mark published commit failed");
                    } else {
                        info!(event_id = %c.event_id, event_type = %c.event.event_type, "outbox event delivered");
                    }
                }
                Err(err) => {
                    warn!(event_id = %c.event_id, error = %err, "outbox delivery failed; scheduling retry");
                    let mut tx = match pool.begin().await {
                        Ok(t) => t,
                        Err(e) => {
                            error!(error = %e, "outbox mark-retry begin failed");
                            continue;
                        }
                    };
                    let attempts = c.attempts + 1;
                    if let Err(e) =
                        mark_retry(&mut tx, c.event_id, attempts, max_attempts, &err).await
                    {
                        error!(error = %e, "mark retry failed");
                    } else if let Err(e) = tx.commit().await {
                        error!(error = %e, "mark retry commit failed");
                    }
                }
            }
        }
    }
}

/// Outbox backlog depth for metrics/health reporting.
pub async fn backlog_depth(pool: &PgPool) -> Result<i64, AppError> {
    let row = sqlx::query("SELECT count(*) AS n FROM outbox_events WHERE published_at IS NULL")
        .fetch_one(pool)
        .await
        .map_err(AppError::infra)?;
    row.try_get::<i64, _>("n").map_err(AppError::infra)
}
