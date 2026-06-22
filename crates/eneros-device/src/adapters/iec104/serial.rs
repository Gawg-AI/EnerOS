//! IEC 60870-5-104 串口传输层（基于 FT 1.2 帧格式）。
//!
//! 本模块实现 IEC 60870-5-1 定义的 FT 1.2 帧格式编解码，以及基于
//! termios 的 Linux 串口 I/O，用于通过串口链路传输 IEC 104 ASDU。
//!
//! # FT 1.2 帧格式
//!
//! 变长帧：
//! ```text
//! 0x68 | L | L | 0x68 | 数据(1..=255 字节) | 校验和 | 0x68
//! ```
//!
//! 固定帧（短帧，1 字节控制域）：
//! ```text
//! 0x68 | 控制域(1 字节) | 校验和 | 0x68
//! ```
//! 固定帧校验和 = 控制域字节本身（单字节之和 mod 256）。
//!
//! # 平台支持
//!
//! - FT 1.2 帧编解码函数跨平台可用（纯计算，无系统调用）
//! - `Iec104SerialTransport` 仅在 Linux 上可用（依赖 termios + libc）
//! - 非 Linux 平台返回 `Iec104SerialError::Unsupported`，保证编译通过
//!
//! 参考实现模式：`adapters::modbus_rtu`（termios 配置 + poll 超时读取）

use std::io;

// ============================================================================
// 常量
// ============================================================================

/// FT 1.2 帧起始/结束标记
const FT12_START_END: u8 = 0x68;
/// FT 1.2 最大数据长度
const FT12_MAX_DATA_LEN: usize = 255;

// ============================================================================
// 错误类型
// ============================================================================

/// IEC 104 串口错误
#[derive(Debug, thiserror::Error)]
pub enum Iec104SerialError {
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),
    #[error("FT 1.2 帧格式错误: {0}")]
    FrameFormat(String),
    #[error("校验和不匹配: 期望 {expected}, 实际 {actual}")]
    Checksum { expected: u8, actual: u8 },
    #[error("帧长度超限: {0} (最大 255)")]
    FrameTooLong(usize),
    #[error("串口超时")]
    Timeout,
    #[error("平台不支持: 仅 Linux 支持 IEC 104 串口")]
    Unsupported,
}

// ============================================================================
// 配置
// ============================================================================

/// IEC 104 串口配置
#[derive(Debug, Clone)]
pub struct Iec104SerialConfig {
    /// 串口设备路径，如 /dev/ttyS0、/dev/ttyUSB0
    pub device: String,
    /// 波特率（默认 9600）
    pub baud_rate: u32,
    /// 数据位（默认 8）
    pub data_bits: u8,
    /// 停止位（默认 1）
    pub stop_bits: u8,
    /// 校验：'N' 无校验, 'E' 偶校验, 'O' 奇校验
    pub parity: char,
    /// 响应超时（毫秒，默认 1000）
    pub timeout_ms: u64,
}

impl Default for Iec104SerialConfig {
    fn default() -> Self {
        Self {
            device: "/dev/ttyS0".to_string(),
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: 1,
            parity: 'N',
            timeout_ms: 1000,
        }
    }
}

// ============================================================================
// FT 1.2 帧编解码（跨平台纯函数）
// ============================================================================

/// 计算 FT 1.2 校验和（数据字节之和 mod 256，使用 wrapping_add 避免溢出）
pub fn ft12_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// 编码 FT 1.2 变长帧
///
/// 返回完整帧：`0x68 L L 0x68 data checksum 0x68`
pub fn encode_ft12_variable_frame(data: &[u8]) -> Result<Vec<u8>, Iec104SerialError> {
    if data.is_empty() {
        return Err(Iec104SerialError::FrameFormat("数据不能为空".to_string()));
    }
    if data.len() > FT12_MAX_DATA_LEN {
        return Err(Iec104SerialError::FrameTooLong(data.len()));
    }
    let len = data.len() as u8;
    let checksum = ft12_checksum(data);
    let mut frame = Vec::with_capacity(data.len() + 6);
    frame.push(FT12_START_END);
    frame.push(len);
    frame.push(len);
    frame.push(FT12_START_END);
    frame.extend_from_slice(data);
    frame.push(checksum);
    frame.push(FT12_START_END);
    Ok(frame)
}

