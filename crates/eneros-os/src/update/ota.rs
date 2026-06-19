//! OTA 更新管理器（v0.22.0）
//!
//! 完整 OTA 流程：下载 → 验签 → 写入非活跃槽位 → 切换槽位 → 重启。
//! 非 Linux 平台对 write_to_slot/switch_slot/apply 提供 stub
//!（返回 UnsupportedPlatform），下载/验签/列举/回滚跨平台可用。

use crate::update::ab_partition::{AbPartition, Slot};
use crate::update::error::UpdateError;
use crate::update::manifest::UpdateManifest;
use crate::update::signer;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// OTA 配置
pub struct OtaConfig {
    /// 下载目录，如 /data/updates/
    pub download_dir: PathBuf,
    /// OTA 服务器 URL（可选，本地文件不需要）
    pub update_server_url: Option<String>,
    /// 公钥路径，如 /etc/eneros/keys/signing.pub
    pub verify_keys_path: PathBuf,
    /// 槽位状态文件，如 /etc/eneros/slot-state.json
    pub slot_state_path: PathBuf,
}

/// 已下载的更新包信息
pub struct UpdateBundleInfo {
    /// 文件名
    pub name: String,
    /// 文件大小（字节）
    pub size: u64,
    /// 完整路径
    pub path: PathBuf,
}

/// OTA 管理器
pub struct OtaManager {
    /// OTA 配置
    pub config: OtaConfig,
    /// A/B 分区状态
    pub ab_partition: AbPartition,
}

impl OtaManager {
    /// 创建 OtaManager，从 slot_state_path 加载 AbPartition。
    /// 文件不存在时使用默认状态（Slot A=Active, Slot B=Inactive）。
    pub fn new(config: OtaConfig) -> Result<Self, UpdateError> {
        let ab_partition = AbPartition::load_from_file(&config.slot_state_path)?;
        Ok(Self { config, ab_partition })
    }

