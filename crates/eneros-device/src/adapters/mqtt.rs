use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use rumqttc::{AsyncClient, Event, MqttOptions, QoS, Packet, SubscribeFilter};
use std::collections::HashMap;

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state, ProtocolConfig,
};
use crate::protocol::ProtocolType;

pub struct MqttAdapter {
    client: Option<Arc<Mutex<AsyncClient>>>,
    shared_state: SharedState,
    name: String,
    subscribed_topics: Arc<Mutex<HashMap<String, bool>>>,
}

impl MqttAdapter {
    pub fn new(name: &str) -> Self {
        Self {
            client: None,
            shared_state: new_shared_state(),
            name: name.to_string(),
            subscribed_topics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn parse_mqtt_address(address: &str) -> Result<(String, QoS)> {
        let parts: Vec<&str> = address.splitn(2, ':').collect();
        if parts.len() == 2 {
            let qos = match parts[1] {
                "0" => QoS::AtMostOnce,
                "1" => QoS::AtLeastOnce,
                "2" => QoS::ExactlyOnce,
                _ => QoS::AtLeastOnce,
            };
            Ok((parts[0].to_string(), qos))
        } else {
            Ok((address.to_string(), QoS::AtLeastOnce))
        }
    }
}

#[async_trait]
impl ProtocolAdapter for MqttAdapter {
    async fn connect(&mut self, config: &ConnectionConfig) -> Result<()> {
        self.shared_state
            .set_state(crate::adapter::ConnectionState::Connecting);

        let client_id = match &config.protocol_config {
            ProtocolConfig::Mqtt { client_id, .. } => client_id.clone(),
            _ => format!("eneros-{}", uuid::Uuid::new_v4()),
        };

        let keep_alive = std::time::Duration::from_secs(30);
        let mut mqttoptions = MqttOptions::new(
            &client_id,
            &config.host,
            config.port,
        );
        mqttoptions.set_keep_alive(keep_alive);

        if let Some(creds) = &config.credentials {
            mqttoptions.set_credentials(&creds.username, &creds.password);
        }

        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);

        let shared = self.shared_state.clone();
        let client_arc = Arc::new(Mutex::new(client));
        self.client = Some(client_arc);

        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        shared.record_received(publish.payload.len() as u64);
                        tracing::debug!(
                            "MQTT received on {}: {} bytes",
                            publish.topic,
                            publish.payload.len()
                        );
                    }
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        shared.mark_connected();
                        tracing::info!("MQTT connected");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        shared.record_error();
                        tracing::warn!("MQTT event loop error: {}", e);
                    }
                }
            }
        });

        self.shared_state.mark_connected();
        tracing::info!("MQTT adapter '{}' connected to {}:{}", self.name, config.host, config.port);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(client) = &self.client {
            let ctx = client.lock().await;
            ctx.disconnect().await.ok();
        }
        self.client = None;
        self.shared_state.mark_disconnected();
        tracing::info!("MQTT adapter '{}' disconnected", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let (_topic, _qos) = Self::parse_mqtt_address(address)?;

        let _client = self.client.as_ref().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        self.shared_state.record_received(0);

        Ok(DataPoint {
            address: address.to_string(),
            value: DataValue::Bool(false),
            timestamp: chrono::Utc::now().timestamp_millis(),
            quality: DataQuality::Uncertain,
        })
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        let (topic, qos) = Self::parse_mqtt_address(address)?;

        let client = self.client.as_ref().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        let payload = match value {
            DataValue::String(s) => s.as_bytes().to_vec(),
            DataValue::Bytes(b) => b.clone(),
            DataValue::Bool(v) => {
                if *v { b"true".to_vec() } else { b"false".to_vec() }
            }
            DataValue::Int16(v) => v.to_string().into_bytes(),
            DataValue::Int32(v) => v.to_string().into_bytes(),
            DataValue::Int64(v) => v.to_string().into_bytes(),
            DataValue::Float32(v) => v.to_string().into_bytes(),
            DataValue::Float64(v) => v.to_string().into_bytes(),
        };

        let ctx = client.lock().await;
        ctx.publish(&topic, qos, false, payload.clone()).await
            .map_err(|e| {
                self.shared_state.record_error();
                eneros_core::EnerOSError::Device(format!("MQTT publish failed: {}", e))
            })?;

        self.shared_state.record_sent(payload.len() as u64);
        tracing::debug!("MQTT published to {}: {} bytes", topic, payload.len());
        Ok(())
    }

    async fn subscribe(
        &mut self,
        addresses: Vec<String>,
        _callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        let client_arc = self.client.clone().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        let mut topics = Vec::new();

        for addr in &addresses {
            let (topic, qos) = Self::parse_mqtt_address(addr)?;
            topics.push((topic, qos, addr.clone()));
        }

        {
            let ctx = client_arc.lock().await;
            let filters: Vec<SubscribeFilter> = topics
                .iter()
                .map(|(t, q, _)| SubscribeFilter::new(t.clone(), *q))
                .collect();
            ctx.subscribe_many(filters).await
                .map_err(|e| {
                    eneros_core::EnerOSError::Device(format!("MQTT subscribe failed: {}", e))
                })?;
        }

        {
            let mut subs = self.subscribed_topics.lock().await;
            for (topic, _, _) in &topics {
                subs.insert(topic.clone(), true);
            }
        }

        tracing::info!(
            "MQTT adapter '{}' subscribed to {} topics",
            self.name,
            topics.len()
        );
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Mqtt
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
            && self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}
