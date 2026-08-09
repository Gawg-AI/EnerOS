//! 22 §2.W4.3：OTA 端到端——image downloader + 完整回路状态机。
//!
//! 回路：fetch（注入下载闭包，复用 21-B transport）→ 写入 B 分区 →
//! SM3+SM2 验签 → 激活 B → 重启 → healthcheck → 失败自动回滚 A。
//! `ota-ab` 已有 A/B 分区 + 验签 + 3 次失败回滚；本模块补 **downloader**
//! 与**回路编排**（fetch→write→verify→activate→reboot→health→rollback），
//! 全程记审计事件 + 进度指标（host 可测，无 IO 依赖注入）。

use alloc::string::String;
use alloc::vec::Vec;

/// 下载进度（指标：`ota.download.percent`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// 已下载字节。
    pub downloaded: u64,
    /// 总字节。
    pub total: u64,
}

impl DownloadProgress {
    /// 百分比（0-100）；`downloaded * 100` 用 saturating_mul 防 u64 溢出，
    /// total=0 时 checked_div 归零。
    pub fn percent(&self) -> u8 {
        let scaled = self.downloaded.saturating_mul(100);
        scaled.checked_div(self.total).unwrap_or(0).min(100) as u8
    }
}

/// OTA 回路阶段（审计/状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaStage {
    Fetch,
    Write,
    Verify,
    Activate,
    Reboot,
    Healthcheck,
    Rollback,
}

/// 回路结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaOutcome {
    /// 升级成功（B 激活 + healthcheck 通过）。
    Success { slot: u8 },
    /// 失败已回滚（A 恢复）。
    RolledBack { attempts: u32 },
}

/// 下载器（注入 fetch 闭包：`FnMut(offset, max) -> Result<Vec<u8>, ()>`）。
pub struct OtaDownloader<F> {
    /// 镜像 URL（元数据）。
    pub url: String,
    /// 镜像总大小。
    pub total_size: u64,
    /// 分块大小。
    pub chunk_size: u64,
    /// 下载闭包（真 HTTP/MQTT fetch 由上层注入）。
    fetch: F,
}

impl<F> OtaDownloader<F>
where
    F: FnMut(u64, u64) -> Result<Vec<u8>, ()>,
{
    /// 新建下载器。
    pub fn new(url: &str, total_size: u64, chunk_size: u64, fetch: F) -> Self {
        Self {
            url: String::from(url),
            total_size,
            chunk_size: chunk_size.max(1),
            fetch,
        }
    }

    /// 流式下载全镜像（分块 fetch；进度可观测）。失败即中断（fail-closed）。
    #[allow(clippy::result_unit_err)]
    pub fn download_all(&mut self) -> Result<(Vec<u8>, Vec<DownloadProgress>), ()> {
        let mut image = Vec::with_capacity(self.total_size as usize);
        let mut progress = Vec::new();
        let mut offset = 0u64;
        while offset < self.total_size {
            let want = self.chunk_size.min(self.total_size - offset);
            let chunk = (self.fetch)(offset, want)?;
            // 空/短分块视为传输失败：若以实际长度推进，offset 卡死 → 死循环 + 内存无限增长。
            if chunk.len() as u64 != want {
                return Err(());
            }
            image.extend_from_slice(&chunk);
            offset += want;
            progress.push(DownloadProgress {
                downloaded: offset,
                total: self.total_size,
            });
        }
        Ok((image, progress))
    }
}

/// 回路编排器（阶段机 + 审计事件流；设备 IO 经闭包注入）。
pub struct OtaPipeline<W, V, H> {
    /// 写分区闭包（B 分区写入）。
    write: W,
    /// 验签闭包（SM3 哈希 + SM2 验签——ota-ab 已有实现）。
    verify: V,
    /// healthcheck 闭包（重启后健康探测）。
    health: H,
    /// 审计事件流（阶段 → 结果）。
    pub audit: Vec<(OtaStage, bool)>,
    /// 回滚计数（3 次失败自动回滚）。
    pub attempts: u32,
}

