use super::{
    GpioPin, HardwareAbstraction, HalError, I2cDevice, SerialPort, SpiConfig, SpiDevice,
};
#[cfg(target_os = "linux")]
use super::{FlowControl, GpioDirection, GpioEdge, Parity, SerialConfig};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

pub struct LinuxHal;

impl LinuxHal {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxHal {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareAbstraction for LinuxHal {
    fn open_serial(&self, path: &str, _baud: u32) -> Result<Box<dyn SerialPort>, HalError> {
        #[cfg(target_os = "linux")]
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?;
            Ok(Box::new(LinuxSerialPort {
                file,
                timeout_ms: None,
            }))
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(HalError::NotFound(format!(
                "serial device {} not available on this platform",
                path
            )))
        }
    }

    fn list_network_interfaces(&self) -> Result<Vec<String>, HalError> {
        #[cfg(target_os = "linux")]
        {
            let mut interfaces = Vec::new();
            for entry in std::fs::read_dir("/sys/class/net")? {
                let entry = entry?;
                if let Some(name) = entry.file_name().to_str() {
                    if name != "lo" {
                        interfaces.push(name.to_string());
                    }
                }
            }
            Ok(interfaces)
        }

        #[cfg(not(target_os = "linux"))]
        {
            Ok(Vec::new())
        }
    }

    fn open_gpio(&self, pin: u32) -> Result<Box<dyn GpioPin>, HalError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(LinuxGpioPin::open(pin)?))
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(HalError::NotFound(format!(
                "gpio {} not available on this platform",
                pin
            )))
        }
    }

    fn open_i2c(&self, bus: u32, addr: u16) -> Result<Box<dyn I2cDevice>, HalError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(LinuxI2cDevice::open(bus, addr)?))
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = addr;
            Err(HalError::NotFound(format!(
                "i2c-{} not available on this platform",
                bus
            )))
        }
    }

    fn open_spi(&self, path: &str, config: &SpiConfig) -> Result<Box<dyn SpiDevice>, HalError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(LinuxSpiDevice::open(path, config)?))
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(HalError::NotFound(format!(
                "spi {} not available on this platform",
                path
            )))
        }
    }
}

#[cfg(target_os = "linux")]
struct LinuxSerialPort {
    file: std::fs::File,
    timeout_ms: Option<u32>,
}

/// 将波特率映射到 libc speed_t 常量
#[cfg(target_os = "linux")]
fn baud_to_speed(baud: u32) -> Result<libc::speed_t, HalError> {
    match baud {
        9600 => Ok(libc::B9600),
        19200 => Ok(libc::B19200),
        38400 => Ok(libc::B38400),
        57600 => Ok(libc::B57600),
        115200 => Ok(libc::B115200),
        230400 => Ok(libc::B230400),
        460800 => Ok(libc::B460800),
        921600 => Ok(libc::B921600),
        _ => Err(HalError::InvalidConfig(format!(
            "unsupported baud rate: {}",
            baud
        ))),
    }
}

#[cfg(target_os = "linux")]
impl SerialPort for LinuxSerialPort {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HalError> {
        use std::io::Read;
        let n = self.file.read(buf)?;
        // 配置了超时且读到 0 字节 → 超时错误
        if n == 0 {
            if let Some(ms) = self.timeout_ms {
                return Err(HalError::Timeout(format!(
                    "serial read timeout ({}ms)",
                    ms
                )));
            }
        }
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize, HalError> {
        use std::io::Write;
        Ok(self.file.write(data)?)
    }

    fn configure(&mut self, config: &SerialConfig) -> Result<(), HalError> {
        use std::os::unix::io::AsRawFd;

        let fd = self.file.as_raw_fd();
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };

        // 读取当前 termios 配置
        if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }

        // 波特率
        let speed = baud_to_speed(config.baud_rate)?;
        unsafe { libc::cfsetspeed(&mut termios, speed) };