/// 编码 FT 1.2 固定帧（短帧，1 字节控制域）
///
/// 返回：`0x68 control checksum 0x68`
/// 固定帧校验和 = 控制域字节本身。
pub fn encode_ft12_fixed_frame(control: u8) -> Vec<u8> {
    let checksum = control; // 单字节校验和 = 字节本身
    vec![FT12_START_END, control, checksum, FT12_START_END]
}

/// 解码 FT 1.2 帧（变长或固定）
///
/// 输入为完整帧字节（含起始/结束标记），返回数据部分。
/// - 固定帧（4 字节）：返回 `vec![control]`
/// - 变长帧：返回 data 字段
pub fn decode_ft12_frame(frame: &[u8]) -> Result<Vec<u8>, Iec104SerialError> {
    if frame.len() < 4 {
        return Err(Iec104SerialError::FrameFormat("帧长度不足".to_string()));
    }
    if frame[0] != FT12_START_END {
        return Err(Iec104SerialError::FrameFormat(format!(
            "起始字节错误: 0x{:02X}",
            frame[0]
        )));
    }

    // 固定帧: 0x68 control checksum 0x68 (4 字节)
    if frame.len() == 4 {
        if frame[3] != FT12_START_END {
            return Err(Iec104SerialError::FrameFormat("结束字节错误".to_string()));
        }
        let control = frame[1];
        let expected_checksum = control;
        if frame[2] != expected_checksum {
            return Err(Iec104SerialError::Checksum {
                expected: expected_checksum,
                actual: frame[2],
            });
        }
        return Ok(vec![control]);
    }

    // 变长帧: 0x68 L L 0x68 data checksum 0x68
    if frame.len() < 6 {
        return Err(Iec104SerialError::FrameFormat("变长帧长度不足".to_string()));
    }
    if frame[1] != frame[2] {
        return Err(Iec104SerialError::FrameFormat("长度字节不匹配".to_string()));
    }
    if frame[3] != FT12_START_END {
        return Err(Iec104SerialError::FrameFormat("第二个起始字节错误".to_string()));
    }
    let len = frame[1] as usize;
    let expected_total = 4 + len + 2; // start(1) + len(2) + start(1) + data + checksum(1) + end(1)
    if frame.len() != expected_total {
        return Err(Iec104SerialError::FrameFormat(format!(
            "帧长度不匹配: 期望 {}, 实际 {}",
            expected_total,
            frame.len()
        )));
    }
    let data = &frame[4..4 + len];
    let checksum_byte = frame[4 + len];
    let end_byte = frame[4 + len + 1];
    if end_byte != FT12_START_END {
        return Err(Iec104SerialError::FrameFormat("结束字节错误".to_string()));
    }
    let expected_checksum = ft12_checksum(data);
    if checksum_byte != expected_checksum {
        return Err(Iec104SerialError::Checksum {
            expected: expected_checksum,
            actual: checksum_byte,
        });
    }
    Ok(data.to_vec())
}

// ============================================================================
// Linux 串口传输层
// ============================================================================

#[cfg(target_os = "linux")]
mod linux_serial {
    use super::{
        ft12_checksum, encode_ft12_variable_frame, encode_ft12_fixed_frame,
        Iec104SerialConfig, Iec104SerialError, FT12_START_END, FT12_MAX_DATA_LEN,
    };
    use std::io;
    use std::os::unix::io::RawFd;
    use std::time::{Duration, Instant};

    /// IEC 104 串口传输层（Linux termios 实现）
    pub struct Iec104SerialTransport {
        config: Iec104SerialConfig,
        fd: RawFd,
    }

    /// 波特率映射到 libc speed_t
    ///
    /// 返回 `Err` 表示不支持的波特率，避免静默回退到 B9600 导致通信参数错误。
    fn baud_to_speed(baud: u32) -> std::result::Result<libc::speed_t, Iec104SerialError> {
        match baud {
            50 => Ok(libc::B50),
            75 => Ok(libc::B75),
            110 => Ok(libc::B110),
            134 => Ok(libc::B134),
            150 => Ok(libc::B150),
            200 => Ok(libc::B200),
            300 => Ok(libc::B300),
            600 => Ok(libc::B600),
            1200 => Ok(libc::B1200),
            1800 => Ok(libc::B1800),
            2400 => Ok(libc::B2400),
            4800 => Ok(libc::B4800),
            9600 => Ok(libc::B9600),
            19200 => Ok(libc::B19200),
            38400 => Ok(libc::B38400),
            57600 => Ok(libc::B57600),
            115200 => Ok(libc::B115200),
            230400 => Ok(libc::B230400),
            _ => Err(Iec104SerialError::FrameFormat(format!(
                "不支持的波特率: {}",
                baud
            ))),
        }
    }

