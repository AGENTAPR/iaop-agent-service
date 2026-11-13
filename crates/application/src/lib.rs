#![forbid(unsafe_code)]
// AFIT: ports are used via generics (concrete impls), never dyn.
// All impl futures are Send; suppression is intentional.
#![allow(async_fn_in_trait)]
//! Application layer for Agent Lifecycle (Agent Registry & Identity).
//!
//! Use cases (commands/queries) and the ports they depend on. Depends on
//! `domain` and `sqlx` transaction types (the transaction boundary) —
//! never on Axum, HTTP clients, or broker SDKs.

pub mod commands;
pub mod error;
pub mod permissions;
pub mod ports;
pub mod principal;
pub mod queries;
pub mod service_info;

pub use error::AppError;
pub use principal::{Principal, TokenClaims};
