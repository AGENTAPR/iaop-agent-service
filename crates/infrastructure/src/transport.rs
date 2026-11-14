//! Event transports for the transactional outbox publisher.
//!
//! Provides [`LogTransport`] for local development without external dependencies,
//! and [`KafkaTransport`] for enterprise event publication to Apache Kafka / KRaft.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use rskafka::client::ClientBuilder;
use rskafka::client::partition::{Compression, UnknownTopicHandling};
use rskafka::record::Record;
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

/// Production Kafka transport publishing integration events with real broker acknowledgement.
pub struct KafkaTransport {
    brokers: Vec<String>,
    topic_prefix: String,
    timeout: Duration,
}

impl KafkaTransport {
    pub fn new(brokers: String, topic_prefix: Option<String>) -> Self {
        let topic_prefix = topic_prefix.unwrap_or_else(|| "iaop".to_string());
        let broker_list: Vec<String> = brokers
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            brokers: broker_list,
            topic_prefix,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn topic_for(&self, event_type: &str) -> String {
        format!("{}.{}", self.topic_prefix, event_type)
    }

    pub fn brokers(&self) -> &[String] {
        &self.brokers
    }
}

impl EventTransport for KafkaTransport {
    async fn deliver(&self, event: &IntegrationEvent) -> Result<(), String> {
        if self.brokers.is_empty() {
            return Err("Kafka transport unconfigured: empty broker list".to_string());
        }

        let topic = self.topic_for(&event.event_type);
        let key = event.aggregate_id.to_string();
        let payload = serde_json::to_vec(&event)
            .map_err(|e| format!("failed to serialize event envelope: {e}"))?;

        // 1. Establish connection to Kafka broker cluster within timeout
        let client = tokio::time::timeout(
            self.timeout,
            ClientBuilder::new(self.brokers.clone()).build(),
        )
        .await
        .map_err(|_| format!("Kafka broker connection timed out after {:?}", self.timeout))?
        .map_err(|e| format!("Kafka client build failed: {e}"))?;

        // 2. Select partition (deterministic partition 0)
        let partition = 0;
        let partition_client = tokio::time::timeout(
            self.timeout,
            client.partition_client(&topic, partition, UnknownTopicHandling::Error),
        )
        .await
        .map_err(|_| format!("Kafka partition client resolution timed out for topic '{topic}'"))?
        .map_err(|e| format!("Kafka partition client error for topic '{topic}': {e}"))?;

        // 3. Construct Kafka record with canonical headers
        let mut headers = BTreeMap::new();
        headers.insert(
            "event_id".to_string(),
            event.event_id.to_string().into_bytes(),
        );
        headers.insert(
            "event_type".to_string(),
            event.event_type.clone().into_bytes(),
        );
        headers.insert(
            "event_version".to_string(),
            event.event_version.to_string().into_bytes(),
        );
        if let Some(corr) = event.correlation_id {
            headers.insert("correlation_id".to_string(), corr.to_string().into_bytes());
        }
        if let Some(caus) = event.causation_id {
            headers.insert("causation_id".to_string(), caus.to_string().into_bytes());
        }

        let record = Record {
            key: Some(key.into_bytes()),
            value: Some(payload),
            timestamp: Utc::now(),
            headers,
        };

        // 4. Produce with broker acknowledgement
        let offsets = tokio::time::timeout(
            self.timeout,
            partition_client.produce(vec![record], Compression::NoCompression),
        )
        .await
        .map_err(|_| format!("Kafka produce timed out after {:?}", self.timeout))?
        .map_err(|e| format!("Kafka produce failed with broker error: {e}"))?;

        info!(
            topic = %topic,
            event_id = %event.event_id,
            partition = partition,
            offset = ?offsets.first(),
            "Kafka broker acknowledged integration event publication"
        );

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_for_uses_prefix() {
        let t = KafkaTransport::new("localhost:9092".to_string(), Some("iaop.agent".to_string()));
        assert_eq!(t.topic_for("agent.created"), "iaop.agent.agent.created");
    }

    #[tokio::test]
    async fn kafka_delivery_fails_cleanly_when_offline() {
        // Point to an invalid unreachable broker port
        let transport =
            KafkaTransport::new("127.0.0.1:19999".to_string(), Some("test".to_string()));
        let event = IntegrationEvent {
            event_id: uuid::Uuid::now_v7(),
            event_type: "agent.activated".to_string(),
            event_version: 1,
            source: "test".to_string(),
            aggregate_id: uuid::Uuid::now_v7(),
            aggregate_version: Some(1),
            occurred_at: Utc::now(),
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({"test": true}),
        };

        let result = transport.deliver(&event).await;
        assert!(result.is_err(), "must fail when broker is unreachable");
        let err = result.unwrap_err();
        assert!(
            err.contains("Kafka"),
            "error should indicate Kafka connectivity failure: {err}"
        );
    }

    #[tokio::test]
    async fn kafka_delivery_succeeds_against_live_broker() {
        let brokers =
            std::env::var("KAFKA_BROKERS").unwrap_or_else(|_| "127.0.0.1:9092".to_string());
        let transport = KafkaTransport::new(brokers, Some("iaop".to_string()));
        let event_id = uuid::Uuid::now_v7();
        let aggregate_id = uuid::Uuid::now_v7();
        let event = IntegrationEvent {
            event_id,
            event_type: "agent.events.v1".to_string(),
            event_version: 1,
            source: "iaop-agent-service".to_string(),
            aggregate_id,
            aggregate_version: Some(1),
            occurred_at: Utc::now(),
            correlation_id: Some(uuid::Uuid::now_v7()),
            causation_id: None,
            payload: serde_json::json!({
                "agent_code": "AG-007",
                "action": "AGENT_REAL_KAFKA_VERIFIED"
            }),
        };

        let result = transport.deliver(&event).await;
        assert!(
            result.is_ok(),
            "Kafka delivery must succeed against live broker: {:?}",
            result
        );
    }
}
