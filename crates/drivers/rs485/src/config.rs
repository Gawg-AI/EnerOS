//! RS485 配置类型与结构（D2 偏差：本地定义 UART 相关类型）.
//!
//! 蓝图假设 `UartPort`/`StopBits`/`Parity`/`GpioPin` 已在 HAL 中定义，
//! 但实际 HAL `types.rs` 中无这些类型。因此在本 crate 内定义。

/// UART 串口号
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartPort {
    /// UART0
    Uart0,
    /// UART1
    Uart1,
    /// UART2
    Uart2,
    /// UART3
    Uart3,
}

/// 停止位
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopBits {
    /// 1 停止位
    One,
    /// 2 停止位
    Two,
}

/// 校验位
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    /// 无校验
    None,
    /// 偶校验
    Even,
    /// 奇校验
    Odd,
}

/// GPIO 引脚号（用于 DE/RE 方向控制，D7 偏差）
pub type GpioPin = u32;

/// RS485 串口配置
#[derive(Clone, Debug)]
pub struct Rs485Config {
    /// 串口号（UART0/UART1/...）
    pub port: UartPort,
    /// 波特率（9600/19200/38400/115200 等）
    pub baud_rate: u32,
    /// 数据位（7/8）
    pub data_bits: u8,
    /// 停止位（1/2）
    pub stop_bits: StopBits,
    /// 校验位
    pub parity: Parity,
    /// 本机地址（Modbus 从站地址）
    pub local_addr: u8,
    /// 响应超时（ms）
    pub response_timeout_ms: u32,
    /// 帧间隔（ms，帧间静默时间，Modbus RTU 3.5 字符时间）
    pub frame_gap_ms: u32,
    /// DE/RE 方向控制 GPIO 引脚号（D7 偏差：使用引脚号而非 HalGpio 对象）
    pub de_re_pin: Option<GpioPin>,
    /// 发送前等待时间（μs，DE 使能后等待发送）
    pub pre_send_delay_us: u32,
    /// 发送后等待时间（μs，发送完成后保持 DE）
    pub post_send_delay_us: u32,
}

impl Default for Rs485Config {
    fn default() -> Self {
        Self {
            port: UartPort::Uart0,
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: StopBits::One,
            parity: Parity::None,
            local_addr: 1,
            response_timeout_ms: 1000,
            frame_gap_ms: 4, // 3.5 字符时间 @9600bps
            de_re_pin: None,
            pre_send_delay_us: 100,
            post_send_delay_us: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Rs485Config::default();
        assert_eq!(cfg.port, UartPort::Uart0);
        assert_eq!(cfg.baud_rate, 9600);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, StopBits::One);
        assert_eq!(cfg.parity, Parity::None);
        assert_eq!(cfg.local_addr, 1);
        assert_eq!(cfg.response_timeout_ms, 1000);
        assert_eq!(cfg.frame_gap_ms, 4);
        assert_eq!(cfg.de_re_pin, None);
        assert_eq!(cfg.pre_send_delay_us, 100);
        assert_eq!(cfg.post_send_delay_us, 100);
    }

    #[test]
    fn test_custom_config() {
        let cfg = Rs485Config {
            baud_rate: 115200,
            port: UartPort::Uart1,
            ..Default::default()
        };
        assert_eq!(cfg.baud_rate, 115200);
        assert_eq!(cfg.port, UartPort::Uart1);
        // 其余字段仍为默认值
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, StopBits::One);
    }

    #[test]
    fn test_uart_port_variants() {
        let ports = [
            UartPort::Uart0,
            UartPort::Uart1,
            UartPort::Uart2,
            UartPort::Uart3,
        ];
        for i in 0..ports.len() {
            for j in (i + 1)..ports.len() {
                assert_ne!(ports[i], ports[j]);
            }
        }
    }

    #[test]
    fn test_stop_bits_variants() {
        assert_ne!(StopBits::One, StopBits::Two);
    }

    #[test]
    fn test_parity_variants() {
        let parities = [Parity::None, Parity::Even, Parity::Odd];
        for i in 0..parities.len() {
            for j in (i + 1)..parities.len() {
                assert_ne!(parities[i], parities[j]);
            }
        }
    }

    #[test]
    fn test_de_re_pin_option() {
        let cfg_none = Rs485Config::default();
        assert!(cfg_none.de_re_pin.is_none());

        let cfg_with_pin = Rs485Config {
            de_re_pin: Some(42),
            ..Default::default()
        };
        assert_eq!(cfg_with_pin.de_re_pin, Some(42));
    }
}