        // 数据位：先清除 CSIZE 掩码再设置
        termios.c_cflag &= !libc::CSIZE;
        termios.c_cflag |= match config.data_bits {
            5 => libc::CS5,
            6 => libc::CS6,
            7 => libc::CS7,
            8 => libc::CS8,
            n => return Err(HalError::InvalidConfig(format!("invalid data bits: {}", n))),
        };

        // 停止位
        match config.stop_bits {
            1 => termios.c_cflag &= !libc::CSTOPB,
            2 => termios.c_cflag |= libc::CSTOPB,
            n => return Err(HalError::InvalidConfig(format!("invalid stop bits: {}", n))),
        }

        // 校验位
        match config.parity {
            Parity::None => {
                termios.c_cflag &= !libc::PARENB;
            }
            Parity::Even => {
                termios.c_cflag |= libc::PARENB;
                termios.c_cflag &= !libc::PARODD;
            }
            Parity::Odd => {
                termios.c_cflag |= libc::PARENB;
                termios.c_cflag |= libc::PARODD;
            }
        }

        // 流控
        match config.flow_control {
            FlowControl::None => {
                termios.c_cflag &= !libc::CRTSCTS;
                termios.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
            }
            FlowControl::Hardware => {
                termios.c_cflag |= libc::CRTSCTS;
                termios.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
            }
            FlowControl::Software => {
                termios.c_cflag &= !libc::CRTSCTS;
                termios.c_iflag |= libc::IXON | libc::IXOFF | libc::IXANY;
            }
        }

        // 超时：VMIN/VTIME
        match config.timeout_ms {
            None => {
                // 阻塞模式：至少读到 1 字节
                termios.c_cc[libc::VMIN] = 1;
                termios.c_cc[libc::VTIME] = 0;
            }
            Some(ms) => {
                // 超时模式：VTIME 以百毫秒为单位
                termios.c_cc[libc::VMIN] = 0;
                termios.c_cc[libc::VTIME] = (ms / 100) as u8;
            }
        }

        // 应用配置
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }

        self.timeout_ms = config.timeout_ms;
        Ok(())
    }

    fn close(&mut self) {
        // File will be closed on drop
    }
}

// ============================================================================
// GPIO — sysfs 接口（/sys/class/gpio/）
// ============================================================================

#[cfg(target_os = "linux")]
struct LinuxGpioPin {
    base_path: PathBuf,
}

#[cfg(target_os = "linux")]
impl LinuxGpioPin {
    fn open(pin: u32) -> Result<Self, HalError> {
        let base_path = PathBuf::from(format!("/sys/class/gpio/gpio{}", pin));
        // 引脚未导出时尝试 export
        if !base_path.exists() {
            std::fs::write("/sys/class/gpio/export", pin.to_string()).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    HalError::PermissionDenied(format!("cannot export gpio {}", pin))
                } else {
                    HalError::Io(e)
                }
            })?;
        }
        Ok(Self { base_path })
    }

    fn write_attr(&self, name: &str, value: &str) -> Result<(), HalError> {
        std::fs::write(self.base_path.join(name), value)?;
        Ok(())
    }

    fn read_attr(&self, name: &str) -> Result<String, HalError> {
        Ok(std::fs::read_to_string(self.base_path.join(name))?)
    }
}

#[cfg(target_os = "linux")]
impl GpioPin for LinuxGpioPin {
    fn read(&self) -> Result<bool, HalError> {
        let value = self.read_attr("value")?;
        Ok(value.trim() == "1")
    }

    fn write(&mut self, value: bool) -> Result<(), HalError> {
        self.write_attr("value", if value { "1" } else { "0" })
    }

    fn set_direction(&mut self, dir: GpioDirection) -> Result<(), HalError> {
        let s = match dir {
            GpioDirection::Input => "in",
            GpioDirection::Output => "out",
        };
        self.write_attr("direction", s)
    }

    fn set_edge(&mut self, edge: GpioEdge) -> Result<(), HalError> {
        let s = match edge {
            GpioEdge::None => "none",
            GpioEdge::Rising => "rising",
            GpioEdge::Falling => "falling",
            GpioEdge::Both => "both",
        };
        self.write_attr("edge", s)
    }
}

