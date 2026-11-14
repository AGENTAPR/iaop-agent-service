//! SQLx implementation of the durable idempotency store.
//!
//! Reserve uses `INSERT ... ON CONFLICT DO NOTHING`: a concurrent
//! identical request blocks on the unique index until the first
//! transaction commits, then observes the completed record and replays.
//! All state changes happen inside the caller's business transaction —
//! rollback leaves no record, so retries are always safe.

use std::time::Duration;

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::{IdempotencyReserve, IdempotencyStore};

pub struct PgIdempotencyStore;

impl PgIdempotencyStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PgIdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IdempotencyStore for PgIdempotencyStore {
    async fn reserve(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal_scope: &str,
        operation: &str,
        key: &str,
        fingerprint: &str,
        ttl: Duration,
    ) -> Result<IdempotencyReserve, AppError> {
        let record_id = Uuid::now_v7();
        let expires = chrono::Utc::now() + chrono::Duration::from_std(ttl).unwrap_or_default();

        let inserted = sqlx::query(
            "INSERT INTO idempotency_records \
             (id, idempotency_key, principal_scope, operation, request_fingerprint, state, expires_at) \
             VALUES ($1,$2,$3,$4,$5,'IN_PROGRESS',$6) \
             ON CONFLICT (principal_scope, operation, idempotency_key) DO NOTHING \
             RETURNING id",
        )
        .bind(record_id)
        .bind(key)
        .bind(principal_scope)
        .bind(operation)
        .bind(fingerprint)
        .bind(expires)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::infra)?;

        if inserted.is_some() {
            return Ok(IdempotencyReserve::Acquired { record_id });
        }

        // Conflict path: an existing record won the race (or pre-existed).
        let row = sqlx::query(
            "SELECT request_fingerprint, state, response_status, response_body \
             FROM idempotency_records \
             WHERE principal_scope=$1 AND operation=$2 AND idempotency_key=$3",
        )
        .bind(principal_scope)
        .bind(operation)
        .bind(key)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::infra)?;

        let existing_fp: String = row
            .try_get("request_fingerprint")
            .map_err(AppError::infra)?;
        if existing_fp != fingerprint {
            return Ok(IdempotencyReserve::Conflict);
        }
        let state: String = row.try_get("state").map_err(AppError::infra)?;
        match state.as_str() {
            "COMPLETED" => {
                let status: i32 = row.try_get("response_status").map_err(AppError::infra)?;
                let body: serde_json::Value =
                    row.try_get("response_body").map_err(AppError::infra)?;
                Ok(IdempotencyReserve::Replay { status, body })
            }
            // IN_PROGRESS: identical request still executing — caller
            // must retry later; treat as conflict for this attempt.
            _ => Ok(IdempotencyReserve::Conflict),
        }
    }

    async fn complete(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record_id: Uuid,
        status: i32,
        body: serde_json::Value,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE idempotency_records \
             SET state='COMPLETED', response_status=$2, response_body=$3, updated_at=now() \
             WHERE id=$1",
        )
        .bind(record_id)
        .bind(status)
        .bind(&body)
        .execute(&mut **tx)
        .await
        .map_err(AppError::infra)?;
        Ok(())
    }
}

/// Best-effort cleanup of expired idempotency records. Called by the
/// outbox maintenance loop; not part of request path.
pub async fn purge_expired(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM idempotency_records WHERE expires_at < now()")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
