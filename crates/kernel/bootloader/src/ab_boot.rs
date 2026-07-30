//! A/B 分区启动选择（v0.163.0 蓝图 §4.3）。
//!
//! Bootloader 启动时读取 [`BootConfig`]，根据 `retry_count` 与 `max_retries`
//! 决定启动哪个分区：
//! - 正常路径：启动 Active 分区，成功后更新 LastGood
//! - 失败路径：retry_count 超限自动切换到 LastGood 分区
//!
//! # 启动流程（蓝图 §4.3 Mermaid 图）
//! ```text
//! Bootloader 启动 → 读取 BootConfig → retry_count < max?
//!   是 → 启动 Active 分区 → 成功? → 更新 LastGood → 正常运行
//!   否 → 切换到 LastGood 分区 → 启动 → 成功? → ...
//! ```
//!
//! # no_std 合规
//! 本模块不使用 `std::*`，适用于 Bootloader 裸机环境。

/// 分区标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partition {
    /// A 分区
    A,
    /// B 分区
    B,
}

impl Partition {
    /// 获取分区名称。
    pub fn name(&self) -> &'static str {
        match self {
            Partition::A => "A",
            Partition::B => "B",
        }
    }

    /// 获取另一个分区。
    pub fn other(&self) -> Partition {
        match self {
            Partition::A => Partition::B,
            Partition::B => Partition::A,
        }
    }

    /// 编码为字节。
    pub fn encode(&self) -> u8 {
        match self {
            Partition::A => 0,
            Partition::B => 1,
        }
    }

    /// 从字节解码。
    pub fn decode(byte: u8) -> Option<Partition> {
        match byte {
            0 => Some(Partition::A),
            1 => Some(Partition::B),
            _ => None,
        }
    }
}

/// 启动配置（持久化到 eMMC 保留块）。
///
/// 蓝图 §4.1 BootConfig 结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootConfig {
    /// 当前活动分区
    pub active: Partition,
    /// 上次成功启动分区
    pub last_good: Partition,
    /// 当前分区重试次数
    pub retry_count: u8,
    /// 最大重试次数（默认 3）
    pub max_retries: u8,
}

/// BootConfig 序列化长度（字节）。
/// 格式：magic(4) + active(1) + last_good(1) + retry_count(1) + max_retries(1) + crc32(4) = 12
pub const BOOT_CONFIG_LEN: usize = 12;

/// BootConfig 魔数 "EBCF"（EnerOS Boot ConFig）。
const BOOT_CONFIG_MAGIC: [u8; 4] = *b"EBCF";

impl BootConfig {
    /// 创建默认启动配置（A 分区活动）。
    pub fn default_config() -> Self {
        Self {
            active: Partition::A,
            last_good: Partition::A,
            retry_count: 0,
            max_retries: 3,
        }
    }

    /// 序列化为字节数组。
    ///
    /// 格式：magic(4) + active(1) + last_good(1) + retry_count(1) + max_retries(1) + crc32(4)
    pub fn serialize(&self) -> [u8; BOOT_CONFIG_LEN] {
        let mut buf = [0u8; BOOT_CONFIG_LEN];
        buf[0..4].copy_from_slice(&BOOT_CONFIG_MAGIC);
        buf[4] = self.active.encode();
        buf[5] = self.last_good.encode();
        buf[6] = self.retry_count;
        buf[7] = self.max_retries;
        // CRC32 覆盖前 8 字节
        let crc = crc32_simple(&buf[..8]);
        buf[8..12].copy_from_slice(&crc.to_be_bytes());
        buf
    }

