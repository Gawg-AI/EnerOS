use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state, ProtocolConfig,
};
use crate::protocol::ProtocolType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModbusRegisterType {
    Holding,
    Input,
    Coil,
    Discrete,
}

pub struct ModbusTcpAdapter {
    client: Option<Arc<Mutex<tokio_modbus::client::Context>>>,
    shared_state: SharedState,
    slave_id: u8,
    name: String,
}

impl ModbusTcpAdapter {
    pub fn new(name: &str) -> Self {
        Self {
            client: None,
            shared_state: new_shared_state(),
            slave_id: 1,
            name: name.to_string(),
        }
    }

    pub fn with_slave_id(name: &str, slave_id: u8) -> Self {
        Self {
            client: None,
            shared_state: new_shared_state(),
            slave_id,
            name: name.to_string(),
        }
    }

    fn parse_address(address: &str) -> Result<(ModbusRegisterType, u16)> {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 2 {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Invalid Modbus address format '{}', expected 'type:address' (e.g., 'holding:40001')",
                address
            )));
        }
        let register_type = parts[0];
        let register_num: u16 = parts[1].parse().map_err(|_| {
            eneros_core::EnerOSError::Device(format!("Invalid register number: {}", parts[1]))
        })?;

        let (rtype, base) = match register_type {
            "holding" => (ModbusRegisterType::Holding, 40001u16),
            "input" => (ModbusRegisterType::Input, 30001u16),
            "coil" => (ModbusRegisterType::Coil, 10001u16),
            "discrete" => (ModbusRegisterType::Discrete, 20001u16),
            _ => {
                return Err(eneros_core::EnerOSError::Device(format!(
                    "Unknown register type: {}",
                    register_type
                )))
            }
        };

        if register_num < base {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Register number {} is below base {} for type {}",
                register_num, base, register_type
            )));
        }

        Ok((rtype, register_num - base))
    }
}

