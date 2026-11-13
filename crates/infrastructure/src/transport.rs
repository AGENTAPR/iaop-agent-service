//! Event transports for the outbox publisher.
//!
//! `LogTransport` is the Phase 1 default: it records delivery in
//! structured logs without claiming broker delivery. A real broker
//! adapter implements [`EventTransport`] once the org broker decision
//! is made — the publisher loop is transport-agnostic.

use tracing::info;

use iaop_agent_service_application::ports::{EventTransport, IntegrationEvent};

/// Logs each event as "delivered". NOT a broker — delivery to an
/// external system is NOT verified while this transport is active.
pub struct LogTransport;

impl EventTransport for LogTransport {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String> {
        info!(
            event_id = %event.event_id,
            event_type = %event.event_type,
            event_version = event.event_version,
            aggregate_id = ?event.aggregate_id,
            aggregate_version = ?event.aggregate_version,
            "outbox event delivered via log transport (no broker configured)"
        );
        Ok(())
    }
}