    /// 从字节数组反序列化。
    ///
    /// 校验魔数与 CRC32，失败返回 `None`。
    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        if buf.len() < BOOT_CONFIG_LEN {
            return None;
        }
        if buf[0..4] != BOOT_CONFIG_MAGIC {
            return None;
        }
        // 校验 CRC32
        let mut crc_bytes = [0u8; 4];
        crc_bytes.copy_from_slice(&buf[8..12]);
        let expected_crc = u32::from_be_bytes(crc_bytes);
        if crc32_simple(&buf[..8]) != expected_crc {
            return None;
        }
        let active = Partition::decode(buf[4])?;
        let last_good = Partition::decode(buf[5])?;
        Some(Self {
            active,
            last_good,
            retry_count: buf[6],
            max_retries: buf[7],
        })
    }

    /// 判断是否应切换到 LastGood 分区。
    ///
    /// 当 retry_count >= max_retries 时返回 true。
    pub fn should_fallback(&self) -> bool {
        self.retry_count >= self.max_retries
    }

    /// 获取应启动的分区。
    ///
    /// 如果 retry_count 超限，返回 LastGood；否则返回 Active。
    pub fn get_boot_partition(&self) -> Partition {
        if self.should_fallback() {
            self.last_good
        } else {
            self.active
        }
    }

    /// 启动失败时递增重试计数。
    ///
    /// 如果超限，自动切换到 LastGood 分区并清零计数。
    /// 返回应启动的分区。
    pub fn on_boot_failed(&mut self) -> Partition {
        self.retry_count += 1;
        if self.retry_count >= self.max_retries {
            // 切换到 LastGood
            self.active = self.last_good;
            self.retry_count = 0;
        }
        self.get_boot_partition()
    }

    /// 启动成功时更新 LastGood。
    pub fn on_boot_success(&mut self) {
        self.last_good = self.active;
        self.retry_count = 0;
    }

    /// 设置下次启动分区。
    pub fn set_next_boot(&mut self, partition: Partition) {
        self.active = partition;
        self.retry_count = 0;
    }
}

/// A/B 启动选择器。
///
/// Bootloader 使用此结构决定启动哪个分区。
pub struct AbBootSelector {
    /// 启动配置
    config: BootConfig,
}

impl AbBootSelector {
    /// 从启动配置创建选择器。
    pub fn new(config: BootConfig) -> Self {
        Self { config }
    }

    /// 从字节数组加载配置创建选择器。
    ///
    /// 解析失败时使用默认配置（A 分区）。
    pub fn from_bytes(buf: &[u8]) -> Self {
        let config = BootConfig::deserialize(buf).unwrap_or_else(BootConfig::default_config);
        Self { config }
    }

    /// 获取当前配置。
    pub fn config(&self) -> &BootConfig {
        &self.config
    }

    /// 获取可变配置引用。
    pub fn config_mut(&mut self) -> &mut BootConfig {
        &mut self.config
    }

    /// 选择启动分区。
    ///
    /// 返回应启动的分区与内核加载地址。
    pub fn select_partition(&self) -> (Partition, u64) {
        let partition = self.config.get_boot_partition();
        let load_addr = partition_load_addr(partition);
        (partition, load_addr)
    }

    /// 启动失败回调。
    ///
    /// 返回切换后应启动的分区。
    pub fn on_boot_failed(&mut self) -> Partition {
        self.config.on_boot_failed()
    }

    /// 启动成功回调。
    pub fn on_boot_success(&mut self) {
        self.config.on_boot_success()
    }

    /// 序列化配置（用于持久化）。
    pub fn serialize(&self) -> [u8; BOOT_CONFIG_LEN] {
        self.config.serialize()
    }
}

/// A 分区内核加载地址（蓝图 §4.1：1MB 偏移）。
pub const PARTITION_A_LOAD_ADDR: u64 = 0x4900_0000;

/// B 分区内核加载地址（蓝图 §4.1：128MB 偏移）。
pub const PARTITION_B_LOAD_ADDR: u64 = 0x5100_0000;

/// 获取分区的内核加载地址。
pub fn partition_load_addr(partition: Partition) -> u64 {
    match partition {
        Partition::A => PARTITION_A_LOAD_ADDR,
        Partition::B => PARTITION_B_LOAD_ADDR,
    }
}

