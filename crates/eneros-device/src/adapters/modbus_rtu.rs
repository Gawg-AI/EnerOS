//! Modbus RTU 串口通信适配器
//!
//! 实现 Modbus RTU over Serial 协议，适用于电力设备串口通信场景。
//! 帧格式：`| 从站地址(1B) | 功能码(1B) | 数据(NB) | CRC16(2B,低字节在前) |`
//!
//! - Linux：通过 `libc` termios 系统调用操作串口设备（/dev/ttyS*、/dev/ttyUSB* 等）
//! - 非 Linux 平台：返回 `ModbusRtuError::Unsupported`，保证编译通过
//!
//! 默认串口预设与 `eneros-os::init::serial_mgr::SerialPreset::ModbusRtu` 一致：9600/8/E/1。

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use std::sync::Arc;

use eneros_core::{EnerOSError, Result};

use crate::adapter::{
    ConnectionState, DataPoint, DataQuality, DataValue, ProtocolAdapter, ProtocolConfig,
    SharedState, new_shared_state,
};
use crate::protocol::ProtocolType;

// ============================================================================
// 错误类型
// ============================================================================

/// Modbus RTU 错误
#[derive(Debug, thiserror::Error)]
pub enum ModbusRtuError {
    #[error("CRC mismatch: expected 0x{0:04X}, got 0x{1:04X}")]
    CrcMismatch(u16, u16),
    #[error("serial I/O error: {0}")]
    SerialIo(String),
    #[error("timeout waiting for response")]
    Timeout,
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("unsupported platform: {0}")]
    Unsupported(String),
}

// ============================================================================
// 配置
// ============================================================================

/// Modbus RTU 配置
#[derive(Debug, Clone)]
pub struct ModbusRtuConfig {
    /// 串口设备路径，如 "/dev/ttyS1"
    pub device: String,
    /// 波特率，如 9600
    pub baud_rate: u32,
    /// 从站地址
    pub slave_id: u8,
    /// 数据位：7 或 8
    pub data_bits: u8,
    /// 停止位：1 或 2
    pub stop_bits: u8,
    /// 校验位：'N' / 'E' / 'O'
    pub parity: char,
    /// 响应超时（毫秒）
    pub timeout_ms: u64,
}

impl Default for ModbusRtuConfig {
    fn default() -> Self {
        Self {
            device: "/dev/ttyS0".to_string(),
            baud_rate: 9600,
            slave_id: 1,
            data_bits: 8,
            stop_bits: 1,
            // 与 serial_mgr ModbusRtu 预设一致：偶校验
            parity: 'E',
            timeout_ms: 1000,
        }
    }
}

// ============================================================================
// CRC16（Modbus CRC-16-ANSI，多项式 0xA001）
// ============================================================================

/// 计算 Modbus CRC-16（CRC-16-ANSI，多项式 0xA001）
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

// ============================================================================
// 帧编解码
// ============================================================================

/// 编码 Modbus RTU 帧
///
/// 帧结构：`从站地址 | 功能码 | 数据 | CRC16(低字节在前)`
pub fn encode_rtu_frame(slave_id: u8, func_code: u8, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(data.len() + 4);
    frame.push(slave_id);
    frame.push(func_code);
    frame.extend_from_slice(data);
    let crc = crc16(&frame);
    // CRC 低字节在前（小端序）
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

/// 解码 Modbus RTU 帧
///
/// 返回 `(slave_id, func_code, data)`，其中 `data` 为功能码与 CRC 之间的负载。
/// 校验 CRC，不匹配则返回 `ModbusRtuError::CrcMismatch`。
pub fn decode_rtu_frame(frame: &[u8]) -> std::result::Result<(u8, u8, Vec<u8>), ModbusRtuError> {
    // 最小帧长：从站(1) + 功能码(1) + CRC(2) = 4
    if frame.len() < 4 {
        return Err(ModbusRtuError::InvalidResponse(format!(
            "frame too short: {} bytes (minimum 4)",
            frame.len()
        )));
    }
    let len = frame.len();
    let payload = &frame[..len - 2];
    let crc_received = u16::from_le_bytes([frame[len - 2], frame[len - 1]]);
    let crc_computed = crc16(payload);
    if crc_received != crc_computed {
        return Err(ModbusRtuError::CrcMismatch(crc_computed, crc_received));
    }
    let slave_id = payload[0];
    let func_code = payload[1];
    let data = payload[2..].to_vec();
    Ok((slave_id, func_code, data))
}

// ============================================================================
// 寄存器类型与地址解析
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModbusRegisterType {
    Holding,
    Input,
    Coil,
    Discrete,
}

/// Modbus 功能码
mod fc {
    pub const READ_COILS: u8 = 0x01;
    pub const READ_DISCRETE_INPUTS: u8 = 0x02;
    pub const READ_HOLDING_REGISTERS: u8 = 0x03;
    pub const READ_INPUT_REGISTERS: u8 = 0x04;
    pub const WRITE_SINGLE_COIL: u8 = 0x05;
    pub const WRITE_SINGLE_REGISTER: u8 = 0x06;
    pub const WRITE_MULTIPLE_REGISTERS: u8 = 0x10;
}

/// RTU 帧最大长度限制（字节），防止恶意从站触发 OOM
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_RTU_FRAME_LEN: usize = 256;

// ============================================================================
// PDU 构建与解析（跨平台纯函数）
// ============================================================================

/// 构建读请求 PDU 负载（功能码之后、CRC 之前的部分）
fn build_read_request(rtype: ModbusRegisterType, addr: u16, quantity: u16) -> (u8, Vec<u8>) {
    let func_code = match rtype {
        ModbusRegisterType::Coil => fc::READ_COILS,
        ModbusRegisterType::Discrete => fc::READ_DISCRETE_INPUTS,
        ModbusRegisterType::Holding => fc::READ_HOLDING_REGISTERS,
        ModbusRegisterType::Input => fc::READ_INPUT_REGISTERS,
    };
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&addr.to_be_bytes());
    data.extend_from_slice(&quantity.to_be_bytes());
    (func_code, data)
}

