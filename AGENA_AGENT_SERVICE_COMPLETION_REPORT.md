# AGENA — Agent Service Completion Report

Date: 2026-11-15 · Environment: InstaBox DEV (`iaop-keycloak-dev`) + isolated local PostgreSQL 16

## A. Source baseline

| Repo | Branch | SHA |
|---|---|---|
| iaop-agent-service | development | `f40f0240` (PR #17 unsafe-mode guard) |
| iaop-agency-service | development | `17a33c7d` (PR #25 fixture guard) |
| iaop-agent-bff | development | `fe001934` |
| iaop-api-contracts | development | `329fc1dd` |
| iaop-event-contracts | development | `3e2b3631` (PR #7 schema parity) |

## B. Feature coverage

| Capability | Status | Evidence |
|---|---|---|
| Agent registry CRUD + pagination | COMPLETE | db_tests CRUD; live DEV `/api/v1/agents` |
| Lifecycle state machine (submit/activate/suspend/reinstate/terminate) | IMPLEMENTED_UNVERIFIED (command-level PG lifecycle tests pending) | domain unit tests; state machine in `domain/src/agent.rs` |
| Activation/eligibility adapters | IMPLEMENTED_UNVERIFIED | `infrastructure/src/activation.rs`; DEV runs `fail_closed` default |
| Appointments aggregate | PARTIAL — read path + aggregate exist; write path ownership needs product confirmation | `GET /agents/{id}/appointments`; AppointmentDto |
| Identity links (link/unlink/resolve) | COMPLETE (verified live) | BFF login resolved `sub→agent_id`; Agency resolution 200 |
| Outbox + Kafka | COMPLETE | live `outbox_events` published; db_tests lease/fence/poison |
| Idempotency | COMPLETE | db_tests replay/conflict |
| Unsafe-mode guards | COMPLETE (this pass) | `validate_mode_safety` unit tests; PRs agent#17, agency#25 |

## C. API coverage

Runtime OpenAPI served at `/api/docs/openapi.json`; canonical spec in
`iaop-api-contracts/specs/development/agent-service/openapi.json`.
Catalog entry: `implemented_verified` / `e2e_verified` (agent-service was
already verified in earlier passes).

## D. Database evidence

Real isolated PostgreSQL 16 (`postgres:16-alpine`, disposable DBs
`agent_test`/`agency_test` on local Docker network — no shared DEV data):

- `cargo test --test db_tests` (agent-service): **2/2 PASSED** —
  migrations, agent CRUD, outbox lease claiming, fencing,
  poison-message terminal handling.
- `cargo test --test db_tests` (agency-service): **10/10 PASSED** —
  unique code, atomic outbox+audit, hierarchy ancestors/descendants +
  cycle rejection, temporal as-of, assignment overlap + transfer
  history, leadership assign/end + duplicate role rejection,
  concurrent reparent, resource-authz negatives, idempotency
  replay/conflict, outbox lease/fence/poison.

## E. Kafka evidence

- Agency DEV: `agency.agent-assigned.v1` published after bounded retry
  (UnknownTopicOrPartition → success), 102 outbox rows observed.
- Agent DEV: `EVENT_TRANSPORT=kafka`, `KAFKA_BROKERS=kafka:9092`.
- Failure-injection matrix (broker outage, worker restart, duplicate
  delivery): partially covered by unit/db tests; live fault injection
  NOT_EXECUTED this pass.

## F. Security evidence

- `AUTH_MODE=dev` / `ACTIVATION_MODE=fixture` now rejected for any
  `ENVIRONMENT` other than `local` (agent PR #17, agency PR #25);
  unit tests cover development/uat/production/staging rejection.
- DEV compose runs `AUTH_MODE=jwt` + real OIDC issuer/JWKS/audience.
- Agency→Agent validation uses service-account client credentials
  (`iaop-agency-service` client) — verified live (assignment creation
  validated agent ACTIVE via Agent Service).
- Identity resolution never parses Keycloak `sub` as agent ID.

## G. Integration evidence (live DEV, 2026-11-15)

- Keycloak password-grant token (`iaop-dev-cli`) → Agency
  `current-assignment` → identity resolved via Agent Service
  `identity-links/resolve` → **200**.
- BFF OIDC code+PKCE login (`demo.agent.1`) → `agentId` resolved at
  callback → `/api/v1/me` returns agent profile; workspaces/hierarchy
  populated from Agency Service.
- Portal `/dev-api/agency` proxy: portal session + Bearer JWT → 200;
  either missing → 401.

## H. Operational evidence

- Containers healthy on DEV: agent-service, agency-service, agent-bff,
  api-docs (image `iaop/api-docs:dev` @ contracts `329fc1dd`+).
- Agency deployed `ac0d3f3` (pre-#25; #25 is config-only, no API change).

## I. Open requirements

- IMPLEMENTATION_DEFECT: none known P0.
- MISSING_TEST: command-level PostgreSQL lifecycle E2E (activate
  requires appointment+eligibility decision path), concurrent lifecycle
  transitions, Kafka live failure-injection matrix, perf benchmarks.
- INTEGRATION_DEPENDENCY: eligibility decision contract for activation
  (DEV uses fail_closed — activation requests return INDETERMINATE by
  design until eligibility contract is exercised).
- BUSINESS_POLICY_UNRESOLVED: appointment write-path ownership
  (Onboarding/Integration vs Agent Service REST); promotion/rank mapping.
- NOT_REQUIRED: none identified.

## J. Verdict

`CONDITIONALLY DEV READY` — registry, identity links, lifecycle
machinery, outbox/Kafka, and cross-service auth are implemented,
deployed, and live-verified; command-level lifecycle E2E against real
PostgreSQL and the eligibility decision path remain to be exercised
before VERIFIED DEV READY.
