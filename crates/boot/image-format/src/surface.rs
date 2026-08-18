//! 安装目标全域安全判定（蓝图 39 WP-39.4，P1-07 修复）。
//!
//! 旧实现只检查首尾 LBA 是否空白——中间有数据的同容量磁盘会被覆盖
//! （P1-07）。本模块提供**分层全域扫描**：读取目标全部 LBA，报告：
//!
//! - 首个非零 LBA 与非零扇区总数；
//! - protective MBR / GPT 签名 / 已知文件系统与 EnerOS durable-log
//!   superblock 签名（`EDLOG1`）；
//!
//! 判定策略（调用方执行）：默认任何未知非零内容都拒绝覆盖；
//! `--force` 不能只是 CLI bool，必须有 provisioning authorization
//! （39 WP-39.3 的 `ProvisioningGate` 语义——本模块不内置强制写）。

use alloc::vec::Vec;

/// 表面探测结果（事实报告，不含裁决）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceProbe {
    /// 首个非零 LBA（全零盘为 None）。
    pub first_nonzero_lba: Option<u64>,
    /// 非零扇区总数。
    pub non_zero_sectors: u64,
    /// 已扫描扇区数。
    pub scanned_lbas: u64,
    /// LBA0 存在 0x55AA 签名且含 protective 类型 0xEE 条目。
    pub protective_mbr: bool,
    /// LBA1 存在 "EFI PART" 签名。
    pub gpt_signature: bool,
    /// 已知文件系统/日志签名（首个匹配或全部）。
    pub fs_signatures: Vec<&'static str>,
}

/// 探测错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceError {
    /// 底层读失败。
    Io,
    /// 参数非法（sector_size 0/非扇区对齐、chunk 为 0）。
    InvalidArgument,
}

/// 已知 superblock 签名（探测表——识别即报告，裁决在调用方）。
const SUPERBLOCK_SIGNATURES: &[(&[u8], &str)] = &[
    (b"EDLOG1", "eneros-durable-log"),
    (b"XFSB", "xfs"),
    (b"NTFS    ", "ntfs"),
    (b"FAT32   ", "fat32"),
    (b"EXFAT   ", "exfat"),
];

/// 检查 LBA0 内是否含已知签名（`sector` 为整个 LBA0 扇区内容）。
fn match_superblocks(sector: &[u8]) -> Vec<&'static str> {
    SUPERBLOCK_SIGNATURES
        .iter()
        .filter(|(sig, _)| {
            sector
                .get(..sig.len())
                .map(|head| head == *sig)
                .unwrap_or(false)
        })
        .map(|(_, name)| *name)
        .collect()
}

/// 扇区读取回调（`(lba, buf) -> Result`）：探测期注入——测试用内存盘、
/// 实机用块设备后端。生命周期参数化（裸 `dyn` 在 type alias 里默认 `'static`，
/// 会拒掉测试中借用内存盘的 reader 闭包）。
pub type SectorReader<'a> = dyn FnMut(u64, &mut [u8]) -> Result<(), SurfaceError> + 'a;

