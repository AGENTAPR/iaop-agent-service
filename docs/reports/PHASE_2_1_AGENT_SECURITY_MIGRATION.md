# Phase 2.1 — Agent Service Security Migration Report

## Shared crate
- Repo: AGENTAPR/iaop-rust-common, branch `development`
- Pinned SHA: `5ccc60ce5b0ec58c18bd1e9536997d7e213dc62a`
- Dependency: `iaop-security = { git = ".../iaop-rust-common", rev = "5ccc60ce..." }` — private repo fetched successfully via git credential helper; Cargo.lock updated.

## Changes
- `crates/infrastructure/src/jwt.rs`: removed local JWKS fetch/cache/decode (~150 lines); now delegates to `iaop_security::jwks::JwtValidator<RemoteJwksProvider>` and maps validated `TokenClaims` -> service `Principal`. Audience-scoped client-role merge preserved verbatim.
- Restored `TokenValidatorImpl` enum + `DevBypassValidator` (dev-mode gated, unchanged).
- Fixed pre-existing test compile errors: `auth` fn shadowed by local in domain tests; `aggregate_id` type mismatch in messaging test.

## Compatibility matrix
| Scenario | Before | After | Result |
|---|---|---|---|
| Valid token | 200/201 (live KC) | shared validator, same claims path | PASS (unit) |
| Missing/malformed | 401 | 401 via auth.rs Forbidden->401 mapping | PASS |
| Wrong audience | 401 | shared Validation aud check | PASS |
| Tampered sig | 401 | shared RS256 verify | PASS |
| Client roles aud-scoped | merged | identical merge logic kept | PASS |
| Unrelated client roles | excluded | excluded | PASS |

## Verification (local)
- `cargo build --workspace` PASS (incl. private git dep fetch)
- `cargo clippy --workspace --all-targets --all-features --locked -D warnings` PASS
- `cargo test --workspace --locked` PASS — 34 tests, 0 failed
- `cargo fmt` applied

## Not verified
- Live InstaBox Keycloak re-run after migration (machine may be suspended; not re-executed this session)
- JWKS outage/rotation live behavior (shared cache: refresh on miss; stale-cache fallback differs slightly from old MIN_REFRESH throttle — noted)

## PR status
- PR #2 (feature/phase-2-shared-keycloak-identity) updated by this push — OPEN, unmerged.
