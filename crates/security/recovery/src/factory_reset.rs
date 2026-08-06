//! FactoryReset 恢复出厂模块 (v0.125.0 Task 4)。
//!
//! 管理配置键值集合（heapless 定容 [`CONFIG_CAPACITY`] 条）与出厂默认集：
//! - [`FactoryReset::clear_configs`] — 清除全部用户配置
//! - [`FactoryReset::write_defaults`] — 写入出厂默认集
//! - [`FactoryReset::backup_configs`] — 配置序列化为字节流（条目流 + 末尾 4B CRC32 大端）
//! - [`FactoryReset::restore_configs`] — 校验 CRC 并还原（损坏 →
//!   [`RecoveryError::ConfigBackupFailed`]；先校验后落地，不做部分写入）

use alloc::vec::Vec;

use eneros_storage::crc32;

use crate::RecoveryError;

/// 配置存储容量上限（条目数）。
pub const CONFIG_CAPACITY: usize = 32;

/// 单条配置键值对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    /// 配置键（≤32 字节，超长截断）
    pub key: heapless::String<32>,
    /// 配置值（≤128 字节，超长截断）
    pub value: heapless::String<128>,
}

/// 将字符串截断为 heapless 定容字符串。
///
/// 按 UTF-8 字符边界截断（不截断半个字符），容量不足时静默截断，不 panic。
fn truncate_heapless<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out = heapless::String::<N>::new();
    for ch in s.chars() {
        if out.len() + ch.len_utf8() > N {
            break;
        }
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}

/// FactoryReset 恢复出厂：配置清除 / 出厂默认 / 备份 / 恢复。
#[derive(Debug, Clone)]
pub struct FactoryReset {
    /// 配置键值集合（定容 [`CONFIG_CAPACITY`] 条）
    configs: heapless::Vec<ConfigEntry, CONFIG_CAPACITY>,
}

impl FactoryReset {
    /// 出厂默认配置集（设备名 / 采样周期 / 上报周期 / 日志级别）。
    pub const fn factory_defaults() -> [(&'static str, &'static str); 4] {
        [
            ("device_name", "eneros-edge"),
            ("sample_interval_ms", "100"),
            ("report_interval_ms", "1000"),
            ("log_level", "info"),
        ]
    }

    /// 创建空配置集。
    pub fn new() -> Self {
        Self {
            configs: heapless::Vec::new(),
        }
    }

    /// 清空全部配置。
    pub fn clear_configs(&mut self) {
        self.configs.clear();
    }

    /// 写入出厂默认集（先清空再写入）。
    ///
    /// 默认集 4 项远小于容量，不会失败；保留 `Result` 返回与蓝图接口一致。
    pub fn write_defaults(&mut self) -> Result<(), RecoveryError> {
        self.clear_configs();
        for (key, value) in Self::factory_defaults() {
            self.set(key, value);
        }
        Ok(())
    }

    /// 写入 / 更新一条配置。
    ///
    /// - 键已存在则原地更新值；
    /// - 键不存在且未满则插入；已满则忽略新键（不 panic）；
    /// - key > 32 字节 / value > 128 字节按字符边界截断（不 panic）。
    pub fn set(&mut self, key: &str, value: &str) {
        let key = truncate_heapless::<32>(key);
        let value = truncate_heapless::<128>(value);
        // 已存在则原地更新
        if let Some(entry) = self.configs.iter_mut().find(|e| e.key == key) {
            entry.value = value;
            return;
        }
        // 不存在且未满则插入；已满忽略新键（容量已检查，push 不会失败，
        // 异常失败时静默忽略，不 panic）
        if !self.configs.is_full() {
            let _ = self.configs.push(ConfigEntry { key, value });
        }
    }

    /// 查询配置值（按精确键匹配）。
    pub fn get(&self, key: &str) -> Option<&heapless::String<128>> {
        self.configs
            .iter()
            .find(|entry| entry.key.as_str() == key)
            .map(|entry| &entry.value)
    }

    /// 配置条目数。
    pub fn len(&self) -> usize {
        self.configs.len()
    }

    /// 配置集是否为空。
    pub fn is_empty(&self) -> bool {
        self.configs.is_empty()
    }

    /// 只读访问配置集合（测试对拍用）。
    pub fn configs(&self) -> &heapless::Vec<ConfigEntry, CONFIG_CAPACITY> {
        &self.configs
    }