    /// 计算 RTU 字符间超时（1.5 字符时间），最小 5ms
    fn inter_char_timeout_ms(baud_rate: u32) -> u64 {
        let ms = (1.5 * 11.0 * 1000.0 / baud_rate as f64) as u64;
        ms.max(5)
    }

    impl Iec104SerialTransport {
        /// 打开串口设备并配置 termios 参数
        pub fn new(config: Iec104SerialConfig) -> Result<Self, Iec104SerialError> {
            let c_device = std::ffi::CString::new(config.device.clone()).map_err(|e| {
                Iec104SerialError::FrameFormat(format!("设备路径错误: {}", e))
            })?;

            // O_RDWR: 读写 | O_NOCTTY: 不作为控制终端 | O_NONBLOCK: 打开时不阻塞
            let fd = unsafe {
                libc::open(
                    c_device.as_ptr(),
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }

            // 清除 O_NONBLOCK 标志，使 poll() + read() 正常工作
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
            if flags < 0 {
                unsafe { libc::close(fd); }
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }
            let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
            if ret < 0 {
                unsafe { libc::close(fd); }
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }

            let transport = Self { config, fd };
            transport.configure_serial()?;
            Ok(transport)
        }

        /// 配置 termios：原始模式 + 波特率/数据位/停止位/校验位
        fn configure_serial(&self) -> Result<(), Iec104SerialError> {
            let mut tio: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(self.fd, &mut tio) } != 0 {
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }

            // 原始模式（无特殊字符处理）
            unsafe { libc::cfmakeraw(&mut tio) };

            // 设置波特率
            let speed = baud_to_speed(self.config.baud_rate)?;
            unsafe {
                libc::cfsetispeed(&mut tio, speed);
                libc::cfsetospeed(&mut tio, speed);
            }

            // 启用接收，忽略调制解调器控制线
            tio.c_cflag |= libc::CLOCAL | libc::CREAD;

            // 数据位：先清除 CSIZE 掩码
            tio.c_cflag &= !libc::CSIZE;
            match self.config.data_bits {
                5 => tio.c_cflag |= libc::CS5,
                6 => tio.c_cflag |= libc::CS6,
                7 => tio.c_cflag |= libc::CS7,
                _ => tio.c_cflag |= libc::CS8,
            }

            // 校验位
            match self.config.parity {
                'E' | 'e' => {
                    tio.c_cflag |= libc::PARENB;
                    tio.c_cflag &= !libc::PARODD;
                }
                'O' | 'o' => {
                    tio.c_cflag |= libc::PARENB;
                    tio.c_cflag |= libc::PARODD;
                }
                _ => {
                    tio.c_cflag &= !libc::PARENB;
                }
            }

            // 停止位
            match self.config.stop_bits {
                2 => tio.c_cflag |= libc::CSTOPB,
                _ => tio.c_cflag &= !libc::CSTOPB,
            }

            // VMIN/VTIME 设为 0：由 poll() 控制超时，read() 立即返回已有数据
            tio.c_cc[libc::VMIN] = 0;
            tio.c_cc[libc::VTIME] = 0;

            if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &tio) } != 0 {
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }
            // 清空输入缓冲区，丢弃残留数据
            unsafe { libc::tcflush(self.fd, libc::TCIFLUSH) };
            Ok(())
        }

        /// 发送 FT 1.2 变长帧
        pub fn send_variable(&self, data: &[u8]) -> Result<(), Iec104SerialError> {
            let frame = encode_ft12_variable_frame(data)?;
            self.write_all(&frame)
        }

        /// 发送 FT 1.2 固定帧
        pub fn send_fixed(&self, control: u8) -> Result<(), Iec104SerialError> {
            let frame = encode_ft12_fixed_frame(control);
            self.write_all(&frame)
        }

        /// 接收 FT 1.2 帧（带超时），返回数据部分
        ///
        /// # 帧类型判定算法（不依赖字符间超时区分帧类型）
        ///
        /// FT 1.2 固定帧和变长帧的前 4 字节结构完全相同：
        /// - 固定帧：`0x68 C CS 0x68`（C=控制域, CS=校验和=C）
        /// - 变长帧：`0x68 L L 0x68 ...`（L=长度）
        ///
        /// 两者均满足 `b1 == b2` 且 `b3 == 0x68`，无法通过第 4 字节区分。
        ///
        /// 本算法采用"先尝试变长帧，失败回退固定帧"策略：
        /// 1. 读取 4 字节头部 `[0x68, b1, b2, b3]`
        /// 2. 验证 `b1 == b2` 且 `b3 == 0x68`，否则重新同步
        /// 3. `b1 == 0` 时直接判定为固定帧（变长帧要求 L >= 1）
        /// 4. 先尝试按变长帧读取 `b1` 字节 data + CS + 0x68：
        ///    - 短超时无数据 → 回退为固定帧，返回 `vec![b1]`
        ///    - 读取成功且校验通过 → 变长帧，返回 data
        ///    - 校验失败 → 丢弃已读字节，重新同步
        ///
        /// 相比旧算法（仅探测 1 字节即决定帧类型），本算法读取完整变长帧尾部
        /// 并校验后才确认，显著降低帧错位与数据丢失风险。
        pub fn recv_frame(&self) -> Result<Vec<u8>, Iec104SerialError> {
            let timeout = Duration::from_millis(self.config.timeout_ms);
            let deadline = Instant::now() + timeout;
            let short_timeout_ms = inter_char_timeout_ms(self.config.baud_rate);

            loop {
                // 1. 扫描到起始字节 0x68
                let start = self.read_bytes(1, deadline)?;
                if start[0] != FT12_START_END {
                    // 不是起始字节，继续扫描下一个 0x68
                    continue;
                }

                // 2. 读取 3 字节: [b1, b2, b3]
                let mid = self.read_bytes(3, deadline)?;
                let b1 = mid[0]; // 固定帧=控制域 C, 变长帧=长度 L
                let b2 = mid[1]; // 固定帧=校验和 CS, 变长帧=长度 L
                let b3 = mid[2]; // 固定帧=结束 0x68, 变长帧=第二个起始 0x68

                // 3. 验证 b1 == b2 且 b3 == 0x68（两种帧均满足）
                if b1 != b2 || b3 != FT12_START_END {
                    // 帧格式错误，从下一个 0x68 重新同步
                    continue;
                }

                // 4. b1 == 0 时只能是固定帧（变长帧要求 L >= 1）
                if b1 == 0 {
                    return Ok(vec![b1]);
                }

                // 5. 先尝试按变长帧读取：data(b1) + CS(1) + 0x68(1)
                let data_len = b1 as usize;
                match self.read_bytes_with_short_timeout(
                    data_len + 2,
                    short_timeout_ms,
                    deadline,
                ) {
                    Ok(tail) if tail.len() == data_len + 2 => {
                        let data = &tail[..data_len];
                        let cs = tail[data_len];
                        let end = tail[data_len + 1];
                        if end == FT12_START_END && cs == ft12_checksum(data) {
                            // 校验通过：变长帧
                            return Ok(data.to_vec());
                        }
                        // 校验失败：可能是固定帧被误判或数据损坏，重新同步
                        continue;
                    }
                    Ok(_) => {
                        // 读取不足（短超时无数据）：回退为固定帧
                        // 固定帧校验：b1=控制域, b2=CS=b1, b3=0x68（已验证）
                        return Ok(vec![b1]);
                    }
                    Err(Iec104SerialError::Timeout) => {
                        // 短超时无数据：回退为固定帧
                        return Ok(vec![b1]);
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        /// 获取配置引用
        pub fn config(&self) -> &Iec104SerialConfig {
            &self.config
        }

        // ---- 内部辅助方法 ----

        /// 写入全部数据
        ///
        /// 检查 `libc::write` 返回值：
        /// - `n < 0`：IO 错误
        /// - `n == 0`：写入 0 字节（WriteZero），避免无限循环
        /// - `n > 0`：推进写入偏移
        fn write_all(&self, data: &[u8]) -> Result<(), Iec104SerialError> {
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
                    return Err(Iec104SerialError::Io(io::Error::last_os_error()));
                }
                if n == 0 {
                    return Err(Iec104SerialError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "serial write returned 0 bytes",
                    )));
                }
                written += n as usize;
            }
            Ok(())
        }

        /// 带超时读取数据，返回读取的字节数（0 表示超时）
        fn read_with_timeout(
            &self,
            buf: &mut [u8],
            timeout_ms: u64,
        ) -> Result<usize, Iec104SerialError> {
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
                return Err(Iec104SerialError::Io(io::Error::last_os_error()));
            }
            if ret == 0 {
                return Ok(0); // 超时
            }
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    return Ok(0); // 竞态条件，视为超时
                }
                return Err(Iec104SerialError::Io(err));
            }
            Ok(n as usize)
        }

        /// 阻塞读取指定字节数，带截止时间超时
        fn read_bytes(&self, count: usize, deadline: Instant) -> Result<Vec<u8>, Iec104SerialError> {
            let mut buf = vec![0u8; count];
            let mut read = 0;
            while read < count {
                let now = Instant::now();
                if now >= deadline {
                    return Err(Iec104SerialError::Timeout);
                }
                let remaining_ms = (deadline - now).as_millis() as u64;
                // 每次最多等待 100ms，避免长时间阻塞
                let wait_ms = remaining_ms.min(100);
                let n = self.read_with_timeout(&mut buf[read..], wait_ms)?;
                if n == 0 {
                    continue; // 超时重试，由 deadline 控制总超时
                }
                read += n;
            }
            Ok(buf)
        }

        /// 带短超时探测读取指定字节数
        ///
        /// 首先用短超时（字符间超时）探测是否有数据：
        /// - 若短超时无数据 → 返回空 `Vec`（调用方据此判断为固定帧）
        /// - 若有数据 → 按完整截止时间读取剩余字节，返回全部数据
        ///
        /// 此方法用于 `recv_frame` 中区分固定帧与变长帧：
        /// 固定帧只有 4 字节头部，短超时后无更多数据；
        /// 变长帧在头部后紧跟 data + CS + 0x68，短超时内即可探测到。
        fn read_bytes_with_short_timeout(
            &self,
            count: usize,
            short_timeout_ms: u64,
            deadline: Instant,
        ) -> Result<Vec<u8>, Iec104SerialError> {
            // 短超时探测 1 字节，判断是否有更多数据
            let mut probe = [0u8; 1];
            let n = self.read_with_timeout(&mut probe, short_timeout_ms)?;
            if n == 0 {
                // 短超时无数据 → 固定帧
                return Ok(vec![]);
            }
            // 有数据 → 变长帧，读取剩余 count-1 字节
            let mut buf = Vec::with_capacity(count);
            buf.push(probe[0]);
            if count > 1 {
                let rest = self.read_bytes(count - 1, deadline)?;
                buf.extend_from_slice(&rest);
            }
            Ok(buf)
        }
    }

    impl Drop for Iec104SerialTransport {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.fd);
            }
        }
    }

    // --- Linux 专用单元测试 ---

    #[cfg(test)]
    mod tests {
        use super::*;

        // --- baud_to_speed 无效波特率返回错误 ---

        #[test]
        fn test_baud_to_speed_invalid() {
            // 不支持的波特率应返回 Err，而非静默回退到 B9600
            assert!(baud_to_speed(0).is_err());
            assert!(baud_to_speed(1).is_err());
            assert!(baud_to_speed(14400).is_err());
            assert!(baud_to_speed(500000).is_err());
            assert!(baud_to_speed(u32::MAX).is_err());
        }

        // --- baud_to_speed 有效波特率返回正确 speed_t ---

        #[test]
        fn test_baud_to_speed_valid() {
            assert_eq!(baud_to_speed(9600).unwrap(), libc::B9600);
            assert_eq!(baud_to_speed(19200).unwrap(), libc::B19200);
            assert_eq!(baud_to_speed(115200).unwrap(), libc::B115200);
            assert_eq!(baud_to_speed(50).unwrap(), libc::B50);
            assert_eq!(baud_to_speed(230400).unwrap(), libc::B230400);
        }

        // --- baud_to_speed 错误类型验证 ---

        #[test]
        fn test_baud_to_speed_error_type() {
            let err = baud_to_speed(9999).unwrap_err();
            assert!(matches!(err, Iec104SerialError::FrameFormat(_)));
            assert!(err.to_string().contains("9999"));
        }
    }
}

