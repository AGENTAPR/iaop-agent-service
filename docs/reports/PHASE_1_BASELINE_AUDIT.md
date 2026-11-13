# Phase 1 Baseline Audit — iaop-agent-service

Date: 2026-11-13 · Auditor: Devin · Commit audited: `81336ca`
(`feat: scaffold Axum microservice foundation` on `main`)

Environment verified: `rustc/cargo 1.100.0` (stable, pinned via
`rust-toolchain.toml`), workspace `resolver = "3"`, edition 2024.

## Component audit

| Component | Existing status | Findings | Gap | Action | Class |
|---|---|---|---|---|---|
| Rust workspace | PASS | 4-crate workspace (domain/application/infrastructure/api) + bin; lockfile committed; deps pinned | none | keep | PASS |
| Axum | PASS | Router with `/health/live`, `/health/ready`, `/api/v1/system/info`, `/api/docs/openapi.json`; request-id middleware; timeout; graceful shutdown | no business routes | add agent routes | PARTIAL |
| DDD | PARTIAL | Crate layering + `EntityId`/`DomainError` exist; no aggregates or behavior yet | entire domain model missing | implement Agent aggregate + state machine | MISSING |
| Database | PARTIAL | `persistence.rs` pool+migrations+ping; outbox/inbox migrations exist | no business tables; no repositories | add migrations + repos | PARTIAL |
| Security | PARTIAL | `bearer_token` extractor + public-path policy; `is_public_path` test | no JWT validation, no auth extractor, no authorization | implement JWKS validation + permissions | MISSING |
| API contract | PARTIAL | utoipa `ApiDoc` stub; contract repo has agent `openapi.yaml` (health/info only) | no endpoint annotations | annotate + sync contract repo | PARTIAL |
| Outbox | PARTIAL | `outbox_events`/`inbox_events` tables + `OutboxRecord` type + `EventPublisher` port | no writer calls, no publisher loop, no transport | implement writer + dispatcher + log transport | PARTIAL |
| Tests | PARTIAL | 11 unit tests (ids, config, middleware helpers, smoke router test) | no domain/application/integration/concurrency tests | add full test layers | PARTIAL |
| Docker | PASS | Multi-stage Dockerfile (rust:1.100.0-slim → bookworm-slim, non-root, healthcheck); `.dockerignore`; verified `docker build` | none | keep | PASS |
| CI/CD | PASS | `main-ci.yml`: fmt/clippy/test on `push: main` + job-level `if:` guard | no PG-backed CI job yet | extend with services postgres | PARTIAL |
| Messaging | PARTIAL | `IntegrationEvent` envelope + `validate_event` | no real publisher | implement | PARTIAL |
| Idempotency | MISSING | — | no idempotency store/logic | implement table + middleware/handler support | MISSING |
| Observability | PARTIAL | JSON tracing, request/correlation IDs, latency log | no OTel SDK, no metrics | document baseline; metrics deferred | PARTIAL |

## Baseline verdict

Scaffold is correct and preserved. All gaps are additive: domain model,
schema, application use cases, auth, idempotency, outbox publisher, tests.
No existing code needs repair or removal.
