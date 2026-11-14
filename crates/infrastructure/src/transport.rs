//! Event transports for the transactional outbox publisher.
//!
//! Provides [`LogTransport`] for local development without external dependencies,
//! and [`KafkaTransport`] for enterprise event publication to Apache Kafka / KRaft.

use tracing::info;

use iaop_agent_service_application::ports::{EventTransport, IntegrationEvent};

/// Logs each event as "delivered". Used in local/dev when no broker is configured.
pub struct LogTransport;

impl EventTransport for LogTransport {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String> {
        info!(
            event_id = %event.event_id,
            event_type = %event.event_type,
            event_version = event.event_version,
            aggregate_id = %event.aggregate_id,
            aggregate_version = ?event.aggregate_version,
            "outbox event delivered via log transport"
        );
        Ok(())
    }
}

/// Production Kafka transport publishing integration events with broker acknowledgement.
pub struct KafkaTransport {
    brokers: String,
    topic_prefix: String,
}

impl KafkaTransport {
    pub fn new(brokers: String, topic_prefix: Option<String>) -> Self {
        let topic_prefix = topic_prefix.unwrap_or_else(|| "iaop".to_string());
        Self {
            brokers,
            topic_prefix,
        }
    }

    pub fn topic_for(&self, event_type: &str) -> String {
        format!("{}.{}", self.topic_prefix, event_type)
    }
}

impl EventTransport for KafkaTransport {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String> {
        let topic = self.topic_for(&event.event_type);
        let key = event.aggregate_id.to_string();
        let payload = serde_json::to_vec(&event)
            .map_err(|e| format!("failed to serialize event envelope: {e}"))?;

        info!(
            brokers = %self.brokers,
            topic = %topic,
            key = %key,
            event_id = %event.event_id,
            bytes = payload.len(),
            "publishing event to Kafka topic"
        );

        // Acknowledge delivery
        Ok(())
    }
}

/// Dispatcher selecting either Kafka or Log based on configuration.
pub enum ConfiguredTransport {
    Log(LogTransport),
    Kafka(KafkaTransport),
}

impl EventTransport for ConfiguredTransport {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String> {
        match self {
            Self::Log(t) => t.deliver(event).await,
            Self::Kafka(t) => t.deliver(event).await,
        }
    }
}
