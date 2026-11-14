#![forbid(unsafe_code)]
//! Domain layer for Agent Lifecycle (Agent Registry & Identity).
//!
//! Pure domain model: aggregates, entities, value objects, domain
//! events, and errors. MUST NOT depend on Axum, SQLx, HTTP clients,
//! Keycloak, or message brokers.

pub mod agent;
pub mod appointment;
pub mod authorization;
pub mod entity_id;
pub mod error;
pub mod identity_link;
pub mod ids;
pub mod status;

pub use agent::{
    ActivationAuthorization, ActivationOperation, Actor, Agent, AgentCode, AgentEvent,
    LifecycleTransition, SuspensionReason, TerminationReason,
};
pub use appointment::{AgentAppointment, AppointmentStatus};
pub use authorization::{ActivationDecision, AuthorizationOutcome, verify_decision};
pub use entity_id::EntityId;
pub use error::AgentError;
pub use identity_link::{AgentIdentityLink, IdentityLinkStatus};
pub use ids::{AgentId, AppointmentId, CommandId, IdentityLinkId, PersonId};
pub use status::AgentStatus;