    /// 备份配置为字节流。
    ///
    /// 序列化格式：每条目 `[key_len: u8][key bytes][value_len: u8][value bytes]`
    /// 顺序拼接，末尾追加 4 字节 CRC32（大端，覆盖前面全部条目字节）。
    pub fn backup_configs(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in self.configs.iter() {
            out.push(entry.key.len() as u8);
            out.extend_from_slice(entry.key.as_bytes());
            out.push(entry.value.len() as u8);
            out.extend_from_slice(entry.value.as_bytes());
        }
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    /// 从字节流恢复配置。
    ///
    /// 先校验 CRC 再逐条目解析到临时集，全部合法才替换现有配置（先校验后落地，
    /// 损坏流不造成部分污染）；key/value 超 heapless 容量时按字符边界截断。
    ///
    /// # 错误
    /// 长度 < 4 / CRC 不符 / 条目长度越界 / 非法 UTF-8 / 条目数超容量
    /// → [`RecoveryError::ConfigBackupFailed`]
    pub fn restore_configs(&mut self, bytes: &[u8]) -> Result<(), RecoveryError> {
        if bytes.len() < 4 {
            return Err(RecoveryError::ConfigBackupFailed);
        }
        let (payload, crc_bytes) = bytes.split_at(bytes.len() - 4);
        let expected = u32::from_be_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
        if crc32(payload) != expected {
            return Err(RecoveryError::ConfigBackupFailed);
        }

        // 逐条目解析到临时集，全部通过才落地替换（先校验后落地）
        let mut parsed = heapless::Vec::<ConfigEntry, CONFIG_CAPACITY>::new();
        let mut pos = 0usize;
        while pos < payload.len() {
            let key_len = payload[pos] as usize;
            pos += 1;
            if pos + key_len > payload.len() {
                return Err(RecoveryError::ConfigBackupFailed);
            }
            let key_bytes = &payload[pos..pos + key_len];
            pos += key_len;
            if pos >= payload.len() {
                // 缺 value_len 字节
                return Err(RecoveryError::ConfigBackupFailed);
            }
            let value_len = payload[pos] as usize;
            pos += 1;
            if pos + value_len > payload.len() {
                return Err(RecoveryError::ConfigBackupFailed);
            }
            let value_bytes = &payload[pos..pos + value_len];
            pos += value_len;

            let key =
                core::str::from_utf8(key_bytes).map_err(|_| RecoveryError::ConfigBackupFailed)?;
            let value =
                core::str::from_utf8(value_bytes).map_err(|_| RecoveryError::ConfigBackupFailed)?;
            let entry = ConfigEntry {
                key: truncate_heapless::<32>(key),
                value: truncate_heapless::<128>(value),
            };
            if parsed.push(entry).is_err() {
                // 条目数超容量
                return Err(RecoveryError::ConfigBackupFailed);
            }
        }

        self.configs = parsed;
        Ok(())
    }
}

impl Default for FactoryReset {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RST1：清空用户配置 + 写入出厂默认集，配置集合等于出厂默认集。
    #[test]
    fn rst1_clear_and_write_defaults() {
        let mut fr = FactoryReset::new();
        fr.set("site_id", "A01");
        fr.set("owner", "ops");
        fr.set("region", "east");
        assert_eq!(fr.len(), 3);

        fr.clear_configs();
        assert!(fr.is_empty());

        fr.write_defaults().expect("写入出厂默认集失败");
        assert_eq!(fr.len(), 4);
        assert_eq!(
            fr.get("device_name").map(|v| v.as_str()),
            Some("eneros-edge")
        );
        assert_eq!(fr.get("log_level").map(|v| v.as_str()), Some("info"));
        assert_eq!(
            fr.get("sample_interval_ms").map(|v| v.as_str()),
            Some("100")
        );
        assert_eq!(
            fr.get("report_interval_ms").map(|v| v.as_str()),
            Some("1000")
        );
    }

    /// RST2：备份 → 修改 → 恢复，用户配置完整还原。
    #[test]
    fn rst2_backup_modify_restore() {
        let mut fr = FactoryReset::new();
        fr.set("k1", "v1");
        fr.set("k2", "v2");
        let snapshot = fr.configs().clone();
        let backup = fr.backup_configs();

        fr.clear_configs();
        fr.set("k1", "changed");
        assert_eq!(fr.get("k1").map(|v| v.as_str()), Some("changed"));
        assert!(fr.get("k2").is_none());

        fr.restore_configs(&backup).expect("恢复备份失败");
        assert_eq!(fr.get("k1").map(|v| v.as_str()), Some("v1"));
        assert_eq!(fr.get("k2").map(|v| v.as_str()), Some("v2"));
        // 整体对拍：恢复后配置集合与备份前完全一致
        assert_eq!(fr.configs(), &snapshot);
    }

