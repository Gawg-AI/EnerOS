//! OTA 更新清单（v0.22.0）
//!
//! Ed25519 签名的更新清单，描述目标槽位、镜像列表和签名。
//! 签名负载使用 \x1f (Unit Separator) 分隔字段，防止字段内注入。

use crate::update::ab_partition::Slot;
use crate::update::error::UpdateError;
use serde::{Deserialize, Serialize};

/// 镜像文件条目（名称 + SHA256 + 大小）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEntry {
    pub name: String,   // 如 "rootfs.img"、"vmlinuz"、"initramfs.img"
    pub sha256: String, // hex 编码的 SHA256
    pub size: u64,      // 文件大小（字节）
}

/// 更新清单（Ed25519 签名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,                           // manifest 格式版本，如 "1.0"
    pub target_slot: Slot,                         // 目标写入槽位
    pub image_version: String,                     // 镜像版本号，如 "0.22.0"
    pub images: Vec<ImageEntry>,                   // 镜像文件列表
    pub created_at: chrono::DateTime<chrono::Utc>, // 创建时间
    pub signature: String,                         // Ed25519 签名（base64 编码）
}

impl UpdateManifest {
    /// 构造签名负载（不含 signature 字段本身）。
    ///
    /// 字段以 `\x1f` (Unit Separator) 分隔，防止字段内注入。
    /// 格式：`version \x1f target_slot \x1f image_version \x1f created_at \x1f images...`
    /// 其中 images 每个条目的 name/sha256/size 也以 \x1f 拼接。
    pub fn signing_payload(&self) -> Vec<u8> {
        let slot_str = match self.target_slot {
            Slot::A => "A",
            Slot::B => "B",
        };
        let mut parts: Vec<String> = vec![
            self.version.clone(),
            slot_str.to_string(),
            self.image_version.clone(),
            self.created_at.to_rfc3339(),
        ];
        for img in &self.images {
            parts.push(img.name.clone());
            parts.push(img.sha256.clone());
            parts.push(img.size.to_string());
        }
        parts.join("\x1f").into_bytes()
    }

    /// 序列化为 JSON 字符串。
    pub fn to_json(&self) -> Result<String, UpdateError> {
        serde_json::to_string(self).map_err(|e| UpdateError::Serialize(e.to_string()))
    }

    /// 从 JSON 字符串反序列化。
    pub fn from_json(json: &str) -> Result<UpdateManifest, UpdateError> {
        serde_json::from_str(json).map_err(|e| UpdateError::Serialize(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_manifest() -> UpdateManifest {
        UpdateManifest {
            version: "1.0".to_string(),
            target_slot: Slot::B,
            image_version: "0.22.0".to_string(),
            images: vec![
                ImageEntry {
                    name: "rootfs.img".to_string(),
                    sha256: "abc123".to_string(),
                    size: 1024,
                },
                ImageEntry {
                    name: "vmlinuz".to_string(),
                    sha256: "def456".to_string(),
                    size: 8192,
                },
            ],
            created_at: Utc::now(),
            signature: String::new(),
        }
    }

    #[test]
    fn test_manifest_signing_payload() {
        let m = sample_manifest();
        let payload = m.signing_payload();
        let s = String::from_utf8(payload).unwrap();
        let parts: Vec<&str> = s.split('\x1f').collect();
        assert_eq!(parts[0], "1.0");
        assert_eq!(parts[1], "B");
        assert_eq!(parts[2], "0.22.0");
        // parts[3] = created_at (RFC3339)
        assert_eq!(parts[4], "rootfs.img");
        assert_eq!(parts[5], "abc123");
        assert_eq!(parts[6], "1024");
        assert_eq!(parts[7], "vmlinuz");
        assert_eq!(parts[8], "def456");
        assert_eq!(parts[9], "8192");
    }

    #[test]
    fn test_manifest_json_roundtrip() {
        let m = sample_manifest();
        let json = m.to_json().unwrap();
        let m2 = UpdateManifest::from_json(&json).unwrap();
        assert_eq!(m2.version, m.version);
        assert_eq!(m2.target_slot, m.target_slot);
        assert_eq!(m2.image_version, m.image_version);
        assert_eq!(m2.images.len(), m.images.len());
        assert_eq!(m2.images[0].name, m.images[0].name);
        assert_eq!(m2.images[0].sha256, m.images[0].sha256);
        assert_eq!(m2.images[0].size, m.images[0].size);
        assert_eq!(m2.created_at, m.created_at);
    }

    #[test]
    fn test_manifest_json_corrupt() {
        let result = UpdateManifest::from_json("{ this is not valid json");
        assert!(result.is_err());
    }
}