    /// 下载更新包。
    /// - HTTP/HTTPS URL：流式下载到 download_dir 临时文件 + rename（原子操作）
    /// - 本地路径：直接返回（验证文件存在）
    ///
    /// 下载失败时清理 .tmp 残留文件；HTTP 请求带 5 分钟超时避免永久阻塞。
    pub fn download_bundle(&self, url_or_path: &str) -> Result<PathBuf, UpdateError> {
        if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
            std::fs::create_dir_all(&self.config.download_dir)?;
            let filename = url_or_path
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("update.eneros-update");
            let final_path = self.config.download_dir.join(filename);
            let tmp_path = self.config.download_dir.join(format!("{filename}.tmp"));

            // 清理可能残留的 .tmp 文件
            let _ = std::fs::remove_file(&tmp_path);

            // 下载（带超时）
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300)) // 5 分钟超时
                .build()
                .map_err(|e| UpdateError::HttpDownload(format!("client build: {e}")))?;
            let mut response = client
                .get(url_or_path)
                .send()
                .map_err(|e| UpdateError::HttpDownload(e.to_string()))?;
            if !response.status().is_success() {
                return Err(UpdateError::HttpDownload(format!(
                    "HTTP {}",
                    response.status()
                )));
            }

            // 下载到临时文件（用闭包捕获错误，避免 ? 提前返回跳过清理）
            let download_result: Result<(), UpdateError> = (|| {
                let mut file = std::fs::File::create(&tmp_path)?;
                std::io::copy(&mut response, &mut file)?;
                file.sync_all()?;
                Ok(())
            })();

            // 下载失败时清理临时文件
            if let Err(e) = download_result {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e);
            }

            std::fs::rename(&tmp_path, &final_path)?;
            tracing::info!("Downloaded bundle to {}", final_path.display());
            Ok(final_path)
        } else {
            let path = PathBuf::from(url_or_path);
            if !path.exists() {
                return Err(UpdateError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("file not found: {url_or_path}"),
                )));
            }
            Ok(path)
        }
    }

    /// 验证更新包：解压 tar.gz，读取 manifest.json，验证 Ed25519 签名，校验每个镜像的 SHA256 和大小。
    /// 返回 (manifest, 解压目录路径)。调用方负责在使用完毕后删除解压目录。
    pub fn verify_bundle(&self, path: &Path) -> Result<(UpdateManifest, PathBuf), UpdateError> {
        let file = std::fs::File::open(path)?;
        let gz = GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);

        let temp_dir = std::env::temp_dir()
            .join(format!("eneros-ota-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)?;
        archive
            .unpack(&temp_dir)
            .map_err(|e| UpdateError::BundleInvalid(e.to_string()))?;

        // 读取 manifest.json
        let manifest_path = temp_dir.join("manifest.json");
        let manifest_content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| UpdateError::BundleInvalid(format!("manifest.json: {e}")))?;
        let manifest = UpdateManifest::from_json(&manifest_content)?;

        // 验证 Ed25519 签名
        let pubkey = signer::load_verifying_key(&self.config.verify_keys_path)?;
        if !signer::verify_manifest(&manifest, &pubkey) {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(UpdateError::SignatureFailed(
                "manifest signature verification failed".into(),
            ));
        }

        // 校验每个镜像的大小和 SHA256
        for entry in &manifest.images {
            let img_path = temp_dir.join(&entry.name);

            // 校验文件大小（manifest 中声明的大小）
            let actual_size = std::fs::metadata(&img_path)
                .map_err(|e| UpdateError::BundleInvalid(format!("image {}: {e}", entry.name)))?
                .len();
            if actual_size != entry.size {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(UpdateError::BundleInvalid(format!(
                    "image {} size mismatch: manifest={}, actual={}",
                    entry.name, entry.size, actual_size
                )));
            }

            // 流式计算 SHA256（避免大文件 OOM，rootfs.img 可能 1.5GB）
            let img_file = std::fs::File::open(&img_path)
                .map_err(|e| UpdateError::BundleInvalid(format!("image {}: {e}", entry.name)))?;
            let mut reader = std::io::BufReader::new(img_file);
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 65536]; // 64KB 缓冲区
            loop {
                let n = std::io::Read::read(&mut reader, &mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            let actual = hex_encode(hasher.finalize().as_slice());
            if actual != entry.sha256 {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(UpdateError::HashMismatch {
                    name: entry.name.clone(),
                    expected: entry.sha256.clone(),
                    actual,
                });
            }
        }

        // 不删除 temp_dir，返回给调用方（write_to_slot）使用，避免双重解压
        Ok((manifest, temp_dir))
    }

    /// 列出 download_dir 中的 .eneros-update 文件。
    pub fn list_updates(&self) -> Vec<UpdateBundleInfo> {
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.config.download_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("eneros-update") {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    result.push(UpdateBundleInfo { name, size, path });
                }
            }
        }
        result
    }

    /// 回滚到最近的 Good 槽位（跨平台可用）。
    /// 若当前已在 Good 槽位则无需操作；否则切换到 last_good_slot。
    /// Linux 下同时更新 GRUB grubenv，确保 GRUB 启动回滚目标槽位。
    pub fn rollback(&mut self) -> Result<(), UpdateError> {
        let good_slot = self
            .ab_partition
            .last_good_slot()
            .ok_or_else(|| UpdateError::SlotError("no good slot available for rollback".into()))?;
        if good_slot != self.ab_partition.active_slot {
            // 更新 GRUB grubenv（仅 Linux）
            #[cfg(target_os = "linux")]
            {
                let slot_str = match good_slot {
                    Slot::A => "A",
                    Slot::B => "B",
                };
                update_grubenv("/boot/efi/EFI/ENEROS/grubenv", slot_str)?;
            }
            self.ab_partition.switch_slot();
            self.ab_partition.save_to_file(&self.config.slot_state_path)?;
        }
        tracing::info!("Rolled back to slot {:?}", good_slot);
        Ok(())
    }

    /// 写入镜像到指定槽位（Linux only）。
    ///
    /// - rootfs.img → 写入目标分区（/dev/sda2 或 /dev/sda3），sync_all 确保落盘
    /// - vmlinuz/initramfs.img → 复制到 EFI 分区对应 slot 目录
    /// - 更新 ab_partition.last_update 时间戳
    ///
    /// 从 verify_bundle 返回的解压目录读取文件，避免双重解压。
    #[cfg(target_os = "linux")]
    pub fn write_to_slot(
        &mut self,
        _manifest: &UpdateManifest,
        slot: Slot,
        extract_dir: &Path,
    ) -> Result<(), UpdateError> {
        let target_partition = match slot {
            Slot::A => "/dev/sda2",
            Slot::B => "/dev/sda3",
        };
        let efi_slot_dir = match slot {
            Slot::A => "/boot/eneros/slot-a",
            Slot::B => "/boot/eneros/slot-b",
        };

        // 写入 rootfs.img 到目标分区（用 std::io::copy + sync_all，避免 std::fs::copy
        // 对块设备设置权限失败，并确保数据落盘防止断电丢失）
        let rootfs_path = extract_dir.join("rootfs.img");
        if rootfs_path.exists() {
            tracing::info!("Writing rootfs.img to {}", target_partition);
            let src = std::fs::File::open(&rootfs_path)?;
            let dst = std::fs::OpenOptions::new()
                .write(true)
                .open(target_partition)?;
            {
                let mut reader = std::io::BufReader::new(src);
                let mut writer = std::io::BufWriter::new(&dst);
                std::io::copy(&mut reader, &mut writer)?;
            } // writer dropped & flushed here
            dst.sync_all()?; // 确保数据落盘
        }

        // 复制 vmlinuz 和 initramfs.img 到 EFI 分区对应 slot 目录
        std::fs::create_dir_all(efi_slot_dir)?;
        for name in &["vmlinuz", "initramfs.img"] {
            let src_path = extract_dir.join(name);
            if src_path.exists() {
                let dst = format!("{efi_slot_dir}/{name}");
                tracing::info!("Copying {} to {}", name, dst);
                let src_file = std::fs::File::open(&src_path)?;
                let mut dst_file = std::fs::File::create(&dst)?;
                std::io::copy(&mut std::io::BufReader::new(src_file), &mut dst_file)?;
                dst_file.sync_all()?;
            }
        }

        // 更新 last_update 时间戳
        self.ab_partition.last_update = Some(chrono::Utc::now());

        Ok(())
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn write_to_slot(
        &mut self,
        _manifest: &UpdateManifest,
        _slot: Slot,
        _extract_dir: &Path,
    ) -> Result<(), UpdateError> {
        Err(UpdateError::UnsupportedPlatform)
    }

    /// 切换槽位：更新 GRUB grubenv + 切换 ab_partition 状态为 Trying（Linux only）。
    /// 旧槽位保留 Good 作为回滚目标，新槽位设为 Trying（尚未确认启动成功）。
    #[cfg(target_os = "linux")]
    pub fn switch_slot(&mut self, slot: Slot) -> Result<(), UpdateError> {
        let slot_str = match slot {
            Slot::A => "A",
            Slot::B => "B",
        };
        update_grubenv("/boot/efi/EFI/ENEROS/grubenv", slot_str)?;
        if slot != self.ab_partition.active_slot {
            self.ab_partition.switch_to_trying();
        }
        self.ab_partition.save_to_file(&self.config.slot_state_path)?;
        tracing::info!("Switched to slot {} (Trying)", slot_str);
        Ok(())
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn switch_slot(&mut self, _slot: Slot) -> Result<(), UpdateError> {
        Err(UpdateError::UnsupportedPlatform)
    }

    /// 完整 OTA 流程：下载 → 验签 → 写入非活跃槽位 → 切换槽位（Linux only）。
    #[cfg(target_os = "linux")]
    pub fn apply(&mut self, url_or_path: &str) -> Result<(), UpdateError> {
        let inactive = self.ab_partition.inactive_slot();

        tracing::info!("OTA apply: downloading bundle from {}", url_or_path);
        let bundle_path = self.download_bundle(url_or_path)?;

        tracing::info!("OTA apply: verifying bundle at {}", bundle_path.display());
        let (manifest, extract_dir) = self.verify_bundle(&bundle_path)?;

        // 校验 manifest.target_slot 与非活跃槽位匹配
        if manifest.target_slot != inactive {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(UpdateError::SlotError(format!(
                "manifest target_slot {:?} does not match inactive slot {:?}",
                manifest.target_slot, inactive
            )));
        }

        tracing::info!("OTA apply: writing to slot {:?}", inactive);
        self.write_to_slot(&manifest, inactive, &extract_dir)?;

        // 清理解压目录
        let _ = std::fs::remove_dir_all(&extract_dir);

        tracing::info!("OTA apply: switching to slot {:?}", inactive);
        self.switch_slot(inactive)?;

        tracing::info!("OTA apply: complete — reboot to activate new slot");
        Ok(())
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn apply(&mut self, _url_or_path: &str) -> Result<(), UpdateError> {
        Err(UpdateError::UnsupportedPlatform)
    }
}

/// 将字节编码为小写十六进制字符串
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 更新 GRUB grubenv 文件中的 next_slot 变量（Linux only）。
///
/// GRUB 环境块必须恰好 1024 字节，用 '#' 填充至 1024 字节。
/// 切换槽位时重置 boot_count=0。
#[cfg(target_os = "linux")]
fn update_grubenv(path: &str, slot: &str) -> Result<(), UpdateError> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut found_next = false;
    let mut found_boot = false;
    let mut lines: Vec<String> = content
        .lines()
        .filter(|l| !l.starts_with('#') || l.is_empty())
        .map(|l| {
            if l.starts_with("next_slot=") {
                found_next = true;
                format!("next_slot={slot}")
            } else if l.starts_with("boot_count=") {
                found_boot = true;
                "boot_count=0".to_string() // 切换槽位时重置 boot_count
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found_next {
        lines.push(format!("next_slot={slot}"));
    }
    if !found_boot {
        lines.push("boot_count=0".to_string());
    }

    let mut new_content = lines.join("\n") + "\n";
    // GRUB 环境块必须恰好 1024 字节，用 '#' 填充
    new_content.push_str(&"#".repeat(1024.saturating_sub(new_content.len())));
    // 确保不超过 1024 字节
    new_content.truncate(1024);
    std::fs::write(path, new_content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成测试用 OtaConfig，每个测试使用独立临时目录避免并行冲突。
    fn test_config(test_name: &str) -> OtaConfig {
        let tmp = std::env::temp_dir().join(format!("eneros-ota-test-{test_name}"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("downloads")).unwrap();
        OtaConfig {
            download_dir: tmp.join("downloads"),
            verify_keys_path: tmp.join("signing.pub"),
            slot_state_path: tmp.join("slot-state.json"),
            update_server_url: None,
        }
    }

    #[test]
    fn test_list_updates_empty() {
        let config = test_config("list_empty");
        let manager = OtaManager::new(config).unwrap();
        assert!(manager.list_updates().is_empty());
    }

    #[test]
    fn test_list_updates_with_files() {
        let config = test_config("list_with_files");
        std::fs::create_dir_all(&config.download_dir).unwrap();
        std::fs::write(config.download_dir.join("v1.0.eneros-update"), b"bundle").unwrap();
        std::fs::write(config.download_dir.join("v2.0.eneros-update"), b"more").unwrap();
        std::fs::write(config.download_dir.join("not-update.txt"), b"text").unwrap();

        let manager = OtaManager::new(config).unwrap();
        let updates = manager.list_updates();
        assert_eq!(updates.len(), 2);
        for u in &updates {
            assert!(u.name.ends_with(".eneros-update"));
            assert!(u.path.exists());
        }
    }

    #[test]
    fn test_verify_bundle_nonexistent() {
        let config = test_config("verify_nonexistent");
        let manager = OtaManager::new(config).unwrap();
        let result = manager.verify_bundle(Path::new("/nonexistent/bundle.eneros-update"));
        assert!(result.is_err());
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_apply_unsupported_platform() {
        let config = test_config("apply_unsupported");
        let mut manager = OtaManager::new(config).unwrap();
        let result = manager.apply("/some/path.eneros-update");
        assert!(matches!(result, Err(UpdateError::UnsupportedPlatform)));
    }

    #[test]
    fn test_rollback_no_good_slot() {
        let config = test_config("rollback_no_good");
        let mut manager = OtaManager::new(config).unwrap();
        // 默认：A=Active, B=Inactive，无 Good 槽位
        let result = manager.rollback();
        assert!(matches!(result, Err(UpdateError::SlotError(_))));
    }

    #[test]
    fn test_download_local_file() {
        let config = test_config("download_local");
        std::fs::create_dir_all(&config.download_dir).unwrap();
        let test_file = config.download_dir.join("local-bundle.eneros-update");
        std::fs::write(&test_file, b"test content").unwrap();

        let manager = OtaManager::new(config).unwrap();
        let result = manager.download_bundle(test_file.to_str().unwrap());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test_file);
    }

    #[test]
    fn test_download_nonexistent_local() {
        let config = test_config("download_nonexistent");
        let manager = OtaManager::new(config).unwrap();
        let result = manager.download_bundle("/nonexistent/path.eneros-update");
        assert!(result.is_err());
    }
}