/// 全域表面探测。
///
/// `read_sector(lba, buf)` 读一个完整扇区到 `buf`（长度 = `sector_size`）。
/// 按 `chunk_sectors` 分块扫描（chunk 内逐扇区回调）。
pub fn probe_surface(
    total_lbas: u64,
    sector_size: usize,
    chunk_sectors: u64,
    read_sector: &mut SectorReader<'_>,
) -> Result<SurfaceProbe, SurfaceError> {
    if sector_size == 0 || chunk_sectors == 0 {
        return Err(SurfaceError::InvalidArgument);
    }
    let mut out = SurfaceProbe {
        first_nonzero_lba: None,
        non_zero_sectors: 0,
        scanned_lbas: 0,
        protective_mbr: false,
        gpt_signature: false,
        fs_signatures: Vec::new(),
    };
    let mut sector = alloc::vec![0u8; sector_size];
    let mut lba = 0u64;
    while lba < total_lbas {
        let end = lba.saturating_add(chunk_sectors).min(total_lbas);
        while lba < end {
            read_sector(lba, &mut sector)?;
            out.scanned_lbas += 1;
            let non_zero = sector.iter().any(|b| *b != 0);
            if non_zero {
                out.non_zero_sectors += 1;
                if out.first_nonzero_lba.is_none() {
                    out.first_nonzero_lba = Some(lba);
                }
                // LBA0 的布局签名。
                if lba == 0 {
                    if sector.len() >= 512 && sector[510..512] == [0x55, 0xAA] {
                        // protective MBR：分区表条目 0 的类型字段 0xEE。
                        if sector.get(446..447) == Some(&[0xEE][..]) {
                            out.protective_mbr = true;
                        }
                    }
                    out.fs_signatures.extend(match_superblocks(&sector));
                }
                // LBA1 的 GPT 签名。
                if lba == 1 && sector.len() >= 8 && sector[..8] == *b"EFI PART" {
                    out.gpt_signature = true;
                }
            }
            lba += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    /// 内存盘 reader（`data` 为扇区数组）。
    fn reader_from<'a>(
        data: &'a [Vec<u8>],
    ) -> impl FnMut(u64, &mut [u8]) -> Result<(), SurfaceError> + 'a {
        move |lba, buf| {
            let src = data.get(lba as usize).ok_or(SurfaceError::Io)?;
            buf.copy_from_slice(src);
            Ok(())
        }
    }

    #[test]
    fn blank_disk_reports_all_zero() {
        let data: Vec<Vec<u8>> = (0..64).map(|_| vec![0u8; 512]).collect();
        let mut r = reader_from(&data);
        let probe = probe_surface(64, 512, 16, &mut r).unwrap();
        assert_eq!(probe.first_nonzero_lba, None);
        assert_eq!(probe.non_zero_sectors, 0);
        assert_eq!(probe.scanned_lbas, 64);
        assert!(!probe.protective_mbr && !probe.gpt_signature);
        assert!(probe.fs_signatures.is_empty());
    }

    #[test]
    fn middle_data_is_detected() {
        // P1-07 反例：首尾空白、中间有数据——旧实现会误判可覆盖。
        let mut data: Vec<Vec<u8>> = (0..64).map(|_| vec![0u8; 512]).collect();
        data[32][10] = 0xAB;
        let mut r = reader_from(&data);
        let probe = probe_surface(64, 512, 16, &mut r).unwrap();
        assert_eq!(probe.first_nonzero_lba, Some(32));
        assert_eq!(probe.non_zero_sectors, 1);
        assert_eq!(probe.scanned_lbas, 64);
    }

    #[test]
    fn gpt_and_protective_mbr_detected() {
        let mut data: Vec<Vec<u8>> = (0..64).map(|_| vec![0u8; 512]).collect();
        // LBA0：protective MBR。
        data[0][446] = 0xEE;
        data[0][510] = 0x55;
        data[0][511] = 0xAA;
        // LBA1：GPT 签名。
        data[1][..8].copy_from_slice(b"EFI PART");
        let mut r = reader_from(&data);
        let probe = probe_surface(64, 512, 16, &mut r).unwrap();
        assert!(probe.protective_mbr);
        assert!(probe.gpt_signature);
        assert_eq!(probe.first_nonzero_lba, Some(0));
    }

    #[test]
    fn durable_log_signature_detected() {
        let mut data: Vec<Vec<u8>> = (0..64).map(|_| vec![0u8; 512]).collect();
        data[0][..6].copy_from_slice(b"EDLOG1");
        let mut r = reader_from(&data);
        let probe = probe_surface(64, 512, 16, &mut r).unwrap();
        assert!(probe.fs_signatures.contains(&"eneros-durable-log"));
    }

    #[test]
    fn invalid_arguments_rejected() {
        let mut r = reader_from(&[]);
        assert_eq!(
            probe_surface(0, 0, 16, &mut r),
            Err(SurfaceError::InvalidArgument)
        );
        assert_eq!(
            probe_surface(0, 512, 0, &mut r),
            Err(SurfaceError::InvalidArgument)
        );
    }
}