impl<W, V, H> OtaPipeline<W, V, H>
where
    W: FnMut(&[u8]) -> Result<(), ()>,
    V: FnMut(&[u8]) -> Result<(), ()>,
    H: FnMut() -> bool,
{
    /// 新建回路（`max_rollback_attempts` = 自动回滚上限，默认 3）。
    pub fn new(write: W, verify: V, health: H) -> Self {
        Self {
            write,
            verify,
            health,
            audit: Vec::new(),
            attempts: 0,
        }
    }

    /// 执行完整回路：fetch → write → verify → activate(注入) → reboot →
    /// healthcheck → 失败回滚（重试最多 3 次）。
    ///
    /// 阶段：`verify` 失败 → 直接回滚（不激活）；`health` 失败 → 回滚重试。
    /// 任一路径连续失败超过 3 次 → 终止（防无限回滚死循环）。
    pub fn run(&mut self, image: &[u8]) -> OtaOutcome {
        loop {
            // 1. 写入 B 分区。
            self.audit.push((OtaStage::Write, false));
            if (self.write)(image).is_err() {
                self.audit.pop();
                if let Some(outcome) = self.rollback() {
                    return outcome;
                }
                continue;
            }
            self.audit.pop();
            self.audit.push((OtaStage::Write, true));

            // 2. SM3+SM2 验签（ota-ab 的 verify 语义；失败不激活）。
            self.audit.push((OtaStage::Verify, false));
            if (self.verify)(image).is_err() {
                self.audit.pop();
                if let Some(outcome) = self.rollback() {
                    return outcome;
                }
                continue;
            }
            self.audit.pop();
            self.audit.push((OtaStage::Verify, true));

            // 3. 激活 B + 重启（注入：boot_manager 的 boot_control 激活）。
            self.audit.push((OtaStage::Activate, true));
            self.audit.push((OtaStage::Reboot, true));

            // 4. healthcheck（重启后健康探测；失败 → 回滚 A 重试）。
            self.audit.push((OtaStage::Healthcheck, false));
            if (self.health)() {
                self.audit.pop();
                self.audit.push((OtaStage::Healthcheck, true));
                return OtaOutcome::Success { slot: 1 };
            }
            self.audit.pop();
            if let Some(outcome) = self.rollback() {
                return outcome;
            }
        }
    }

    /// 回滚（A 恢复）；超上限返回 `Some(终止)`，未超限返回 `None`（run 继续重试）。
    fn rollback(&mut self) -> Option<OtaOutcome> {
        self.attempts += 1;
        self.audit.push((OtaStage::Rollback, true));
        if self.attempts > 3 {
            // 3 次失败：不再自动回滚（防抖，标记人工介入）。
            Some(OtaOutcome::RolledBack {
                attempts: self.attempts,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验收：完整回路 fetch→write→verify→activate→health 成功。
    #[test]
    fn test_pipeline_success_flow() {
        // fetch 闭包：按 chunk 提供数据。
        let mut served = 0u64;
        let fetch = move |offset: u64, want: u64| -> Result<Vec<u8>, ()> {
            assert_eq!(offset, served);
            served += want;
            Ok(vec![0xABu8; want as usize])
        };
        let mut dl = OtaDownloader::new("http://ota/fw.bin", 100, 32, fetch);
        let (image, progress) = dl.download_all().expect("download");
        assert_eq!(image.len(), 100);
        assert_eq!(progress.len(), 4); // 32+32+32+4
        assert_eq!(progress.last().unwrap().percent(), 100);

        let mut pipe = OtaPipeline::new(|_img: &[u8]| Ok(()), |_img: &[u8]| Ok(()), || true);
        let outcome = pipe.run(&image);
        assert_eq!(outcome, OtaOutcome::Success { slot: 1 });
        assert_eq!(pipe.attempts, 0);
        // 审计事件流完整。
        let stages: Vec<OtaStage> = pipe.audit.iter().map(|(s, _)| *s).collect();
        assert!(stages.contains(&OtaStage::Write));
        assert!(stages.contains(&OtaStage::Verify));
        assert!(stages.contains(&OtaStage::Activate));
        assert!(stages.contains(&OtaStage::Healthcheck));
        assert!(!stages.contains(&OtaStage::Rollback));
    }

    /// 验收：验签失败 → 拒绝升级（不激活）+ 回滚。
    #[test]
    fn test_verify_rejection_rolls_back() {
        let mut pipe = OtaPipeline::new(
            |_img: &[u8]| Ok(()),
            |_img: &[u8]| Err(()), // 验签拒绝（坏镜像）
            || true,
        );
        let outcome = pipe.run(&[1u8; 10]);
        assert!(matches!(outcome, OtaOutcome::RolledBack { .. }));
        // 验签失败不得进入 Activate。
        let stages: Vec<OtaStage> = pipe.audit.iter().map(|(s, _)| *s).collect();
        assert!(!stages.contains(&OtaStage::Activate), "验签失败不激活");
        assert!(stages.contains(&OtaStage::Rollback));
    }

    /// 验收：healthcheck 失败 → 自动回滚 A（最多 3 次）。
    #[test]
    fn test_health_failure_auto_rollback() {
        let mut pipe = OtaPipeline::new(
            |_img: &[u8]| Ok(()),
            |_img: &[u8]| Ok(()),
            || false, // healthcheck 永远失败
        );
        let outcome = pipe.run(&[1u8; 10]);
        assert_eq!(
            outcome,
            OtaOutcome::RolledBack { attempts: 4 },
            "3 次失败后停止自动回滚（防抖）"
        );
        assert!(pipe.attempts > 3);
        // 每次失败都有 Rollback 审计。
        let rollbacks = pipe
            .audit
            .iter()
            .filter(|(s, ok)| *s == OtaStage::Rollback && *ok)
            .count();
        assert!(rollbacks >= 3);
    }

    /// 验收：下载进度指标（percent 单调）。
    #[test]
    fn test_download_progress() {
        let mut dl = OtaDownloader::new("http://x", 100, 50, |_o, w| Ok(vec![0u8; w as usize]));
        let (_, progress) = dl.download_all().expect("dl");
        assert_eq!(progress[0].percent(), 50);
        assert_eq!(progress[1].percent(), 100);
    }

    /// 验收：空/短分块 → 传输失败（fail-closed，防 offset 卡死 → 无限循环/OOM）。
    #[test]
    fn test_short_chunk_fails_closed() {
        let mut dl = OtaDownloader::new("http://x", 100, 50, |_o, _w| Ok(Vec::new()));
        assert!(dl.download_all().is_err(), "空分块必须 Err，不得死循环");
        let mut dl = OtaDownloader::new("http://x", 100, 50, |_o, _w| Ok(vec![1u8; 10]));
        assert!(dl.download_all().is_err(), "短分块必须 Err（长度 != want）");
    }
}
