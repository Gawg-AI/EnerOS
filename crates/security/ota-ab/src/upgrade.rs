//! 升级流程数据模型与 OTA 管理器 trait（蓝图 §4.2 / §4.4）。
//!
//! 定义升级错误 `OtaError`、升级元数据 `UpgradeMeta` 与七方法
//! `OtaManager` trait。具体状态机实现见 [`crate::ab_partition::AbPartitionOta`]。

use heapless::String as HString;

use crate::ab_partition::{Partition, PartitionSlot};

/// OTA 升级错误（蓝图 §4.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaError {
    /// 无空闲分区
    NoFreeSlot,
    /// 哈希不匹配（SM3 回读校验失败）
    HashMismatch,
    /// 签名无效（SM2 验签失败）
    SignatureInvalid,
    /// 写入失败，携带失败时的分区偏移
    WriteFailed {
        /// 写入失败时的分区偏移（字节）
        offset: u64,
    },
    /// 验证失败（状态不允许验证或激活）
    VerifyFailed,
    /// 无可回滚的激活分区
    NoActiveSlot,
    /// 启动失败
    BootFailed,
    /// 块损坏，携带块序号
    ChunkCorrupted {
        /// 损坏块的序号
        index: u32,
    },
    /// 版本降级被拒绝（CRY-006：版本单调防降级）
    DowngradeRejected,
}

/// 语义版本比较（CRY-006）：解析 "X.Y.Z" 三段数字并逐段比较。
///
/// 解析失败（非 "X.Y.Z" 形态）视为 `Equal`（不拦截、也不误判升级）——
/// 调用方对未知格式版本应另行校验；本函数只保证数字段单调。
pub fn compare_versions(a: &str, b: &str) -> core::cmp::Ordering {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.trim().split('.');
        let major = it.next()?.parse::<u64>().ok()?;
        let minor = it.next()?.parse::<u64>().ok()?;
        let patch = it.next()?.parse::<u64>().ok()?;
        if it.next().is_some() {
            return None; // 多于三段
        }
        Some((major, minor, patch))
    }
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => {
            if x.0 != y.0 {
                x.0.cmp(&y.0)
            } else if x.1 != y.1 {
                x.1.cmp(&y.1)
            } else {
                x.2.cmp(&y.2)
            }
        }
        _ => core::cmp::Ordering::Equal,
    }
}

/// 版本单调检查（CRY-006）：target 不得低于 current（防降级）。
///
/// `target < current` → `Err(OtaError::DowngradeRejected)`；
/// 相等/升级/不可解析版本均放行（不可解析由调用方另行把关）。
pub fn check_monotonic(target: &str, current: &str) -> Result<(), OtaError> {
    if compare_versions(target, current) == core::cmp::Ordering::Less {
        return Err(OtaError::DowngradeRejected);
    }
    Ok(())
}

/// 升级元数据（蓝图 §4.1）。
#[derive(Debug, Clone)]
pub struct UpgradeMeta {
    /// 目标镜像版本号（≤32 字节）
    pub target_version: HString<32>,
    /// 镜像大小（字节）
    pub image_size: u64,
    /// 镜像 SM3 哈希
    pub image_hash: [u8; 32],
    /// 镜像 SM2 签名（r ‖ s，64 字节）
    pub image_sig: [u8; 64],
    /// 镜像下载地址（≤128 字节）
    pub download_url: HString<128>,
    /// 分块大小（字节）
    pub chunk_size: u32,
    /// 总分块数
    pub total_chunks: u32,
    /// 已下载分块数
    pub downloaded_chunks: u32,
}

/// OTA 管理器 trait（蓝图 §4.2 七方法）。
pub trait OtaManager {
    /// 准备升级：选择非激活分区置 Writing 并回填元数据，返回目标槽位。
    fn prepare_upgrade(&mut self, meta: UpgradeMeta) -> Result<PartitionSlot, OtaError>;

    /// 写入一个分块：仅 Writing 态接收；按槽位基址 + offset 写盘；
    /// `downloaded_chunks` 推进，达到 `total_chunks` 置 Written。
    fn write_chunk(
        &mut self,
        slot: PartitionSlot,
        offset: u64,
        data: &[u8],
    ) -> Result<(), OtaError>;

    /// 校验：回读镜像做 SM3 哈希比对 + SM2 签名验证；双过置 Verified。
    fn verify(&mut self, slot: PartitionSlot) -> Result<bool, OtaError>;

    /// 激活：仅 Verified 态可切换；新分区 Active、旧分区保留 Verified 备用。
    fn activate(&mut self, slot: PartitionSlot) -> Result<(), OtaError>;

    /// 回滚：切回非激活（旧版本）分区，返回回滚到的槽位。
    fn rollback(&mut self) -> Result<PartitionSlot, OtaError>;

    /// 获取当前激活槽位。
    fn get_active_slot(&self) -> PartitionSlot;

    /// 获取双分区快照。
    fn get_partitions(&self) -> [Partition; 2];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cry006_compare_versions() {
        use core::cmp::Ordering;
        assert_eq!(compare_versions("0.123.0", "0.122.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.123.0", "0.123.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0", "0.999.999"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.0", "0.10.0"), Ordering::Less);
        // 不可解析 → Equal（不误判）
        assert_eq!(compare_versions("dev", "0.1.0"), Ordering::Equal);
    }

    #[test]
    fn test_cry006_check_monotonic_rejects_downgrade() {
        assert_eq!(
            check_monotonic("0.122.0", "0.123.0"),
            Err(OtaError::DowngradeRejected),
            "降级必须拒绝"
        );
        assert!(check_monotonic("0.124.0", "0.123.0").is_ok(), "升级放行");
        assert!(check_monotonic("0.123.0", "0.123.0").is_ok(), "同版本放行");
        // 不可解析 target 放行（由调用方另行把关，不静默拒绝升级）
        assert!(check_monotonic("dev", "0.123.0").is_ok());
    }
}