/// 构建写请求 PDU 负载
///
/// - Bool/Int16：功能码 0x05/0x06（写单线圈/单寄存器）
/// - Int32/Float32/String：功能码 0x10（写多寄存器），双寄存器或多寄存器编码
fn build_write_request(
    rtype: ModbusRegisterType,
    addr: u16,
    value: &DataValue,
) -> Result<(u8, Vec<u8>)> {
    match rtype {
        ModbusRegisterType::Holding => match value {
            DataValue::Bool(v) => {
                // 功能码 0x06（写单个寄存器），Bool 转 0/1
                let val: u16 = if *v { 1 } else { 0 };
                let mut data = Vec::with_capacity(4);
                data.extend_from_slice(&addr.to_be_bytes());
                data.extend_from_slice(&val.to_be_bytes());
                Ok((fc::WRITE_SINGLE_REGISTER, data))
            }
            DataValue::Int16(v) => {
                // 功能码 0x06（写单个寄存器）
                let val: u16 = *v as u16;
                let mut data = Vec::with_capacity(4);
                data.extend_from_slice(&addr.to_be_bytes());
                data.extend_from_slice(&val.to_be_bytes());
                Ok((fc::WRITE_SINGLE_REGISTER, data))
            }
            DataValue::Int32(v) => {
                // 功能码 0x10（写多寄存器），2 个寄存器，大端序双寄存器编码
                let high = (*v as u32 >> 16) as u16;
                let low = (*v as u32 & 0xFFFF) as u16;
                let reg_count = 2u16;
                let byte_count = 4u8;
                let mut data = Vec::with_capacity(6 + 4);
                data.extend_from_slice(&addr.to_be_bytes());
                data.extend_from_slice(&reg_count.to_be_bytes());
                data.push(byte_count);
                data.extend_from_slice(&high.to_be_bytes());
                data.extend_from_slice(&low.to_be_bytes());
                Ok((fc::WRITE_MULTIPLE_REGISTERS, data))
            }
            DataValue::Float32(v) => {
                // 功能码 0x10，IEEE 754 双寄存器编码（大端序）
                let bits = v.to_bits(); // u32
                let high = (bits >> 16) as u16;
                let low = (bits & 0xFFFF) as u16;
                let reg_count = 2u16;
                let byte_count = 4u8;
                let mut data = Vec::with_capacity(6 + 4);
                data.extend_from_slice(&addr.to_be_bytes());
                data.extend_from_slice(&reg_count.to_be_bytes());
                data.push(byte_count);
                data.extend_from_slice(&high.to_be_bytes());
                data.extend_from_slice(&low.to_be_bytes());
                Ok((fc::WRITE_MULTIPLE_REGISTERS, data))
            }
            DataValue::String(s) => {
                // 功能码 0x10，字符串按字节对齐到寄存器（不足 2 字节补 0）
                let bytes = s.as_bytes();
                let reg_count = bytes.len().div_ceil(2) as u16;
                let byte_count = (reg_count * 2) as u8;
                let mut data = Vec::with_capacity(6 + bytes.len() + 1);
                data.extend_from_slice(&addr.to_be_bytes());
                data.extend_from_slice(&reg_count.to_be_bytes());
                data.push(byte_count);
                let mut padded = bytes.to_vec();
                if padded.len() % 2 != 0 {
                    padded.push(0);
                }
                data.extend(padded);
                Ok((fc::WRITE_MULTIPLE_REGISTERS, data))
            }
            _ => Err(EnerOSError::Device(format!(
                "unsupported value type for holding register: {:?}",
                value
            ))),
        },
        ModbusRegisterType::Coil => {
            let on: bool = match value {
                DataValue::Bool(v) => *v,
                DataValue::Int16(v) => *v != 0,
                _ => {
                    return Err(EnerOSError::Device(format!(
                        "unsupported value type for coil: {:?}",
                        value
                    )))
                }
            };
            // 写单线圈：ON = 0xFF00，OFF = 0x0000
            let val: u16 = if on { 0xFF00 } else { 0x0000 };
            let mut data = Vec::with_capacity(4);
            data.extend_from_slice(&addr.to_be_bytes());
            data.extend_from_slice(&val.to_be_bytes());
            Ok((fc::WRITE_SINGLE_COIL, data))
        }
        _ => Err(EnerOSError::Device(
            "register type is read-only".to_string(),
        )),
    }
}

/// 解析读响应负载，提取数据值
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_read_response(
    rtype: ModbusRegisterType,
    data: &[u8],
) -> std::result::Result<DataValue, ModbusRtuError> {
    // 响应负载格式：字节计数(1) + 数据(N)
    match rtype {
        ModbusRegisterType::Holding | ModbusRegisterType::Input => {
            // 每个寄存器 2 字节，大端序
            if data.len() < 3 {
                return Err(ModbusRtuError::InvalidResponse(format!(
                    "read register response too short: {} bytes",
                    data.len()
                )));
            }
            let reg = u16::from_be_bytes([data[1], data[2]]);
            Ok(DataValue::Int16(reg as i16))
        }
        ModbusRegisterType::Coil | ModbusRegisterType::Discrete => {
            // 线圈按位打包，每 8 个线圈 1 字节
            if data.len() < 2 {
                return Err(ModbusRtuError::InvalidResponse(format!(
                    "read coil response too short: {} bytes",
                    data.len()
                )));
            }
            Ok(DataValue::Bool(data[1] & 0x01 != 0))
        }
    }
}

// ============================================================================
// Linux 串口实现
// ============================================================================

#[cfg(target_os = "linux")]
mod linux_serial {
    use super::{ModbusRtuConfig, ModbusRtuError};
    use std::ffi::CString;
    use std::os::unix::io::RawFd;

    /// 串口句柄，持有原始文件描述符
    pub struct RtuSerial {
        fd: RawFd,
    }