#[async_trait]
impl ProtocolAdapter for ModbusTcpAdapter {
    async fn connect(&mut self, config: &ConnectionConfig) -> Result<()> {
        use tokio_modbus::prelude::*;

        self.shared_state
            .set_state(crate::adapter::ConnectionState::Connecting);

        if let ProtocolConfig::Modbus { slave_id, .. } = &config.protocol_config {
            self.slave_id = *slave_id;
        }

        let addr = format!("{}:{}", config.host, config.port);
        let socket_addr: std::net::SocketAddr = addr.parse().map_err(|_| {
            eneros_core::EnerOSError::Device(format!("Invalid address: {}", addr))
        })?;

        let mut ctx = client::tcp::connect(socket_addr)
            .await
            .map_err(|e| {
                self.shared_state.record_error();
                self.shared_state.mark_error(e.to_string());
                eneros_core::EnerOSError::Device(format!("TCP connection failed: {}", e))
            })?;

        let slave = Slave(self.slave_id);
        ctx.set_slave(slave);

        self.client = Some(Arc::new(Mutex::new(ctx)));
        self.shared_state.mark_connected();

        tracing::info!("Modbus adapter '{}' connected to {}", self.name, addr);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.client = None;
        self.shared_state.mark_disconnected();
        tracing::info!("Modbus adapter '{}' disconnected", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        use tokio_modbus::prelude::*;

        let client = self.client.as_ref().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        let (register_type, register_addr) = Self::parse_address(address)?;

        let result = {
            let mut ctx = client.lock().await;
            match register_type {
                ModbusRegisterType::Holding => {
                    ctx.read_holding_registers(register_addr, 1).await
                }
                ModbusRegisterType::Input => {
                    ctx.read_input_registers(register_addr, 1).await
                }
                ModbusRegisterType::Coil => {
                    let r = ctx.read_coils(register_addr, 1).await;
                    r.map(|inner| inner.map(|v| v.into_iter().map(|b| b as u16).collect()))
                }
                ModbusRegisterType::Discrete => {
                    let r = ctx.read_discrete_inputs(register_addr, 1).await;
                    r.map(|inner| inner.map(|v| v.into_iter().map(|b| b as u16).collect()))
                }
            }
        };

        match result {
            Ok(Ok(data)) => {
                self.shared_state.record_received(data.len() as u64 * 2);

                let value = match register_type {
                    ModbusRegisterType::Holding | ModbusRegisterType::Input => {
                        if let Some(&v) = data.first() {
                            DataValue::Int16(v as i16)
                        } else {
                            DataValue::Int16(0)
                        }
                    }
                    ModbusRegisterType::Coil | ModbusRegisterType::Discrete => {
                        if let Some(&v) = data.first() {
                            DataValue::Bool(v != 0)
                        } else {
                            DataValue::Bool(false)
                        }
                    }
                };

                Ok(DataPoint {
                    address: address.to_string(),
                    value,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    quality: DataQuality::Good,
                })
            }
            Ok(Err(e)) => {
                self.shared_state.record_error();
                Err(eneros_core::EnerOSError::Device(format!(
                    "Modbus exception for {}: {:?}",
                    address, e
                )))
            }
            Err(e) => {
                self.shared_state.record_error();
                Err(eneros_core::EnerOSError::Device(format!(
                    "Modbus read failed for {}: {}",
                    address, e
                )))
            }
        }
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        use tokio_modbus::prelude::*;

        let client = self.client.as_ref().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        let (register_type, register_addr) = Self::parse_address(address)?;

        let write_result = {
            let mut ctx = client.lock().await;
            match register_type {
                ModbusRegisterType::Holding => {
                    let val = match value {
                        DataValue::Int16(v) => *v as u16,
                        DataValue::Int32(v) => *v as u16,
                        DataValue::Float32(v) => (*v as i16) as u16,
                        DataValue::Bool(v) => {
                            if *v {
                                1u16
                            } else {
                                0u16
                            }
                        }
                        _ => {
                            return Err(eneros_core::EnerOSError::Device(format!(
                                "Unsupported value type for holding register: {:?}",
                                value
                            )))
                        }
                    };
                    ctx.write_single_register(register_addr, val)
                        .await
                        .map(|r| r.map(|_| ()))
                }
                ModbusRegisterType::Coil => {
                    let val = match value {
                        DataValue::Bool(v) => *v,
                        DataValue::Int16(v) => *v != 0,
                        _ => {
                            return Err(eneros_core::EnerOSError::Device(format!(
                                "Unsupported value type for coil: {:?}",
                                value
                            )))
                        }
                    };
                    ctx.write_single_coil(register_addr, val)
                        .await
                        .map(|r| r.map(|_| ()))
                }
                _ => {
                    return Err(eneros_core::EnerOSError::Device(
                        "Register type is read-only".to_string(),
                    ))
                }
            }
        };

        match write_result {
            Ok(Ok(())) => {
                self.shared_state.record_sent(4);
                tracing::debug!("Modbus write {} = {}", address, value);
                Ok(())
            }
            Ok(Err(e)) => {
                self.shared_state.record_error();
                Err(eneros_core::EnerOSError::Device(format!(
                    "Modbus write exception for {}: {:?}",
                    address, e
                )))
            }
            Err(e) => {
                self.shared_state.record_error();
                Err(eneros_core::EnerOSError::Device(format!(
                    "Modbus write failed for {}: {}",
                    address, e
                )))
            }
        }
    }

    async fn read_batch(&self, addresses: &[&str]) -> Result<Vec<DataPoint>> {
        use tokio_modbus::prelude::*;

        let mut results = Vec::with_capacity(addresses.len());

        let holding_regs: Vec<(u16, &str)> = addresses
            .iter()
            .filter_map(|addr| {
                Self::parse_address(addr)
                    .ok()
                    .and_then(|(rtype, reg)| {
                        if rtype == ModbusRegisterType::Holding {
                            Some((reg, *addr))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        if holding_regs.len() > 1 {
            let min_addr = holding_regs.iter().map(|(r, _)| *r).min().unwrap();
            let max_addr = holding_regs.iter().map(|(r, _)| *r).max().unwrap();
            let count = max_addr - min_addr + 1;

            let client = self.client.as_ref().ok_or_else(|| {
                eneros_core::EnerOSError::Device("Not connected".to_string())
            })?;

            let batch_result = {
                let mut ctx = client.lock().await;
                ctx.read_holding_registers(min_addr, count).await
            };

            match batch_result {
                Ok(Ok(data)) => {
                    self.shared_state.record_received(data.len() as u64 * 2);
                    for (reg, addr) in &holding_regs {
                        let idx = (*reg - min_addr) as usize;
                        let value = if idx < data.len() {
                            DataValue::Int16(data[idx] as i16)
                        } else {
                            DataValue::Int16(0)
                        };
                        results.push(DataPoint {
                            address: addr.to_string(),
                            value,
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            quality: DataQuality::Good,
                        });
                    }
                }
                _ => {
                    self.shared_state.record_error();
                    for (_, addr) in &holding_regs {
                        results.push(DataPoint {
                            address: addr.to_string(),
                            value: DataValue::Bool(false),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            quality: DataQuality::Bad,
                        });
                    }
                }
            }
        }

        for addr in addresses {
            if results.iter().any(|r| r.address == *addr) {
                continue;
            }
            match self.read(addr).await {
                Ok(point) => results.push(point),
                Err(_) => {
                    results.push(DataPoint {
                        address: addr.to_string(),
                        value: DataValue::Bool(false),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        quality: DataQuality::Bad,
                    });
                }
            }
        }

        Ok(results)
    }

    async fn subscribe(
        &mut self,
        addresses: Vec<String>,
        callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        use tokio_modbus::prelude::*;

        let shared = self.shared_state.clone();
        let client_arc = self.client.clone().ok_or_else(|| {
            eneros_core::EnerOSError::Device("Not connected".to_string())
        })?;

        let addrs = addresses;
        let addrs_len = addrs.len();
        let interval_ms = 1000;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));

            loop {
                interval.tick().await;

                for addr in &addrs {
                    let parsed = match Self::parse_address(addr) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let result = {
                        let mut ctx = client_arc.lock().await;
                        match parsed.0 {
                            ModbusRegisterType::Holding => {
                                ctx.read_holding_registers(parsed.1, 1).await
                            }
                            ModbusRegisterType::Input => {
                                ctx.read_input_registers(parsed.1, 1).await
                            }
                            _ => continue,
                        }
                    };

                    if let Ok(Ok(data)) = result {
                        if let Some(&v) = data.first() {
                            let point = DataPoint {
                                address: addr.clone(),
                                value: DataValue::Int16(v as i16),
                                timestamp: chrono::Utc::now().timestamp_millis(),
                                quality: DataQuality::Good,
                            };
                            shared.record_received(2);
                            callback(point);
                        }
                    }
                }
            }
        });

        tracing::info!(
            "Modbus adapter '{}' subscribed to {} addresses (polling)",
            self.name,
            addrs_len
        );
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Modbus
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
            && self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}