/// 简单 CRC32 实现（IEEE 802.3）。
///
/// Bootloader 环境无外部依赖，使用内联实现。
fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// ABT1：默认配置字段验证。
    #[test]
    fn abt1_default_config() {
        let config = BootConfig::default_config();
        assert_eq!(config.active, Partition::A);
        assert_eq!(config.last_good, Partition::A);
        assert_eq!(config.retry_count, 0);
        assert_eq!(config.max_retries, 3);
        assert!(!config.should_fallback());
        assert_eq!(config.get_boot_partition(), Partition::A);
    }

    /// ABT2：序列化/反序列化往返。
    #[test]
    fn abt2_serialize_round_trip() {
        let config = BootConfig {
            active: Partition::B,
            last_good: Partition::A,
            retry_count: 2,
            max_retries: 3,
        };
        let bytes = config.serialize();
        assert_eq!(bytes.len(), BOOT_CONFIG_LEN);

        let restored = BootConfig::deserialize(&bytes).expect("反序列化应成功");
        assert_eq!(restored, config);
    }

    /// ABT3：魔数错误拒绝。
    #[test]
    fn abt3_bad_magic_rejected() {
        let config = BootConfig::default_config();
        let mut bytes = config.serialize();
        bytes[0] = b'X'; // 篡改魔数
        assert!(BootConfig::deserialize(&bytes).is_none());
    }

    /// ABT4：CRC 错误拒绝。
    #[test]
    fn abt4_bad_crc_rejected() {
        let config = BootConfig::default_config();
        let mut bytes = config.serialize();
        bytes[5] ^= 0xFF; // 篡改数据
        assert!(BootConfig::deserialize(&bytes).is_none());
    }

    /// ABT5：重试计数超限自动切换。
    #[test]
    fn abt5_retry_exceeded_fallback() {
        let mut config = BootConfig {
            active: Partition::B,
            last_good: Partition::A,
            retry_count: 0,
            max_retries: 3,
        };

        // 前 2 次失败：未超限
        assert_eq!(config.on_boot_failed(), Partition::B);
        assert_eq!(config.retry_count, 1);
        assert_eq!(config.on_boot_failed(), Partition::B);
        assert_eq!(config.retry_count, 2);

        // 第 3 次失败：超限，切换到 LastGood (A)
        assert_eq!(config.on_boot_failed(), Partition::A);
        assert_eq!(config.active, Partition::A);
        assert_eq!(config.retry_count, 0);
    }

    /// ABT6：启动成功更新 LastGood。
    #[test]
    fn abt6_boot_success_updates_last_good() {
        let mut config = BootConfig {
            active: Partition::B,
            last_good: Partition::A,
            retry_count: 2,
            max_retries: 3,
        };
        config.on_boot_success();
        assert_eq!(config.last_good, Partition::B);
        assert_eq!(config.retry_count, 0);
    }

    /// ABT7：AbBootSelector 分区选择。
    #[test]
    fn abt7_selector_partition_selection() {
        let config = BootConfig {
            active: Partition::B,
            last_good: Partition::A,
            retry_count: 0,
            max_retries: 3,
        };
        let selector = AbBootSelector::new(config);
        let (partition, addr) = selector.select_partition();
        assert_eq!(partition, Partition::B);
        assert_eq!(addr, PARTITION_B_LOAD_ADDR);
    }

    /// ABT8：AbBootSelector 失败回退。
    #[test]
    fn abt8_selector_fallback_on_failure() {
        let config = BootConfig {
            active: Partition::B,
            last_good: Partition::A,
            retry_count: 2,
            max_retries: 3,
        };
        let mut selector = AbBootSelector::new(config);

        // 第 3 次失败触发回退
        let partition = selector.on_boot_failed();
        assert_eq!(partition, Partition::A);
        assert_eq!(selector.config().active, Partition::A);
    }

    /// ABT9：Partition 编解码。
    #[test]
    fn abt9_partition_encode_decode() {
        assert_eq!(Partition::A.encode(), 0);
        assert_eq!(Partition::B.encode(), 1);
        assert_eq!(Partition::decode(0), Some(Partition::A));
        assert_eq!(Partition::decode(1), Some(Partition::B));
        assert_eq!(Partition::decode(2), None);
    }

    /// ABT10：Partition other() 方法。
    #[test]
    fn abt10_partition_other() {
        assert_eq!(Partition::A.other(), Partition::B);
        assert_eq!(Partition::B.other(), Partition::A);
    }

    /// ABT11：set_next_boot 清零重试计数。
    #[test]
    fn abt11_set_next_boot_resets_retry() {
        let mut config = BootConfig {
            active: Partition::A,
            last_good: Partition::A,
            retry_count: 2,
            max_retries: 3,
        };
        config.set_next_boot(Partition::B);
        assert_eq!(config.active, Partition::B);
        assert_eq!(config.retry_count, 0);
    }

    /// ABT12：from_bytes 损坏数据回退默认。
    #[test]
    fn abt12_from_bytes_corrupted_fallback() {
        let corrupted = [0xFF; BOOT_CONFIG_LEN];
        let selector = AbBootSelector::from_bytes(&corrupted);
        assert_eq!(selector.config().active, Partition::A);
        assert_eq!(selector.config().last_good, Partition::A);
    }
}