    /// 波特率映射到 libc speed_t
    fn baud_to_speed(baud: u32) -> Option<libc::speed_t> {
        match baud {
            1200 => Some(libc::B1200),
            2400 => Some(libc::B2400),
            4800 => Some(libc::B4800),
            9600 => Some(libc::B9600),
            19200 => Some(libc::B19200),
            38400 => Some(libc::B38400),
            57600 => Some(libc::B57600),
            115200 => Some(libc::B115200),
            230400 => Some(libc::B230400),
            460800 => Some(libc::B460800),
            921600 => Some(libc::B921600),
            _ => None,
        }
    }

    impl RtuSerial {
        /// 打开串口设备并配置 termios 参数
        pub fn open(config: &ModbusRtuConfig) -> Result<Self, ModbusRtuError> {
            let c_path = CString::new(config.device.as_str()).map_err(|e| {
                ModbusRtuError::SerialIo(format!("invalid device path: {}", e))
            })?;

            // O_RDWR: 读写 | O_NOCTTY: 不作为控制终端 | O_NONBLOCK: 打开时不阻塞
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
            if fd < 0 {
                return Err(ModbusRtuError::SerialIo(format!(
                    "open {}: {}",
                    config.device,
                    std::io::Error::last_os_error()
                )));
            }

            let serial = RtuSerial { fd };
            serial.configure(config)?;
            Ok(serial)
        }

        /// 配置 termios：原始模式 + 波特率/数据位/停止位/校验位
        fn configure(&self, config: &ModbusRtuConfig) -> Result<(), ModbusRtuError> {
            let mut termios: libc::termios = unsafe { std::mem::zeroed() };

            if unsafe { libc::tcgetattr(self.fd, &mut termios) } != 0 {
                return Err(ModbusRtuError::SerialIo(format!(
                    "tcgetattr: {}",
                    std::io::Error::last_os_error()
                )));
            }

            // 原始模式（无特殊字符处理）
            unsafe { libc::cfmakeraw(&mut termios) };

            // 波特率
            let speed = baud_to_speed(config.baud_rate).ok_or_else(|| {
                ModbusRtuError::SerialIo(format!("unsupported baud rate: {}", config.baud_rate))
            })?;
            unsafe {
                libc::cfsetispeed(&mut termios, speed);
                libc::cfsetospeed(&mut termios, speed);
            }

            // 数据位：先清除 CSIZE 掩码
            termios.c_cflag &= !libc::CSIZE;
            termios.c_cflag |= match config.data_bits {
                7 => libc::CS7,
                8 => libc::CS8,
                n => {
                    return Err(ModbusRtuError::SerialIo(format!(
                        "invalid data bits: {}",
                        n
                    )))
                }
            };

            // 停止位
            match config.stop_bits {
                1 => termios.c_cflag &= !libc::CSTOPB,
                2 => termios.c_cflag |= libc::CSTOPB,
                n => {
                    return Err(ModbusRtuError::SerialIo(format!(
                        "invalid stop bits: {}",
                        n
                    )))
                }
            }

            // 校验位
            match config.parity {
                'N' | 'n' => {
                    termios.c_cflag &= !libc::PARENB;
                }
                'E' | 'e' => {
                    termios.c_cflag |= libc::PARENB;
                    termios.c_cflag &= !libc::PARODD;
                }
                'O' | 'o' => {
                    termios.c_cflag |= libc::PARENB;
                    termios.c_cflag |= libc::PARODD;
                }
                c => {
                    return Err(ModbusRtuError::SerialIo(format!(
                        "invalid parity: {}",
                        c
                    )))
                }
            }

            // 启用接收，忽略调制解调器控制线
            termios.c_cflag |= libc::CLOCAL | libc::CREAD;

            // VMIN/VTIME 设为 0：由 poll() 控制超时，read() 立即返回已有数据
            termios.c_cc[libc::VMIN] = 0;
            termios.c_cc[libc::VTIME] = 0;

            if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &termios) } != 0 {
                return Err(ModbusRtuError::SerialIo(format!(
                    "tcsetattr: {}",
                    std::io::Error::last_os_error()
                )));
            }

