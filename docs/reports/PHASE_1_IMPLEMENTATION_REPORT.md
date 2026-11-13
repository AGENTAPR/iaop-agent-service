# IAOP Phase 1 — Agent Registry & Identity Foundation
## Implementation Report

**Status:** `IMPLEMENTED_ON_FEATURE_BRANCH — PENDING_PR_REVIEW`

| Field | Value |
|---|---|
| Repository | `AGENTAPR/iaop-agent-service` |
| Source branch | `feature/phase-1-agent-registry-identity-foundation` |
| Base branch | `main` |
| Head commit | see `git log` on branch |
| Working tree | clean at report time |

> **Deviation disclosure:** the initial Phase 1 commit (`5f992c0`) was
> pushed directly to `main` before the feature-branch policy arrived.
> History was not rewritten per policy. All subsequent work is on the
> feature branch. Remaining verification gaps are tracked below.

## What was implemented

- **Domain** (`crates/domain`): `Agent` aggregate, lifecycle state
  machine (DRAFT, PENDING_ACTIVATION, ACTIVE, SUSPENDED, TERMINATED,
  REJECTED) with all specified transitions enforced in the aggregate,
  typed IDs (UUIDv7), value objects (AgentCode, reasons, references),
  domain events. No infrastructure dependencies.
- **Application** (`crates/application`): commands for register, read,
  list, submit-for-activation, activate, suspend, reinstate, terminate,
  link/unlink identity; queries for get/list/history/appointments/
  identity-links. Ports: `AgentStore`, `IdempotencyStore`,
  `ActivationAuthorizationPort`, `TokenValidator`, `EventTransport`.
  Every mutating use case: permission check → row-locked load →
  optimistic version check → domain transition → history + audit +
  outbox + idempotency completion in ONE transaction.
- **Infrastructure** (`crates/infrastructure`): `PgAgentStore`
  (SELECT … FOR UPDATE + `UPDATE … WHERE version = $n`), durable
  PostgreSQL idempotency store (reserve/complete inside tx, replay of
  stored responses), JWKS JWT validator (RS256 pinned; iss/aud/exp/nbf;
  kid-based key cache), outbox publisher (`FOR UPDATE SKIP LOCKED`,
  exponential backoff + jitter, at-least-once), `FailClosed` and
  `Fixture` activation adapters (fixture refused when
  `ENVIRONMENT=production`), `DevBypass` auth (same guard).
- **API** (`crates/api`): `/api/v1/agents` REST surface, Bearer auth
  extractor, RFC 9457 problem details, `Idempotency-Key` (required on
  mutations) and `If-Match` (optimistic concurrency) support, keyset
  pagination (`cursor`/`limit`), OpenAPI 3.1 document.
- **Migrations**: `agents`, `agent_appointments`,
  `agent_identity_links`, `agent_lifecycle_history`,
  `agent_audit_events`, `idempotency_records`, outbox delivery columns
  (`next_attempt_at`, `publish_attempts`, `last_error`).

## Local verification (actually executed)

| Check | Result |
|---|---|
| `cargo fmt` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS (0 warnings) |
| `cargo test --locked` | PASS — unit + 2 integration smoke tests |
| `cargo build` | PASS |

## Live PostgreSQL E2E (executed against postgres:16 container)

Full lifecycle verified over HTTP with `AUTH_MODE=dev` +
`ACTIVATION_MODE=fixture` (local-only modes):

1. `POST /agents` → 201, `AGT-…` code, DRAFT, row in `agents` ✔
2. `POST …/identity-links` → ACTIVE link ✔
3. Appointment row inserted (no API — owned by Onboarding service) ✔
4. `POST …/submit-for-activation` → PENDING_ACTIVATION, v1 ✔
5. `POST …/activate` → ACTIVE, v2 (fixture authz + appointment + link verified) ✔
6. `POST …/suspend` → SUSPENDED, v3 ✔
7. `POST …/reinstate` → ACTIVE, v4 ✔
8. `POST …/terminate` → TERMINATED, v5 ✔
9. `GET …/history` → 6 transitions, correct from/to + versions ✔
10. DB inspection: 7 outbox events all `published_at` set (log
    transport), 7 audit rows, 7 COMPLETED idempotency records ✔
11. Idempotent replay: re-POST `submit-for-activation` with same
    `Idempotency-Key` returned the stored v1 response — no duplicate
    transition/audit/outbox ✔
12. Invalid transition (suspend TERMINATED agent) → 409
    `INVALID_LIFECYCLE_TRANSITION` ✔
13. `If-Match` mismatch → 412 `PRECONDITION_FAILED` ✔

## Security verification

- Missing Bearer token → 401 `UNAUTHENTICATED` (verified)
- `AUTH_MODE=dev` / `ACTIVATION_MODE=fixture` are refused at startup
  when `ENVIRONMENT=production` (config-enforced)
- Activation requires: authenticated principal + `Agent:activate`
  permission + valid ACTIVE appointment + ACTIVE identity link +
  non-INDETERMINATE authorization decision scoped to agent/appointment/
  operation and unexpired — all verified in `commands.rs`
- JWKS validator: RS256 only, iss/aud/exp/nbf enforced, tokens never
  logged — implemented; **not yet exercised against live Keycloak**

## Event delivery evidence (honest separation)

| Stage | Status |
|---|---|
| Outbox row written atomically with state change | VERIFIED (7 rows) |
| Publisher claims & marks published | VERIFIED (log transport) |
| Broker delivery | NOT IMPLEMENTED — `LogTransport` only; no broker chosen yet |
| Consumer inbox dedup | Schema exists (inbox baseline); no consumer yet |

Delivery semantics claimed: **at-least-once**. Exactly-once is NOT claimed.

## Not yet done / blockers

- Real Keycloak token validation E2E (needs running Keycloak realm)
- Automated Postgres integration test suite (concurrency, constraint,
  rollback cases) — manual E2E only so far
- `iaop-api-contracts` / `iaop-event-contracts` agent-schema updates
  (separate repos, separate feature branches + PRs required)
- Docker image build verification
- GitHub Actions run (main-only policy; will run after authorized merge)
- Reference-standard doc + future-service blueprint in
  `iaop-architecture`

## PR

- PR URL: _created after branch push_
- Merge status: OPEN — not merged, awaiting review
- Main CI status: pending (runs only after merge to `main`)
