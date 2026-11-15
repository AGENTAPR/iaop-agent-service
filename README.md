# iaop-agent-service

Agent registry, lifecycle, activation, suspension, and termination.

## Purpose

iaop-agent-service is part of the **Insurance Agent Operating Platform (IAOP)**, an enterprise
insurance agent lifecycle platform. It owns the **Agent Lifecycle** bounded context
(or platform capability) within the IAOP microservices landscape.

## Bounded Context / Ownership

- **Bounded context:** Agent Lifecycle
- **Owner:** AGENTAPR
- **Category:** microservice

## Technical Stack

Rust, Axum, PostgreSQL

## Architecture Principles

- Domain-Driven Design (DDD) with explicit bounded contexts
- Hexagonal architecture (ports and adapters)
- Event-driven integration via versioned contracts
- PostgreSQL as the system of record where applicable
- Identity and access via Keycloak
- Workflow orchestration via Flowable BPM where applicable

## Responsibilities

- Agent registry and master data
- Agent lifecycle state management
- Activation, suspension, and termination
- Agent profile and status APIs

## Non-Responsibilities

- Recruitment and onboarding
- Agency hierarchy management
- Learning and certification

## Related Repositories

- [iaop-architecture](https://github.com/AGENTAPR/iaop-architecture) - DDD strategic design, bounded context maps, ADRs
- [iaop-api-contracts](https://github.com/AGENTAPR/iaop-api-contracts) - Versioned OpenAPI contracts
- [iaop-event-contracts](https://github.com/AGENTAPR/iaop-event-contracts) - Versioned integration events
- [iaop-workflow-service](https://github.com/AGENTAPR/iaop-workflow-service) - Flowable BPM orchestration
- [iaop-local-dev](https://github.com/AGENTAPR/iaop-local-dev) - Local development environment

## Status

**IMPLEMENTED — DEV VERIFIED (as of 2026-11-15, `development`)**

The Agent Lifecycle bounded context is implemented and deployed on InstaBox DEV:

- Agent registry: create/list/get by ID and agent code, cursor pagination,
  duplicate-code prevention, optimistic concurrency (`version`), audit history.
- Agent lifecycle state machine: `DRAFT → PENDING_ACTIVATION → ACTIVE`,
  plus `SUSPENDED → ACTIVE (reinstate)` and terminal `TERMINATED`, with
  invalid-transition rejection and stale-version (`412`) protection.
- Activation authorization via pluggable adapters (`fail_closed` default,
  `http` eligibility integration, `fixture` test-only).
- Agent appointments aggregate with effective dates and status transitions.
- Identity links: link/unlink/resolve external IdP `(issuer, subject)` →
  agent, unique active mapping, revocation support. Keycloak `sub` is
  never parsed as an agent ID.
- Transactional outbox + Kafka transport (`EVENT_TRANSPORT=kafka` in DEV),
  lease/fencing, retry, poison-message terminal handling.
- Idempotency records for mutating operations.
- JWT authn via Keycloak OIDC (`AUTH_MODE=jwt` in DEV); `AUTH_MODE=dev`
  and `ACTIVATION_MODE=fixture` are restricted to `ENVIRONMENT=local`.
- OpenAPI 3.1 via utoipa at `/api/docs/openapi.json`.

Verification evidence: `tests/db_tests.rs` (real PostgreSQL 16: migrations,
CRUD, outbox leasing/fencing/poison) and `tests/integration` smoke tests.
See `AGENA_AGENT_SERVICE_COMPLETION_REPORT.md` for the full evidence matrix.

## Development Conventions

- Default branch: `main`
- Development branch: `development` — all changes are committed and pushed here
- Squash merges preferred; merge commits and rebases disabled
- Pull requests required before merging into `main`
- Topics: iaop, insurance, insurance-agents, enterprise-architecture, rust, axum, microservices, domain-driven-design, postgresql
## Foundation implementation (2026-11)

Rust/Axum microservice scaffold generated from the canonical IAOP
template (see `iaop-rust-common/templates/axum-microservice` and
`iaop-architecture/docs/standards/IAOP_AXUM_SERVICE_TEMPLATE.md`):

- Crates: `domain`, `application`, `infrastructure`, `api` (+ `src/main.rs`)
- Endpoints: `/health/live`, `/health/ready`, `/api/v1/system/info`, `/openapi.json`, `/docs`
- Persistence: PostgreSQL/SQLx boundary + outbox/inbox migrations
- Security: OIDC/JWT boundary via `iaop-security` (Keycloak)
- Observability: structured JSON logs, correlation IDs, OTLP-ready
- CI: `.github/workflows/main-ci.yml` — fmt/clippy/test on `main` pushes only
- Docs: `docs/architecture.md`, `docs/database.md`, `docs/api.md`

Business features are intentionally not implemented yet.