// ============================================================================
// 非 Linux 平台 stub
// ============================================================================

#[cfg(not(target_os = "linux"))]
mod stub {
    use super::Iec104SerialConfig;

    /// IEC 104 串口传输层（非 Linux 平台 stub）
    ///
    /// `new()` 始终返回 `Err(Unsupported)`，因此不会产生实例，
    /// 也不提供 `config()` 方法（避免 `unreachable!()` panic）。
    /// 如需在非 Linux 平台访问配置，请在调用 `new()` 前保存配置。
    pub struct Iec104SerialTransport {
        _phantom: (),
    }

    impl Iec104SerialTransport {
        pub fn new(_config: Iec104SerialConfig) -> Result<Self, super::Iec104SerialError> {
            Err(super::Iec104SerialError::Unsupported)
        }

        pub fn send_variable(&self, _data: &[u8]) -> Result<(), super::Iec104SerialError> {
            Err(super::Iec104SerialError::Unsupported)
        }

        pub fn send_fixed(&self, _control: u8) -> Result<(), super::Iec104SerialError> {
            Err(super::Iec104SerialError::Unsupported)
        }

        pub fn recv_frame(&self) -> Result<Vec<u8>, super::Iec104SerialError> {
            Err(super::Iec104SerialError::Unsupported)
        }
    }
}

