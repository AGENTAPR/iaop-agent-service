//! Transactional outbox publisher with multi-instance lease locking and fencing.
//!
//! Claims unpublished rows using atomic `UPDATE ... RETURNING` with lease timestamps
//! and `FOR UPDATE SKIP LOCKED`. Delivers via the configured [`EventTransport`], and
//! marks `published_at` with strict lease fencing, or schedules a retry with exponential
//! backoff + jitter. Terminal poison messages are explicitly flagged (`is_terminal = true`).
//! Delivery is at-least-once — consumers must deduplicate (inbox table).
//!
//! The publisher never holds a business transaction open across network calls —
//! claiming uses a short dedicated transaction per batch, holding a durable lease
//! (`locked_until` / `locked_by`) so other instances cannot claim the same events
//! while network delivery is in flight.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{PgPool, Postgres, Row, Transaction};
use tracing::{debug, error, info, warn};

use iaop_agent_service_application::ports::{EventTransport, IntegrationEvent};

/// One claimed outbox row ready for delivery.
pub struct ClaimedEvent {
    pub event_id: uuid::Uuid,
    pub attempts: i32,
    pub event: IntegrationEvent,
}

/// Claims up to `batch_size` due events inside `tx` with an atomic lease.
pub async fn claim_batch(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: &str,
    lease_duration_secs: i64,
    batch_size: i64,
) -> Result<Vec<ClaimedEvent>, sqlx::Error> {
    let lease_interval = format!("{} seconds", lease_duration_secs);
    let sql = format!(
        "UPDATE outbox_events \
         SET locked_until = now() + interval '{lease}', \
             locked_by = $1, \
             publish_attempts = publish_attempts + 1 \
         WHERE event_id IN ( \
             SELECT event_id \
             FROM outbox_events \
             WHERE published_at IS NULL \
               AND (is_terminal = FALSE) \
               AND (locked_until IS NULL OR locked_until < now()) \
               AND (next_attempt_at IS NULL OR next_attempt_at <= now()) \
             ORDER BY created_at \
             LIMIT $2 \
             FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING event_id, aggregate_id, aggregate_type, aggregate_version, event_type, \
                   event_version, payload, occurred_at, source, correlation_id, causation_id, publish_attempts",
        lease = lease_interval
    );

    let rows = sqlx::query(&sql)
        .bind(instance_id)
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
                aggregate_id: r
                    .try_get::<Option<uuid::Uuid>, _>("aggregate_id")?
                    .unwrap_or(r.try_get("event_id")?),
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

/// Marks an event published and clears the lease with strict fencing.
pub async fn mark_published(
    tx: &mut Transaction<'_, Postgres>,
    event_id: uuid::Uuid,
    instance_id: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE outbox_events \
         SET published_at = now(), locked_until = NULL, locked_by = NULL \
         WHERE event_id = $1 AND locked_by = $2",
    )
    .bind(event_id)
    .bind(instance_id)
    .execute(&mut **tx)
    .await?;

    let fenced = result.rows_affected() > 0;
    if !fenced {
        warn!(
            event_id = %event_id,
            instance_id = %instance_id,
            "outbox lease fencing: lease was lost or expired before completion"
        );
    }
    Ok(fenced)
}