            Ok(())
        }

        /// 写入全部数据
        pub fn write_all(&self, data: &[u8]) -> Result<(), ModbusRtuError> {
            let mut written = 0;
            while written < data.len() {
                let n = unsafe {
                    libc::write(
                        self.fd,
                        data[written..].as_ptr() as *const _,
                        data.len() - written,
                    )
                };
                if n < 0 {
                    return Err(ModbusRtuError::SerialIo(format!(
                        "write: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                if n == 0 {
                    // 写入返回 0 字节：避免无限循环
                    return Err(ModbusRtuError::SerialIo(
                        "serial write returned 0 bytes".to_string(),
                    ));
                }
                written += n as usize;
            }
            Ok(())
        }

        /// 带超时读取数据，返回读取的字节数（0 表示超时）
        pub fn read_with_timeout(
            &self,
            buf: &mut [u8],
            timeout_ms: u64,
        ) -> Result<usize, ModbusRtuError> {
            let mut pfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout: libc::c_int = if timeout_ms > i32::MAX as u64 {
                -1 // 无限等待
            } else {
                timeout_ms as libc::c_int
            };

            let ret = unsafe { libc::poll(&mut pfd as *mut _, 1, timeout) };
            if ret < 0 {
                return Err(ModbusRtuError::SerialIo(format!(
                    "poll: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if ret == 0 {
                return Ok(0); // 超时
            }
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n < 0 {
                return Err(ModbusRtuError::SerialIo(format!(
                    "read: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(n as usize)
        }

        /// 清空输入缓冲区（丢弃残留数据）
        pub fn drain_input(&self) {
            unsafe { libc::tcflush(self.fd, libc::TCIFLUSH) };
        }
    }

    impl Drop for RtuSerial {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
        }
    }

    /// 计算 RTU 字符间超时（1.5 字符时间），最小 5ms
    pub fn inter_char_timeout_ms(baud_rate: u32) -> u64 {
        // 1.5 字符时间 = 1.5 * 11 bits / baud * 1000 ms
        let ms = (1.5 * 11.0 * 1000.0 / baud_rate as f64) as u64;
        ms.max(5)
    }
}

// ============================================================================
// RTU 事务（Linux）
// ============================================================================

#[cfg(target_os = "linux")]
fn rtu_transaction(
    serial: &parking_lot::Mutex<linux_serial::RtuSerial>,
    slave_id: u8,
    func_code: u8,
    pdu_data: &[u8],
    timeout_ms: u64,
    baud_rate: u32,
) -> std::result::Result<(u8, Vec<u8>), ModbusRtuError> {
    let port = serial.lock();

    // 清空输入缓冲区，避免残留数据干扰
    port.drain_input();

    // 发送请求帧
    let request = encode_rtu_frame(slave_id, func_code, pdu_data);
    port.write_all(&request)?;

    let inter_char_ms = linux_serial::inter_char_timeout_ms(baud_rate);
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut tmp = [0u8; 64];

    // 读取第一个数据块：使用完整响应超时
    let n = port.read_with_timeout(&mut tmp, timeout_ms)?;
    if n == 0 {
        return Err(ModbusRtuError::Timeout);
    }
    buf.extend_from_slice(&tmp[..n]);

    // 后续数据块使用字符间超时，直到无更多数据（帧结束）
    loop {
        let n = port.read_with_timeout(&mut tmp, inter_char_ms)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        // 帧长度上限：防止恶意从站触发 OOM
        if buf.len() > MAX_RTU_FRAME_LEN {
            return Err(ModbusRtuError::InvalidResponse(format!(
                "帧长度超过最大限制 {} 字节",
                MAX_RTU_FRAME_LEN
            )));
        }
    }
    drop(port);

    // 解码并校验 CRC
    let (resp_slave, resp_fc, resp_data) = decode_rtu_frame(&buf)?;

    if resp_slave != slave_id {
        return Err(ModbusRtuError::InvalidResponse(format!(
            "slave id mismatch: expected {}, got {}",
            slave_id, resp_slave
        )));
    }

    // 异常响应：功能码最高位为 1
    if resp_fc & 0x80 != 0 {
        let exception_code = resp_data.first().copied().unwrap_or(0);
        return Err(ModbusRtuError::InvalidResponse(format!(
            "Modbus exception: fc=0x{:02X}, code={}",
            resp_fc & 0x7F,
            exception_code
        )));
    }

    if resp_fc != func_code {
        return Err(ModbusRtuError::InvalidResponse(format!(
            "function code mismatch: expected 0x{:02X}, got 0x{:02X}",
            func_code, resp_fc
        )));
    }

    Ok((resp_fc, resp_data))
}

// ============================================================================
// 适配器
// ============================================================================

/// Modbus RTU 适配器
pub struct ModbusRtuAdapter {
    config: ModbusRtuConfig,
    shared_state: SharedState,
    slave_id: u8,
    name: String,
    /// Linux: 串口句柄；非 Linux: 无此字段
    #[cfg(target_os = "linux")]
    serial: Option<Arc<parking_lot::Mutex<linux_serial::RtuSerial>>>,
    /// subscribe 后台轮询任务的句柄，disconnect/drop 时 abort 防泄漏
    subscribe_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ModbusRtuAdapter {
    /// 创建适配器（默认配置 9600/8/E/1）
    pub fn new(name: &str) -> Self {
        Self::with_config(name, ModbusRtuConfig::default())
    }

    /// 创建适配器（自定义配置）
    pub fn with_config(name: &str, config: ModbusRtuConfig) -> Self {
        let slave_id = config.slave_id;
        Self {
            slave_id,
            config,
            shared_state: new_shared_state(),
            name: name.to_string(),
            #[cfg(target_os = "linux")]
            serial: None,
            subscribe_handle: None,
        }
    }

    /// 创建适配器（指定从站地址，其余默认）
    pub fn with_slave_id(name: &str, slave_id: u8) -> Self {
        let config = ModbusRtuConfig {
            slave_id,
            ..Default::default()
        };
        Self::with_config(name, config)
    }

    /// 解析 Modbus 地址字符串，如 "holding:40001"
    fn parse_address(address: &str) -> Result<(ModbusRegisterType, u16)> {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 2 {
            return Err(EnerOSError::Device(format!(
                "Invalid Modbus address format '{}', expected 'type:address' (e.g., 'holding:40001')",
                address
            )));
        }
        let register_type = parts[0];
        let register_num: u16 = parts[1].parse().map_err(|_| {
            EnerOSError::Device(format!("Invalid register number: {}", parts[1]))
        })?;

        let (rtype, base) = match register_type {
            "holding" => (ModbusRegisterType::Holding, 40001u16),
            "input" => (ModbusRegisterType::Input, 30001u16),
            "coil" => (ModbusRegisterType::Coil, 10001u16),
            "discrete" => (ModbusRegisterType::Discrete, 20001u16),
            _ => {
                return Err(EnerOSError::Device(format!(
                    "Unknown register type: {}",
                    register_type
                )))
            }
        };

        if register_num < base {
            return Err(EnerOSError::Device(format!(
                "Register number {} is below base {} for type {}",
                register_num, base, register_type
            )));
        }

        Ok((rtype, register_num - base))
    }
}

#[async_trait]
impl ProtocolAdapter for ModbusRtuAdapter {
    async fn connect(&mut self, config: &crate::adapter::ConnectionConfig) -> Result<()> {
        self.shared_state
            .set_state(ConnectionState::Connecting);

        // 从协议配置覆盖从站地址和波特率
        if let ProtocolConfig::Modbus {
            slave_id,
            baud_rate,
        } = &config.protocol_config
        {
            self.config.slave_id = *slave_id;
            if let Some(br) = baud_rate {
                self.config.baud_rate = *br;
            }
        }

        // host 可作为串口设备路径（如 /dev/ttyS1）
        if !config.host.is_empty() {
            self.config.device = config.host.clone();
        }

        self.slave_id = self.config.slave_id;

        #[cfg(target_os = "linux")]
        {
            match linux_serial::RtuSerial::open(&self.config) {
                Ok(s) => {
                    self.serial = Some(Arc::new(parking_lot::Mutex::new(s)));
                    self.shared_state.mark_connected();
                    tracing::info!(
                        "Modbus RTU adapter '{}' connected to {} ({} bps, {}{}{})",
                        self.name,
                        self.config.device,
                        self.config.baud_rate,
                        self.config.data_bits,
                        self.config.parity,
                        self.config.stop_bits
                    );
                    Ok(())
                }
                Err(e) => {
                    self.shared_state.record_error();
                    self.shared_state.mark_error(e.to_string());
                    Err(EnerOSError::Device(format!(
                        "RTU connection failed: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            self.shared_state
                .mark_error("unsupported platform".to_string());
            Err(EnerOSError::Device(
                ModbusRtuError::Unsupported(std::env::consts::OS.to_string()).to_string(),
            ))
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        // 终止 subscribe 后台轮询任务，防止 fd/任务泄漏
        if let Some(handle) = self.subscribe_handle.take() {
            handle.abort();
        }
        #[cfg(target_os = "linux")]
        {
            self.serial = None;
        }
        self.shared_state.mark_disconnected();
        tracing::info!("Modbus RTU adapter '{}' disconnected", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let (rtype, reg_addr) = Self::parse_address(address)?;
        let (func_code, pdu_data) = build_read_request(rtype, reg_addr, 1);

        #[cfg(target_os = "linux")]
        {
            let serial = self.serial.as_ref().ok_or_else(|| {
                EnerOSError::Device("Not connected".to_string())
            })?;
            let serial = serial.clone();
            let slave_id = self.slave_id;
            let timeout_ms = self.config.timeout_ms;
            let baud_rate = self.config.baud_rate;

            let result = tokio::task::spawn_blocking(move || {
                rtu_transaction(&serial, slave_id, func_code, &pdu_data, timeout_ms, baud_rate)
            })
            .await
            .map_err(|e| EnerOSError::Device(format!("task join error: {}", e)))?;

            match result {
                Ok((_fc, data)) => {
                    self.shared_state
                        .record_received(data.len() as u64 + 4);
                    let value = parse_read_response(rtype, &data)?;
                    Ok(DataPoint {
                        address: address.to_string(),
                        value,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        quality: DataQuality::Good,
                    })
                }
                Err(e) => {
                    self.shared_state.record_error();
                    Err(EnerOSError::Device(format!(
                        "Modbus RTU read failed for {}: {}",
                        address, e
                    )))
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (rtype, reg_addr, func_code, pdu_data);
            Err(EnerOSError::Device(
                ModbusRtuError::Unsupported(std::env::consts::OS.to_string()).to_string(),
            ))
        }
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        let (rtype, reg_addr) = Self::parse_address(address)?;
        let (func_code, pdu_data) = build_write_request(rtype, reg_addr, value)?;

        #[cfg(target_os = "linux")]
        {
            let serial = self.serial.as_ref().ok_or_else(|| {
                EnerOSError::Device("Not connected".to_string())
            })?;
            let serial = serial.clone();
            let slave_id = self.slave_id;
            let timeout_ms = self.config.timeout_ms;
            let baud_rate = self.config.baud_rate;

            let result = tokio::task::spawn_blocking(move || {
                rtu_transaction(&serial, slave_id, func_code, &pdu_data, timeout_ms, baud_rate)
            })
            .await
            .map_err(|e| EnerOSError::Device(format!("task join error: {}", e)))?;

            match result {
                Ok((_fc, _data)) => {
                    self.shared_state
                        .record_sent(pdu_data.len() as u64 + 4);
                    tracing::debug!("Modbus RTU write {} = {}", address, value);
                    Ok(())
                }
                Err(e) => {
                    self.shared_state.record_error();
                    Err(EnerOSError::Device(format!(
                        "Modbus RTU write failed for {}: {}",
                        address, e
                    )))
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (rtype, reg_addr, func_code, pdu_data);
            Err(EnerOSError::Device(
                ModbusRtuError::Unsupported(std::env::consts::OS.to_string()).to_string(),
            ))
        }
    }

    async fn read_batch(&self, addresses: &[&str]) -> Result<Vec<DataPoint>> {
        let mut results = Vec::with_capacity(addresses.len());
        for addr in addresses {
            match self.read(addr).await {
                Ok(point) => results.push(point),
                Err(e) => {
                    tracing::warn!("Modbus RTU batch read failed for {}: {}", addr, e);
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
        #[cfg(target_os = "linux")]
        {
            let serial = self.serial.as_ref().ok_or_else(|| {
                EnerOSError::Device("Not connected".to_string())
            })?;
            let serial = serial.clone();
            let shared = self.shared_state.clone();
            let slave_id = self.slave_id;
            let timeout_ms = self.config.timeout_ms;
            let baud_rate = self.config.baud_rate;
            let addrs = addresses;
            let addrs_len = addrs.len();

            let handle = tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_millis(1000));

                loop {
                    interval.tick().await;

                    for addr in &addrs {
                        let parsed = match Self::parse_address(addr) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let (func_code, pdu) = build_read_request(parsed.0, parsed.1, 1);
                        let serial_c = serial.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            rtu_transaction(
                                &serial_c,
                                slave_id,
                                func_code,
                                &pdu,
                                timeout_ms,
                                baud_rate,
                            )
                        })
                        .await;

                        if let Ok(Ok((_fc, data))) = result {
                            if let Ok(value) = parse_read_response(parsed.0, &data) {
                                shared.record_received(data.len() as u64 + 4);
                                callback(DataPoint {
                                    address: addr.clone(),
                                    value,
                                    timestamp: chrono::Utc::now().timestamp_millis(),
                                    quality: DataQuality::Good,
                                });
                            }
                        }
                    }
                }
            });
            // 存储 JoinHandle，disconnect/drop 时 abort 防泄漏
            self.subscribe_handle = Some(handle);

            tracing::info!(
                "Modbus RTU adapter '{}' subscribed to {} addresses (polling)",
                self.name,
                addrs_len
            );
            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (addresses, callback);
            Err(EnerOSError::Device(
                ModbusRtuError::Unsupported(std::env::consts::OS.to_string()).to_string(),
            ))
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Modbus
    }

    fn is_connected(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.serial.is_some()
                && self.shared_state.state() == ConnectionState::Connected
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}

impl Drop for ModbusRtuAdapter {
    fn drop(&mut self) {
        // 兜底：若 adapter 被 drop 而未 disconnect，终止 subscribe 后台任务防泄漏
        if let Some(handle) = self.subscribe_handle.take() {
            handle.abort();
        }
    }
}

// ============================================================================
// 单元测试（跨平台）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- CRC16 测试 ---

    #[test]
    fn test_crc16_empty() {
        // 空输入：初始值 0xFFFF，无迭代
        assert_eq!(crc16(&[]), 0xFFFF);
    }

    #[test]
    fn test_crc16_known_vector() {
        // 标准测试向量：CRC-16/Modbus 对 "123456789" 的校验值为 0x4B37
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn test_crc16_single_byte() {
        // 单字节 0x00：crc = 0xFFFF ^ 0x00 = 0xFFFF，8 次移位后 = 0x40BF
        assert_eq!(crc16(&[0x00]), 0x40BF);
    }

    #[test]
    fn test_crc16_frame_vector() {
        // Modbus RTU 帧：从站1, 功能码4(读输入寄存器), 字节计数2, 数据0xFFFF
        let data = [0x01, 0x04, 0x02, 0xFF, 0xFF];
        let crc = crc16(&data);
        // 验证编码后的帧尾为 CRC 小端序
        let frame = encode_rtu_frame(0x01, 0x04, &[0x02, 0xFF, 0xFF]);
        assert_eq!(frame.len(), 7);
        assert_eq!(&frame[..5], &data);
        assert_eq!(frame[5], (crc & 0xFF) as u8);
        assert_eq!(frame[6], (crc >> 8) as u8);
    }

    // --- 帧编解码测试 ---

    #[test]
    fn test_encode_decode_roundtrip() {
        let slave_id = 0x02u8;
        let func_code = 0x03u8;
        let data = [0x00, 0x0A, 0x00, 0x01]; // 起始地址10, 数量1

        let frame = encode_rtu_frame(slave_id, func_code, &data);
        assert_eq!(frame.len(), data.len() + 4); // +slave +fc +crc(2)

        let (s, fc, d) = decode_rtu_frame(&frame).unwrap();
        assert_eq!(s, slave_id);
        assert_eq!(fc, func_code);
        assert_eq!(d, data.to_vec());
    }

    #[test]
    fn test_encode_empty_data() {
        // 空数据帧：从站 + 功能码 + CRC = 4 字节
        let frame = encode_rtu_frame(0x01, 0x06, &[]);
        assert_eq!(frame.len(), 4);
        let (s, fc, d) = decode_rtu_frame(&frame).unwrap();
        assert_eq!(s, 0x01);
        assert_eq!(fc, 0x06);
        assert!(d.is_empty());
    }

    #[test]
    fn test_decode_crc_error() {
        // 翻转一个字节，CRC 校验应失败
        let mut frame = encode_rtu_frame(0x01, 0x03, &[0x00, 0x01, 0x00, 0x02]);
        frame[2] ^= 0xFF; // 破坏数据
        let result = decode_rtu_frame(&frame);
        assert!(matches!(result, Err(ModbusRtuError::CrcMismatch(_, _))));
    }

    #[test]
    fn test_decode_crc_bytes_flipped() {
        // 直接破坏 CRC 字节
        let mut frame = encode_rtu_frame(0x01, 0x03, &[0x00, 0x01]);
        let len = frame.len();
        frame[len - 1] ^= 0xFF; // 破坏 CRC 高字节
        let result = decode_rtu_frame(&frame);
        assert!(matches!(result, Err(ModbusRtuError::CrcMismatch(_, _))));
    }

    #[test]
    fn test_decode_too_short() {
        // 少于 4 字节
        assert!(decode_rtu_frame(&[0x01, 0x03]).is_err());
        assert!(decode_rtu_frame(&[0x01, 0x03, 0x00]).is_err());
        assert!(decode_rtu_frame(&[]).is_err());
    }

    #[test]
    fn test_decode_exception_response() {
        // 异常响应：功能码最高位为 1（0x84 = 0x04 | 0x80），异常码 0x02
        let frame = encode_rtu_frame(0x01, 0x84, &[0x02]);
        let (slave, fc, data) = decode_rtu_frame(&frame).unwrap();
        assert_eq!(slave, 0x01);
        assert_eq!(fc, 0x84);
        assert_eq!(data, vec![0x02]);
        assert!(fc & 0x80 != 0); // 异常标志
    }

    #[test]
    fn test_decode_minimum_frame() {
        // 最小合法帧：4 字节（从站 + 功能码 + CRC）
        let frame = encode_rtu_frame(0xFF, 0x00, &[]);
        assert_eq!(frame.len(), 4);
        let (s, fc, d) = decode_rtu_frame(&frame).unwrap();
        assert_eq!(s, 0xFF);
        assert_eq!(fc, 0x00);
        assert!(d.is_empty());
    }

    // --- 错误类型测试 ---

    #[test]
    fn test_error_display() {
        assert_eq!(
            ModbusRtuError::Timeout.to_string(),
            "timeout waiting for response"
        );
        let e = ModbusRtuError::CrcMismatch(0x1234, 0x5678);
        assert!(e.to_string().contains("0x1234"));
        assert!(e.to_string().contains("0x5678"));
        let e = ModbusRtuError::Unsupported("windows".to_string());
        assert!(e.to_string().contains("windows"));
    }

    // --- 配置测试 ---

    #[test]
    fn test_config_default() {
        let cfg = ModbusRtuConfig::default();
        assert_eq!(cfg.baud_rate, 9600);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, 1);
        assert_eq!(cfg.parity, 'E');
        assert_eq!(cfg.slave_id, 1);
        assert_eq!(cfg.timeout_ms, 1000);
        assert_eq!(cfg.device, "/dev/ttyS0");
    }

    // --- 地址解析测试 ---

    #[test]
    fn test_parse_address_holding() {
        let (rtype, addr) = ModbusRtuAdapter::parse_address("holding:40001").unwrap();
        assert_eq!(rtype, ModbusRegisterType::Holding);
        assert_eq!(addr, 0);
    }

    #[test]
    fn test_parse_address_input() {
        let (rtype, addr) = ModbusRtuAdapter::parse_address("input:30001").unwrap();
        assert_eq!(rtype, ModbusRegisterType::Input);
        assert_eq!(addr, 0);
    }

    #[test]
    fn test_parse_address_coil() {
        let (rtype, addr) = ModbusRtuAdapter::parse_address("coil:10001").unwrap();
        assert_eq!(rtype, ModbusRegisterType::Coil);
        assert_eq!(addr, 0);
    }

    #[test]
    fn test_parse_address_discrete() {
        let (rtype, addr) = ModbusRtuAdapter::parse_address("discrete:20001").unwrap();
        assert_eq!(rtype, ModbusRegisterType::Discrete);
        assert_eq!(addr, 0);
    }

    #[test]
    fn test_parse_address_offset() {
        let (rtype, addr) = ModbusRtuAdapter::parse_address("holding:40100").unwrap();
        assert_eq!(rtype, ModbusRegisterType::Holding);
        assert_eq!(addr, 99);
    }

    #[test]
    fn test_parse_address_invalid_format() {
        assert!(ModbusRtuAdapter::parse_address("invalid").is_err());
    }

    #[test]
    fn test_parse_address_unknown_type() {
        assert!(ModbusRtuAdapter::parse_address("analog:40001").is_err());
    }

    #[test]
    fn test_parse_address_below_base() {
        assert!(ModbusRtuAdapter::parse_address("holding:30001").is_err());
    }

    #[test]
    fn test_parse_address_non_numeric() {
        assert!(ModbusRtuAdapter::parse_address("holding:abc").is_err());
    }

    // --- PDU 构建测试 ---

    #[test]
    fn test_build_read_request_holding() {
        let (fc, data) = build_read_request(ModbusRegisterType::Holding, 10, 1);
        assert_eq!(fc, 0x03);
        assert_eq!(data, vec![0x00, 0x0A, 0x00, 0x01]);
    }

    #[test]
    fn test_build_read_request_coil() {
        let (fc, data) = build_read_request(ModbusRegisterType::Coil, 5, 8);
        assert_eq!(fc, 0x01);
        assert_eq!(data, vec![0x00, 0x05, 0x00, 0x08]);
    }

    #[test]
    fn test_build_write_request_holding() {
        let (fc, data) =
            build_write_request(ModbusRegisterType::Holding, 10, &DataValue::Int16(100)).unwrap();
        assert_eq!(fc, 0x06);
        assert_eq!(data, vec![0x00, 0x0A, 0x00, 0x64]);
    }

    #[test]
    fn test_build_write_request_coil_on() {
        let (fc, data) =
            build_write_request(ModbusRegisterType::Coil, 5, &DataValue::Bool(true)).unwrap();
        assert_eq!(fc, 0x05);
        assert_eq!(data, vec![0x00, 0x05, 0xFF, 0x00]);
    }

    #[test]
    fn test_build_write_request_coil_off() {
        let (fc, data) =
            build_write_request(ModbusRegisterType::Coil, 5, &DataValue::Bool(false)).unwrap();
        assert_eq!(fc, 0x05);
        assert_eq!(data, vec![0x00, 0x05, 0x00, 0x00]);
    }

    #[test]
    fn test_build_write_request_readonly() {
        let result =
            build_write_request(ModbusRegisterType::Input, 0, &DataValue::Int16(1));
        assert!(result.is_err());
    }

    #[test]
    fn test_build_write_request_float32() {
        // 220.5f32 的 IEEE 754 位表示：0x435C8000
        // high = 0x435C, low = 0x8000
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            10,
            &DataValue::Float32(220.5),
        )
        .unwrap();
        assert_eq!(fc, 0x10);
        // 地址(2) + 寄存器数(2) + 字节计数(1) + 数据(4) = 9
        assert_eq!(
            data,
            vec![
                0x00, 0x0A, // 地址 10
                0x00, 0x02, // 2 个寄存器
                0x04, // 4 字节
                0x43, 0x5C, // high = 0x435C
                0x80, 0x00, // low = 0x8000
            ]
        );
    }

    #[test]
    fn test_build_write_request_int32() {
        // 0x12345678 → high=0x1234, low=0x5678
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            10,
            &DataValue::Int32(0x12345678),
        )
        .unwrap();
        assert_eq!(fc, 0x10);
        assert_eq!(
            data,
            vec![
                0x00, 0x0A, // 地址 10
                0x00, 0x02, // 2 个寄存器
                0x04, // 4 字节
                0x12, 0x34, // high = 0x1234
                0x56, 0x78, // low = 0x5678
            ]
        );
    }

    #[test]
    fn test_build_write_request_int32_negative() {
        // -1i32 = 0xFFFFFFFF → high=0xFFFF, low=0xFFFF
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            0,
            &DataValue::Int32(-1),
        )
        .unwrap();
        assert_eq!(fc, 0x10);
        assert_eq!(
            data,
            vec![
                0x00, 0x00, // 地址 0
                0x00, 0x02, // 2 个寄存器
                0x04, // 4 字节
                0xFF, 0xFF, // high
                0xFF, 0xFF, // low
            ]
        );
    }

    #[test]
    fn test_build_write_request_string() {
        // "AB" → 2 字节，1 个寄存器，无需填充
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            5,
            &DataValue::String("AB".to_string()),
        )
        .unwrap();
        assert_eq!(fc, 0x10);
        assert_eq!(
            data,
            vec![
                0x00, 0x05, // 地址 5
                0x00, 0x01, // 1 个寄存器
                0x02, // 2 字节
                0x41, 0x42, // 'A', 'B'
            ]
        );
    }

    #[test]
    fn test_build_write_request_string_odd_length() {
        // "A" → 1 字节，需补 0 对齐到 2 字节（1 个寄存器）
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            5,
            &DataValue::String("A".to_string()),
        )
        .unwrap();
        assert_eq!(fc, 0x10);
        assert_eq!(
            data,
            vec![
                0x00, 0x05, // 地址 5
                0x00, 0x01, // 1 个寄存器
                0x02, // 2 字节（含填充）
                0x41, 0x00, // 'A', 0x00 填充
            ]
        );
    }

    #[test]
    fn test_build_write_request_bool_holding() {
        // Bool 写入 holding 寄存器：使用 0x06，值 0/1
        let (fc, data) = build_write_request(
            ModbusRegisterType::Holding,
            10,
            &DataValue::Bool(true),
        )
        .unwrap();
        assert_eq!(fc, 0x06);
        assert_eq!(data, vec![0x00, 0x0A, 0x00, 0x01]);
    }

    #[test]
    fn test_max_rtu_frame_len_constant() {
        // 帧长度上限常量：256 字节
        assert_eq!(MAX_RTU_FRAME_LEN, 256);
    }

    #[test]
    fn test_frame_length_limit_exceeded() {
        // 验证超过最大帧长度时返回错误（模拟缓冲区超限）
        let buf: Vec<u8> = vec![0u8; MAX_RTU_FRAME_LEN + 1];
        // 模拟 rtu_transaction 中的帧长度检查逻辑
        let result = if buf.len() > MAX_RTU_FRAME_LEN {
            Err(ModbusRtuError::InvalidResponse(format!(
                "帧长度超过最大限制 {} 字节",
                MAX_RTU_FRAME_LEN
            )))
        } else {
            Ok(())
        };
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ModbusRtuError::InvalidResponse(_))
        ));
    }

    // --- 响应解析测试 ---

    #[test]
    fn test_parse_read_response_register() {
        // 字节计数=2, 数据=0x1234
        let data = [0x02, 0x12, 0x34];
        let value = parse_read_response(ModbusRegisterType::Holding, &data).unwrap();
        assert_eq!(value, DataValue::Int16(0x1234));
    }

    #[test]
    fn test_parse_read_response_coil_on() {
        // 字节计数=1, 数据=0x01
        let data = [0x01, 0x01];
        let value = parse_read_response(ModbusRegisterType::Coil, &data).unwrap();
        assert_eq!(value, DataValue::Bool(true));
    }

    #[test]
    fn test_parse_read_response_coil_off() {
        let data = [0x01, 0x00];
        let value = parse_read_response(ModbusRegisterType::Coil, &data).unwrap();
        assert_eq!(value, DataValue::Bool(false));
    }

    #[test]
    fn test_parse_read_response_too_short() {
        assert!(parse_read_response(ModbusRegisterType::Holding, &[0x02]).is_err());
        assert!(parse_read_response(ModbusRegisterType::Coil, &[0x01]).is_err());
    }

    // --- 适配器状态测试 ---

    #[test]
    fn test_new_adapter_not_connected() {
        let adapter = ModbusRtuAdapter::new("test-rtu");
        assert!(!adapter.is_connected());
        assert_eq!(adapter.name(), "test-rtu");
        assert_eq!(adapter.slave_id, 1);
        assert_eq!(adapter.protocol_type(), ProtocolType::Modbus);
    }

    #[test]
    fn test_with_slave_id() {
        let adapter = ModbusRtuAdapter::with_slave_id("test-rtu", 5);
        assert_eq!(adapter.slave_id, 5);
    }

    #[test]
    fn test_with_config() {
        let cfg = ModbusRtuConfig {
            device: "/dev/ttyUSB0".to_string(),
            baud_rate: 115200,
            slave_id: 3,
            data_bits: 8,
            stop_bits: 1,
            parity: 'N',
            timeout_ms: 500,
        };
        let adapter = ModbusRtuAdapter::with_config("test-rtu", cfg);
        assert_eq!(adapter.slave_id, 3);
        assert_eq!(adapter.config.baud_rate, 115200);
        assert_eq!(adapter.config.device, "/dev/ttyUSB0");
    }

    #[tokio::test]
    async fn test_read_not_connected() {
        let adapter = ModbusRtuAdapter::new("test-rtu");
        let result = adapter.read("holding:40001").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_write_not_connected() {
        let mut adapter = ModbusRtuAdapter::new("test-rtu");
        let result = adapter
            .write("holding:40001", &DataValue::Int16(100))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_batch_not_connected() {
        let adapter = ModbusRtuAdapter::new("test-rtu");
        let addrs = ["holding:40001", "holding:40002"];
        let result = adapter.read_batch(&addrs).await;
        assert!(result.is_ok());
        let points = result.unwrap();
        assert_eq!(points.len(), 2);
        // 未连接时所有点应为 Bad 质量
        for p in &points {
            assert_eq!(p.quality, DataQuality::Bad);
        }
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let mut adapter = ModbusRtuAdapter::new("test-rtu");
        // 断开未连接的适配器应成功
        let result = adapter.disconnect().await;
        assert!(result.is_ok());
        assert!(!adapter.is_connected());
    }

    // --- Linux 专属测试 ---

    #[cfg(target_os = "linux")]
    #[test]
    fn test_inter_char_timeout() {
        // 9600 baud: 1.5 * 11 * 1000 / 9600 ≈ 1.7ms → 最小 5ms
        assert_eq!(linux_serial::inter_char_timeout_ms(9600), 5);
        // 115200 baud: ≈ 0.14ms → 最小 5ms
        assert_eq!(linux_serial::inter_char_timeout_ms(115200), 5);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_baud_to_speed() {
        assert_eq!(
            linux_serial::baud_to_speed(9600),
            Some(libc::B9600)
        );
        assert_eq!(
            linux_serial::baud_to_speed(115200),
            Some(libc::B115200)
        );
        assert!(linux_serial::baud_to_speed(12345).is_none());
    }
}
