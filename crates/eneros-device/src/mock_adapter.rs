#[cfg(test)]
pub mod mock {
    use std::collections::HashMap;
    use async_trait::async_trait;
    use parking_lot::RwLock;
    use std::sync::Arc;

    use crate::adapter::{
        ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
        SharedState, new_shared_state,
    };
    use crate::protocol::ProtocolType;

    pub struct MockAdapter {
        name: String,
        shared_state: SharedState,
        connected: Arc<RwLock<bool>>,
        store: Arc<RwLock<HashMap<String, DataValue>>>,
    }

    impl MockAdapter {
        pub fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                shared_state: new_shared_state(),
                connected: Arc::new(RwLock::new(false)),
                store: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        pub fn with_data(name: &str, data: HashMap<String, DataValue>) -> Self {
            Self {
                name: name.to_string(),
                shared_state: new_shared_state(),
                connected: Arc::new(RwLock::new(false)),
                store: Arc::new(RwLock::new(data)),
            }
        }

        pub fn set_data(&self, address: &str, value: DataValue) {
            self.store.write().insert(address.to_string(), value);
        }
    }

    #[async_trait]
    impl ProtocolAdapter for MockAdapter {
        async fn connect(&mut self, _config: &ConnectionConfig) -> eneros_core::Result<()> {
            *self.connected.write() = true;
            self.shared_state.mark_connected();
            Ok(())
        }

        async fn disconnect(&mut self) -> eneros_core::Result<()> {
            *self.connected.write() = false;
            self.shared_state.mark_disconnected();
            Ok(())
        }

        async fn read(&self, address: &str) -> eneros_core::Result<DataPoint> {
            if !*self.connected.read() {
                return Err(eneros_core::EnerOSError::Device("Not connected".into()));
            }

            let store = self.store.read();
            let value = store.get(address).cloned().unwrap_or(DataValue::Int16(0));
            self.shared_state.record_received(4);

            Ok(DataPoint {
                address: address.to_string(),
                value,
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Good,
            })
        }

        async fn write(
            &mut self,
            address: &str,
            value: &DataValue,
        ) -> eneros_core::Result<()> {
            if !*self.connected.read() {
                return Err(eneros_core::EnerOSError::Device("Not connected".into()));
            }

            self.store
                .write()
                .insert(address.to_string(), value.clone());
            self.shared_state.record_sent(4);
            Ok(())
        }

        async fn subscribe(
            &mut self,
            _addresses: Vec<String>,
            _callback: Box<dyn Fn(DataPoint) + Send + Sync>,
        ) -> eneros_core::Result<()> {
            Ok(())
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn protocol_type(&self) -> ProtocolType {
            ProtocolType::Modbus
        }

        fn is_connected(&self) -> bool {
            *self.connected.read()
        }

        fn shared_state(&self) -> SharedState {
            self.shared_state.clone()
        }
    }
}