// ============================================================================
// GPIO 事件监听 — sysfs edge + poll(POLLPRI)
// ============================================================================

/// GPIO 事件监听器（Linux sysfs poll 模式）
///
/// 通过 sysfs `edge` 文件配置中断边沿，再用 `poll()` 监听 `value` 文件的
/// `POLLPRI` 事件实现中断等待。
#[cfg(target_os = "linux")]
pub struct GpioEventMonitor {
    pin: u32,
    edge: GpioEdge,
    value_fd: std::fs::File,
}

#[cfg(target_os = "linux")]
impl GpioEventMonitor {
    /// 创建事件监听器（export + 设为输入 + set_edge + 打开 value 文件）
    pub fn new(pin: u32, edge: GpioEdge) -> Result<Self, HalError> {
        let gpio = LinuxGpioPin::open(pin)?;
        // 边沿检测要求引脚为输入方向
        gpio.set_direction(GpioDirection::Input)?;
        gpio.set_edge(edge)?;

        let value_path = format!("/sys/class/gpio/gpio{}/value", pin);
        let value_fd = std::fs::OpenOptions::new().read(true).open(&value_path)?;
        Ok(Self {
            pin,
            edge,
            value_fd,
        })
    }

    /// 引脚编号
    pub fn pin(&self) -> u32 {
        self.pin
    }

    /// 监听的边沿类型
    pub fn edge(&self) -> GpioEdge {
        self.edge
    }

    /// 阻塞等待下一次 GPIO 事件（返回 true=高电平, false=低电平）
    pub fn wait_event(&mut self) -> Result<bool, HalError> {
        use std::os::unix::io::AsRawFd;
        loop {
            let mut fds = [libc::pollfd {
                fd: self.value_fd.as_raw_fd(),
                events: libc::POLLPRI,
                revents: 0,
            }];
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                // 信号中断时重试
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(HalError::Io(err));
            }
            if ret > 0 && (fds[0].revents & libc::POLLPRI) != 0 {
                return self.read_value();
            }
        }
    }

    /// 设置超时等待（毫秒），返回 Ok(None) 表示超时
    pub fn wait_event_timeout(&mut self, timeout_ms: u32) -> Result<Option<bool>, HalError> {
        use std::os::unix::io::AsRawFd;
        let mut fds = [libc::pollfd {
            fd: self.value_fd.as_raw_fd(),
            events: libc::POLLPRI,
            revents: 0,
        }];
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms as libc::c_int) };
        if ret < 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }
        if ret == 0 {
            return Ok(None);
        }
        if (fds[0].revents & libc::POLLPRI) != 0 {
            Ok(Some(self.read_value()?))
        } else {
            Ok(None)
        }
    }

    /// 读取 value 文件并重新 seek 到文件头（供下次 poll 后读取）
    fn read_value(&mut self) -> Result<bool, HalError> {
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = [0u8; 1];
        self.value_fd.seek(SeekFrom::Start(0))?;
        self.value_fd.read_exact(&mut buf)?;
        Ok(buf[0] == b'1')
    }
}

// ============================================================================
// I2C — /dev/i2c-{bus} + ioctl(I2C_SLAVE)
// ============================================================================

#[cfg(target_os = "linux")]
struct LinuxI2cDevice {
    file: std::fs::File,
}

#[cfg(target_os = "linux")]
impl LinuxI2cDevice {
    /// I2C_SLAVE = 0x0703
    const I2C_SLAVE: u64 = 0x0703;

    fn open(bus: u32, addr: u16) -> Result<Self, HalError> {
        use std::os::unix::io::AsRawFd;
        let path = format!("/dev/i2c-{}", bus);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::ioctl(fd, Self::I2C_SLAVE, addr as libc::c_ulong) };
        if ret != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }
        Ok(Self { file })
    }
}

