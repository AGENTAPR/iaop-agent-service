#![forbid(unsafe_code)]
//! Infrastructure layer for Agent Lifecycle (Agent Registry & Identity).
//!
//! Adapters implementing application ports: configuration, PostgreSQL
//! persistence (agent store, idempotency, outbox publisher), JWT/JWKS
//! token validation, activation authorization, event transport.

pub mod activation;
pub mod agent_store;
pub mod config;
pub mod idempotency;
pub mod jwt;
pub mod messaging;
pub mod outbox;
pub mod persistence;
pub mod security;
pub mod transport;