    /// RST3：备份字节流损坏（数据区 / CRC 区）→ ConfigBackupFailed。
    #[test]
    fn rst3_corrupted_backup_rejected() {
        let mut fr = FactoryReset::new();
        fr.set("k1", "v1");
        fr.set("k2", "v2");
        let backup = fr.backup_configs();

        // 数据区改 1 字节 → CRC 校验失败
        let mut bad_payload = backup.clone();
        bad_payload[1] ^= 0xFF;
        assert_eq!(
            fr.restore_configs(&bad_payload),
            Err(RecoveryError::ConfigBackupFailed)
        );

        // CRC 区改 1 字节 → CRC 校验失败
        let mut bad_crc = backup;
        let last = bad_crc.len() - 1;
        bad_crc[last] ^= 0xFF;
        assert_eq!(
            fr.restore_configs(&bad_crc),
            Err(RecoveryError::ConfigBackupFailed)
        );
    }

    /// RST4：过短流与 key_len 越界的非法流 → ConfigBackupFailed。
    #[test]
    fn rst4_short_and_malformed_stream_rejected() {
        let mut fr = FactoryReset::new();

        // 空 slice 与 3 字节 slice（不足 CRC 长度）
        assert_eq!(
            fr.restore_configs(&[]),
            Err(RecoveryError::ConfigBackupFailed)
        );
        assert_eq!(
            fr.restore_configs(&[1, 2, 3]),
            Err(RecoveryError::ConfigBackupFailed)
        );

        // 构造 key_len 越界的非法流：声明 key_len=10 但仅有 2 字节数据
        let payload = [10u8, b'a', b'b'];
        let crc = crc32(&payload);
        let mut stream = Vec::from(&payload[..]);
        stream.extend_from_slice(&crc.to_be_bytes());
        assert_eq!(
            fr.restore_configs(&stream),
            Err(RecoveryError::ConfigBackupFailed)
        );
    }

    /// RST5：超长键值截断存储 + 容量满忽略新键，均不 panic。
    #[test]
    fn rst5_truncation_and_capacity_limit() {
        let mut fr = FactoryReset::new();

        // 超长 key（40 字节）/ 超长 value（200 字节）→ 截断存储
        let long_key = "k".repeat(40);
        let long_value = "v".repeat(200);
        fr.set(&long_key, &long_value);
        let stored = fr.get(&"k".repeat(32)).expect("截断键应可查询");
        assert_eq!(stored.as_str(), "v".repeat(128).as_str());
        // 完整超长键查不到（已按截断键存储）
        assert!(fr.get(&long_key).is_none());

        // 填满 32 条后再 set 新键 → 忽略且 len 不变
        fr.clear_configs();
        for i in 0..CONFIG_CAPACITY {
            fr.set(&format!("key{i:02}"), "x");
        }
        assert_eq!(fr.len(), CONFIG_CAPACITY);
        fr.set("overflow_key", "y");
        assert_eq!(fr.len(), CONFIG_CAPACITY);
        assert!(fr.get("overflow_key").is_none());
        // 已存在键仍可更新（不受容量满影响）
        fr.set("key00", "updated");
        assert_eq!(fr.get("key00").map(|v| v.as_str()), Some("updated"));
    }

    /// RST6：先校验后落地——前半合法后半损坏的流恢复失败且原配置不被污染。
    #[test]
    fn rst6_atomic_restore_on_partial_corruption() {
        let mut fr = FactoryReset::new();
        fr.set("keep", "original");

        // 构造流：第 1 条合法 + 第 2 条 value_len 越界
        //（整体 CRC 正确，损坏发生在条目解析层）
        let mut payload = Vec::new();
        payload.push(2u8);
        payload.extend_from_slice(b"k1");
        payload.push(2u8);
        payload.extend_from_slice(b"v1");
        // 损坏条目：value_len=50 但其后仅有 1 字节
        payload.push(2u8);
        payload.extend_from_slice(b"k2");
        payload.push(50u8);
        payload.push(b'x');
        let crc = crc32(&payload);
        payload.extend_from_slice(&crc.to_be_bytes());

        assert_eq!(
            fr.restore_configs(&payload),
            Err(RecoveryError::ConfigBackupFailed)
        );
        // 原配置未被部分写入污染
        assert_eq!(fr.len(), 1);
        assert_eq!(fr.get("keep").map(|v| v.as_str()), Some("original"));
        assert!(fr.get("k1").is_none());
    }
}