#[cfg(target_os = "linux")]
impl I2cDevice for LinuxI2cDevice {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HalError> {
        use std::io::Read;
        Ok(self.file.read(buf)?)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize, HalError> {
        use std::io::Write;
        Ok(self.file.write(data)?)
    }

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), HalError> {
        use std::io::{Read, Write};
        self.file.write_all(write)?;
        self.file.read_exact(read)?;
        Ok(())
    }
}

// ============================================================================
// SPI — /dev/spidev* + ioctl 配置/传输
// ============================================================================

/// spidev 全双工传输结构体（与内核 spi_ioc_transfer 布局一致）
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Default)]
struct SpiIocTransfer {
    tx_buf: u64,
    rx_buf: u64,
    len: u32,
    speed_hz: u32,
    delay_usecs: u16,
    bits_per_word: u8,
    cs_change: u8,
    tx_nbits: u8,
    rx_nbits: u8,
    word_delay_usecs: u8,
    pad: [u8; 3],
}

#[cfg(target_os = "linux")]
struct LinuxSpiDevice {
    file: std::fs::File,
}

#[cfg(target_os = "linux")]
impl LinuxSpiDevice {
    // SPI ioctl 常量（'k' = 0x6B）
    const SPI_IOC_WR_MODE: u64 = 0x40016B01;
    const SPI_IOC_WR_BITS_PER_WORD: u64 = 0x40016B03;
    const SPI_IOC_WR_MAX_SPEED_HZ: u64 = 0x40046B04;
    /// SPI_IOC_MESSAGE(1)，struct 大小 32 字节
    const SPI_IOC_MESSAGE_1: u64 = 0x40206B00;

    fn open(path: &str, config: &SpiConfig) -> Result<Self, HalError> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let fd = file.as_raw_fd();

        let mode = config.mode;
        let bits = config.bits_per_word;
        let speed = config.speed_hz;

        if unsafe { libc::ioctl(fd, Self::SPI_IOC_WR_MODE, &mode) } != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }
        if unsafe { libc::ioctl(fd, Self::SPI_IOC_WR_BITS_PER_WORD, &bits) } != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }
        if unsafe { libc::ioctl(fd, Self::SPI_IOC_WR_MAX_SPEED_HZ, &speed) } != 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }

        Ok(Self { file })
    }
}

#[cfg(target_os = "linux")]
impl SpiDevice for LinuxSpiDevice {
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> Result<(), HalError> {
        use std::os::unix::io::AsRawFd;
        let len = tx.len().min(rx.len());
        let mut tr = SpiIocTransfer::default();
        tr.tx_buf = tx.as_ptr() as u64;
        tr.rx_buf = rx.as_mut_ptr() as u64;
        tr.len = len as u32;
        let fd = self.file.as_raw_fd();
        if unsafe { libc::ioctl(fd, Self::SPI_IOC_MESSAGE_1, &tr) } < 0 {
            return Err(HalError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<(), HalError> {
        use std::io::Write;
        self.file.write_all(data)?;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<(), HalError> {
        use std::io::Read;
        self.file.read_exact(buf)?;
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn test_baud_rate_mapping() {
        assert_eq!(baud_to_speed(9600).unwrap(), libc::B9600);
        assert_eq!(baud_to_speed(19200).unwrap(), libc::B19200);
        assert_eq!(baud_to_speed(38400).unwrap(), libc::B38400);
        assert_eq!(baud_to_speed(57600).unwrap(), libc::B57600);
        assert_eq!(baud_to_speed(115200).unwrap(), libc::B115200);
        assert_eq!(baud_to_speed(230400).unwrap(), libc::B230400);
        assert_eq!(baud_to_speed(460800).unwrap(), libc::B460800);
        assert_eq!(baud_to_speed(921600).unwrap(), libc::B921600);
    }

    #[test]
    fn test_baud_rate_invalid() {
        assert!(baud_to_speed(12345).is_err());
        assert!(baud_to_speed(0).is_err());
    }
}