/// Schedules a retry with exponential backoff + jitter and releases lease,
/// or marks terminal failure after `max_attempts` with explicit `is_terminal = true`.
pub async fn mark_retry(
    tx: &mut Transaction<'_, Postgres>,
    event_id: uuid::Uuid,
    instance_id: &str,
    attempts: i32,
    max_attempts: i32,
    error: &str,
) -> Result<bool, sqlx::Error> {
    if attempts >= max_attempts {
        // Explicit terminal poison event: no 100-year synthetic hack; requires operator replay.
        let result = sqlx::query(
            "UPDATE outbox_events \
             SET last_error = $2, publish_attempts = $3, \
                 is_terminal = TRUE, next_attempt_at = NULL, \
                 locked_until = NULL, locked_by = NULL \
             WHERE event_id = $1 AND locked_by = $4",
        )
        .bind(event_id)
        .bind(error)
        .bind(attempts)
        .bind(instance_id)
        .execute(&mut **tx)
        .await?;

        warn!(
            event_id = %event_id,
            attempts = attempts,
            error = %error,
            "outbox event reached max_attempts; marked as terminal poison event"
        );
        return Ok(result.rows_affected() > 0);
    }

    // backoff = 2^attempts seconds, capped at 5min, + up to 1s jitter
    let base = 2u64.saturating_pow(attempts.max(0) as u32).min(300);
    let jitter_ms = (uuid::Uuid::now_v7().as_u128() % 1000) as u64;
    let delay = Duration::from_secs(base) + Duration::from_millis(jitter_ms);
    let result = sqlx::query(
        "UPDATE outbox_events \
         SET last_error = $2, publish_attempts = $3, next_attempt_at = $4, \
             locked_until = NULL, locked_by = NULL \
         WHERE event_id = $1 AND locked_by = $5",
    )
    .bind(event_id)
    .bind(error)
    .bind(attempts)
    .bind(chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default())
    .bind(instance_id)
    .execute(&mut **tx)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Runs the publisher loop until `shutdown` resolves.
pub async fn run_publisher<T: EventTransport + Send + Sync + 'static>(
    pool: PgPool,
    transport: Arc<T>,
    poll_interval: Duration,
    batch_size: i64,
    max_attempts: i32,
    shutdown: impl std::future::Future<Output = ()>,
) {
    let instance_id = format!("agent-pub-{}", uuid::Uuid::now_v7());
    let lease_duration_secs = 30i64;
    let mut shutdown = std::pin::pin!(shutdown);

    info!(instance_id = %instance_id, "outbox publisher started with atomic leasing");

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("outbox publisher shutting down");
                return;
            }
            _ = tokio::time::sleep(poll_interval) => {}
        }

        // 1) Claim batch with durable atomic lease.
        let claimed = {
            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    error!(error = %e, "outbox claim failed to begin tx");
                    continue;
                }
            };
            match claim_batch(&mut tx, &instance_id, lease_duration_secs, batch_size).await {
                Ok(events) => {
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
        for item in claimed {
            let res = transport.deliver(&item.event).await;

            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    error!(error = %e, event_id = %item.event_id, "outbox outcome begin tx failed");
                    continue;
                }
            };

            match res {
                Ok(()) => {
                    match mark_published(&mut tx, item.event_id, &instance_id).await {
                        Ok(true) => {
                            if let Err(e) = tx.commit().await {
                                error!(error = %e, event_id = %item.event_id, "mark_published commit failed");
                            } else {
                                info!(
                                    event_id = %item.event_id,
                                    event_type = %item.event.event_type,
                                    "outbox event successfully delivered and marked published"
                                );
                            }
                        }
                        Ok(false) => {
                            // Lease lost/expired
                            let _ = tx.rollback().await;
                        }
                        Err(e) => {
                            error!(error = %e, event_id = %item.event_id, "mark_published query failed");
                            let _ = tx.rollback().await;
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        event_id = %item.event_id,
                        error = %err,
                        attempts = item.attempts,
                        "outbox delivery failed; scheduling retry"
                    );
                    match mark_retry(
                        &mut tx,
                        item.event_id,
                        &instance_id,
                        item.attempts,
                        max_attempts,
                        &err,
                    )
                    .await
                    {
                        Ok(_) => {
                            if let Err(e) = tx.commit().await {
                                error!(error = %e, event_id = %item.event_id, "mark_retry commit failed");
                            }
                        }
                        Err(e) => {
                            error!(error = %e, event_id = %item.event_id, "mark_retry query failed");
                            let _ = tx.rollback().await;
                        }
                    }
                }
            }
        }
    }
}