// 平台条件 re-export
#[cfg(target_os = "linux")]
pub use linux_serial::Iec104SerialTransport;
#[cfg(not(target_os = "linux"))]
pub use stub::Iec104SerialTransport;

// ============================================================================
// 单元测试（FT 1.2 帧编解码部分跨平台可测试）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 校验和测试 ---

    #[test]
    fn test_ft12_checksum() {
        // 空输入
        assert_eq!(ft12_checksum(&[]), 0);
        // 单字节
        assert_eq!(ft12_checksum(&[0x01]), 1);
        // 溢出回绕：0xFF + 0x01 = 0x100 → 0x00
        assert_eq!(ft12_checksum(&[0xFF, 0x01]), 0);
        // 多字节求和
        assert_eq!(ft12_checksum(&[0x10, 0x20, 0x30]), 0x60);
        // 全 0xFF 回绕
        assert_eq!(ft12_checksum(&[0xFF, 0xFF, 0xFF]), 0xFD);
    }

    // --- 变长帧编码测试 ---

    #[test]
    fn test_encode_variable_frame() {
        let data = [0x01, 0x02, 0x03];
        let frame = encode_ft12_variable_frame(&data).unwrap();
        // 0x68 L L 0x68 data CS 0x68 = 3 + 6 = 9 字节
        assert_eq!(frame.len(), 9);
        assert_eq!(frame[0], 0x68); // 起始
        assert_eq!(frame[1], 3); // L
        assert_eq!(frame[2], 3); // L
        assert_eq!(frame[3], 0x68); // 第二个起始
        assert_eq!(&frame[4..7], &data); // 数据
        assert_eq!(frame[7], ft12_checksum(&data)); // 校验和
        assert_eq!(frame[8], 0x68); // 结束
    }

    // --- 固定帧编码测试 ---

    #[test]
    fn test_encode_fixed_frame() {
        let control = 0x07; // STARTDT_ACT
        let frame = encode_ft12_fixed_frame(control);
        // 0x68 control CS 0x68 = 4 字节
        assert_eq!(frame.len(), 4);
        assert_eq!(frame[0], 0x68);
        assert_eq!(frame[1], control);
        assert_eq!(frame[2], control); // 单字节校验和 = 控制域本身
        assert_eq!(frame[3], 0x68);
    }

    // --- 变长帧解码测试 ---

    #[test]
    fn test_decode_variable_frame() {
        let data = [0x01, 0x02, 0x03];
        let frame = encode_ft12_variable_frame(&data).unwrap();
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, data.to_vec());
    }

    // --- 固定帧解码测试 ---

    #[test]
    fn test_decode_fixed_frame() {
        let control = 0x43; // TESTFR_ACT
        let frame = encode_ft12_fixed_frame(control);
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![control]);
    }

    // --- 编解码往返测试 ---

    #[test]
    fn test_encode_decode_roundtrip() {
        // 变长帧往返
        let data = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let frame = encode_ft12_variable_frame(&data).unwrap();
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, data);

        // 固定帧往返
        let control = 0x0B; // STARTDT_CON
        let frame = encode_ft12_fixed_frame(control);
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![control]);
    }

    // --- 错误起始字节测试 ---

    #[test]
    fn test_decode_bad_start_byte() {
        let frame = [0x00, 0x01, 0x01, 0x68, 0x01, 0x01, 0x68];
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::FrameFormat(_))));
    }

    // --- 错误结束字节测试 ---

    #[test]
    fn test_decode_bad_end_byte() {
        let data = [0x01, 0x02, 0x03];
        let mut frame = encode_ft12_variable_frame(&data).unwrap();
        let len = frame.len();
        frame[len - 1] = 0x00; // 破坏结束字节
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::FrameFormat(_))));
    }

    // --- 校验和不匹配测试 ---

    #[test]
    fn test_decode_checksum_mismatch() {
        let data = [0x01, 0x02, 0x03];
        let mut frame = encode_ft12_variable_frame(&data).unwrap();
        let len = frame.len();
        frame[len - 2] ^= 0xFF; // 破坏校验和
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::Checksum { .. })));
    }

    // --- 长度字节不匹配测试 ---

    #[test]
    fn test_decode_length_mismatch() {
        // 0x68 0x05 0x06 0x68 ... (两个长度字节不匹配)
        let frame = [0x68, 0x05, 0x06, 0x68, 0x00, 0x00, 0x00];
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::FrameFormat(_))));
    }

    // --- 空数据错误测试 ---

    #[test]
    fn test_encode_empty_data() {
        let result = encode_ft12_variable_frame(&[]);
        assert!(matches!(result, Err(Iec104SerialError::FrameFormat(_))));
    }

    // --- 超长数据错误测试 ---

    #[test]
    fn test_encode_too_long() {
        let data = vec![0x00; 256];
        let result = encode_ft12_variable_frame(&data);
        assert!(matches!(result, Err(Iec104SerialError::FrameTooLong(_))));
    }

    // --- 256 字节超限测试 ---

    #[test]
    fn test_frame_too_long_256() {
        let data = vec![0xFF; 256];
        let result = encode_ft12_variable_frame(&data);
        assert!(result.is_err());
        if let Err(Iec104SerialError::FrameTooLong(n)) = result {
            assert_eq!(n, 256);
        } else {
            panic!("期望 FrameTooLong 错误");
        }
    }

    // --- 255 字节边界测试（应成功）---

    #[test]
    fn test_encode_max_length_255() {
        let data = vec![0x42; 255];
        let frame = encode_ft12_variable_frame(&data).unwrap();
        assert_eq!(frame.len(), 255 + 6);
        assert_eq!(frame[1], 255);
        assert_eq!(frame[2], 255);
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded.len(), 255);
        assert_eq!(decoded, data);
    }

    // --- 配置默认值测试 ---

    #[test]
    fn test_config_default() {
        let cfg = Iec104SerialConfig::default();
        assert_eq!(cfg.device, "/dev/ttyS0");
        assert_eq!(cfg.baud_rate, 9600);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, 1);
        assert_eq!(cfg.parity, 'N');
        assert_eq!(cfg.timeout_ms, 1000);
    }

    // --- 帧长度不足测试 ---

    #[test]
    fn test_decode_too_short() {
        assert!(decode_ft12_frame(&[]).is_err());
        assert!(decode_ft12_frame(&[0x68]).is_err());
        assert!(decode_ft12_frame(&[0x68, 0x01]).is_err());
        assert!(decode_ft12_frame(&[0x68, 0x01, 0x01]).is_err());
    }

    // --- 固定帧校验和错误测试 ---

    #[test]
    fn test_decode_fixed_frame_checksum_error() {
        // 固定帧，校验和与控制域不匹配
        let frame = [0x68, 0x07, 0x08, 0x68];
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::Checksum { .. })));
    }

    // --- 变长帧总长度不匹配测试 ---

    #[test]
    fn test_decode_variable_frame_length_mismatch() {
        // L=5 但数据不足
        let frame = [0x68, 0x05, 0x05, 0x68, 0x01, 0x02, 0x68];
        let result = decode_ft12_frame(&frame);
        assert!(matches!(result, Err(Iec104SerialError::FrameFormat(_))));
    }

    // --- 错误类型 Display 测试 ---

    #[test]
    fn test_error_display() {
        assert_eq!(Iec104SerialError::Timeout.to_string(), "串口超时");
        assert_eq!(
            Iec104SerialError::Unsupported.to_string(),
            "平台不支持: 仅 Linux 支持 IEC 104 串口"
        );
        let e = Iec104SerialError::Checksum {
            expected: 0x10,
            actual: 0x20,
        };
        assert!(e.to_string().contains("0x10") || e.to_string().contains("16"));
        assert!(e.to_string().contains("0x20") || e.to_string().contains("32"));
    }

    // --- 多种控制域值测试 ---

    #[test]
    fn test_fixed_frame_various_controls() {
        let controls = [0x07, 0x0B, 0x13, 0x23, 0x43, 0x83];
        for &c in &controls {
            let frame = encode_ft12_fixed_frame(c);
            let decoded = decode_ft12_frame(&frame).unwrap();
            assert_eq!(decoded, vec![c]);
        }
    }

    // --- 变长帧显式解析测试（不依赖超时，直接测试 decode_ft12_frame）---

    #[test]
    fn test_decode_variable_frame_explicit() {
        // 手动构造变长帧: 0x68 L L 0x68 data CS 0x68
        // data = [0x01, 0x02, 0x03, 0x04], L = 4
        // CS = 0x01 + 0x02 + 0x03 + 0x04 = 0x0A
        let frame = [0x68, 0x04, 0x04, 0x68, 0x01, 0x02, 0x03, 0x04, 0x0A, 0x68];
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![0x01, 0x02, 0x03, 0x04]);
    }

    // --- 固定帧显式解析测试 ---

    #[test]
    fn test_decode_fixed_frame_explicit() {
        // 手动构造固定帧: 0x68 C CS 0x68
        // C = 0x07 (STARTDT_ACT), CS = 0x07
        let frame = [0x68, 0x07, 0x07, 0x68];
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![0x07]);
    }

    // --- 固定帧控制域为 0 的边界测试 ---
    // recv_frame 新算法中 b1 == 0 直接判定为固定帧

    #[test]
    fn test_decode_fixed_frame_control_zero() {
        // 固定帧，控制域 = 0x00, CS = 0x00
        let frame = [0x68, 0x00, 0x00, 0x68];
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![0x00]);
    }

    // --- 变长帧单字节数据测试 ---

    #[test]
    fn test_decode_variable_frame_single_byte() {
        // 变长帧，data = [0x53], L = 1, CS = 0x53
        let frame = [0x68, 0x01, 0x01, 0x68, 0x53, 0x53, 0x68];
        let decoded = decode_ft12_frame(&frame).unwrap();
        assert_eq!(decoded, vec![0x53]);
    }

    // --- 变长帧与固定帧结构对比测试 ---
    // 验证两种帧的前 4 字节结构相同（b1==b2, b3==0x68）

    #[test]
    fn test_fixed_and_variable_frame_same_header_structure() {
        // 固定帧: 0x68 0x07 0x07 0x68 (C=0x07, CS=0x07)
        let fixed_frame = encode_ft12_fixed_frame(0x07);
        // 变长帧: 0x68 0x07 0x07 0x68 ... (L=7)
        let var_data = vec![0x01; 7];
        let var_frame = encode_ft12_variable_frame(&var_data).unwrap();

        // 前 4 字节结构相同
        assert_eq!(&fixed_frame[..4], &var_frame[..4]);
        assert_eq!(fixed_frame[1], fixed_frame[2]); // b1 == b2
        assert_eq!(fixed_frame[3], 0x68); // b3 == 0x68
        assert_eq!(var_frame[1], var_frame[2]); // b1 == b2
        assert_eq!(var_frame[3], 0x68); // b3 == 0x68

        // 但总长度不同：固定帧 4 字节，变长帧 4 + 7 + 2 = 13 字节
        assert_eq!(fixed_frame.len(), 4);
        assert_eq!(var_frame.len(), 13);
    }
}
