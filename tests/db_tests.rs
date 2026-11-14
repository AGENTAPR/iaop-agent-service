//! Real-PostgreSQL integration tests for Agent Service against local-postgresql16.
//!
//! Covers: migrations applied, agent registration, outbox leasing & fencing,
//! poison message terminal handling, and lifecycle history auditing.

use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://postgres:340ce6f29651795482a97e89a2dd36b6f9336f60ef2ed6c4@127.0.0.1:5432/iaop_agent".into()
    });
    let pool = PgPool::connect(&url)
        .await
        .expect("failed to connect to iaop_agent database");
    iaop_agent_service_infrastructure::persistence::run_migrations(&pool)
        .await
        .expect("failed to run migrations");
    pool
}

#[tokio::test]
async fn migrations_and_agent_crud_in_real_database() {
    let pool = pool().await;
    let agent_id = Uuid::now_v7();
    let person_id = Uuid::now_v7();
    let code = format!("AG-{}", &Uuid::now_v7().simple().to_string()[20..28]);

    // Insert new agent
    sqlx::query(
        r#"
        INSERT INTO agents (id, person_id, agent_code, lifecycle_status, version)
        VALUES ($1, $2, $3, 'DRAFT', 1)
        "#,
    )
    .bind(agent_id)
    .bind(person_id)
    .bind(&code)
    .execute(&pool)
    .await
    .unwrap();

    // Query agent by code
    let (fetched_id, status, ver): (Uuid, String, i64) =
        sqlx::query_as("SELECT id, lifecycle_status, version FROM agents WHERE agent_code = $1")
            .bind(&code)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(fetched_id, agent_id);
    assert_eq!(status, "DRAFT");
    assert_eq!(ver, 1);

    // Record lifecycle history
    let hist_id = Uuid::now_v7();
    let cmd_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_lifecycle_history
            (id, agent_id, from_status, to_status, reason_code, reason_text, effective_at, performed_by, command_id, aggregate_version)
        VALUES ($1, $2, 'DRAFT', 'PENDING_ACTIVATION', 'SUBMIT_ACTIVATION', 'Submitted by agent', now(), 'test-user', $3, 2)
        "#,
    )
    .bind(hist_id)
    .bind(agent_id)
    .bind(cmd_id)
    .execute(&pool)
    .await
    .unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_lifecycle_history WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn outbox_lease_claiming_fenced_completion_and_poison_handling() {
    use iaop_agent_service_infrastructure::outbox::{claim_batch, mark_published, mark_retry};

    let pool = pool().await;
    let event_id = Uuid::now_v7();

    // Clean any prior leftover test events
    let _ = sqlx::query("DELETE FROM outbox_events WHERE event_type = 'agent.registered.v1'")
        .execute(&pool)
        .await;

    // Insert an unpublished test event
    sqlx::query(
        r#"
        INSERT INTO outbox_events (event_id, aggregate_id, event_type, event_version, payload, occurred_at)
        VALUES ($1, $1, 'agent.registered.v1', 1, '{"test": true}', now())
        "#,
    )
    .bind(event_id)
    .execute(&pool)
    .await
    .unwrap();

    let instance_a = "agent-worker-a";
    let instance_b = "agent-worker-b";

    // 1. Worker A claims the batch
    let mut tx_a1 = pool.begin().await.unwrap();
    let batch = claim_batch(&mut tx_a1, instance_a, 30, 1000).await.unwrap();
    tx_a1.commit().await.unwrap();
    assert!(batch.iter().any(|r| r.event_id == event_id));

    // 2. Fencing: Worker B attempting to complete Worker A's lease must fail
    let mut tx_b = pool.begin().await.unwrap();
    let published_by_b = mark_published(&mut tx_b, event_id, instance_b)
        .await
        .unwrap();
    tx_b.commit().await.unwrap();
    assert!(
        !published_by_b,
        "Worker B must not complete Worker A's lease"
    );

    // 3. Worker A completes successfully
    let mut tx_a2 = pool.begin().await.unwrap();
    let published_by_a = mark_published(&mut tx_a2, event_id, instance_a)
        .await
        .unwrap();
    tx_a2.commit().await.unwrap();
    assert!(published_by_a, "Worker A holding lease must succeed");

    // Verify event is now published
    let is_published: bool = sqlx::query_scalar(
        "SELECT published_at IS NOT NULL FROM outbox_events WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(is_published);

    // 4. Poison handling: insert event with 4 previous attempts
    let poison_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO outbox_events (event_id, aggregate_id, event_type, event_version, payload, occurred_at, publish_attempts)
        VALUES ($1, $1, 'agent.poison.v1', 1, '{"poison": true}', now(), 4)
        "#,
    )
    .bind(poison_id)
    .execute(&pool)
    .await
    .unwrap();

    // Claim poison event
    let mut tx_p1 = pool.begin().await.unwrap();
    let poison_batch = claim_batch(&mut tx_p1, instance_a, 30, 1000).await.unwrap();
    tx_p1.commit().await.unwrap();
    assert!(poison_batch.iter().any(|r| r.event_id == poison_id));

    // Mark retry (5th attempt hits max_attempts = 5 -> terminal)
    let mut tx_p2 = pool.begin().await.unwrap();
    let retried = mark_retry(
        &mut tx_p2,
        poison_id,
        instance_a,
        5,
        5,
        "permanent Kafka serialization failure",
    )
    .await
    .unwrap();
    tx_p2.commit().await.unwrap();
    assert!(retried);

    // Verify is_terminal is true and next_attempt_at is NULL
    let (is_term, next_attempt): (bool, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT is_terminal, next_attempt_at FROM outbox_events WHERE event_id = $1",
    )
    .bind(poison_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(is_term, "Poison event must be marked terminal");
    assert!(
        next_attempt.is_none(),
        "Terminal poison event must not have next_attempt_at set"
    );

    // Terminal event must not be claimed by subsequent claim_batch
    let mut tx_p3 = pool.begin().await.unwrap();
    let fresh_batch = claim_batch(&mut tx_p3, instance_b, 30, 1000).await.unwrap();
    tx_p3.commit().await.unwrap();
    assert!(
        !fresh_batch.iter().any(|r| r.event_id == poison_id),
        "Terminal events must never be re-leased"
    );
}
