//! 47（WP-47.3）密钥出仓与显式注入：签名/验签键的唯一解析入口。
//!
//! # 出仓事实（A6 锚点修订登记）
//!
//! - 私钥**不在 git**：`.gitignore` `*.key` 规则 + `check-ignore` 证实；
//!   `git log` 无历史；`key-policy.json` 早已声明
//!   `private_keys_in_repository:false`。
//! - 47.3 处置的是「默认引用」：xtask 各命令此前硬编码旧路径——现统一移入
//!   `product/security/keys-local/`（gitignore 白名单目录）并全部改为
//!   **显式传入**（`--sign-key` / 环境变量），无键时非测试装配 fail-closed。
//!
//! # 解析优先级（单一事实源）
//!
//! 1. 显式 `--sign-key <path>`；
//! 2. 环境变量（`ENEROS_SIGN_KEY` 签章 / `ENEROS_VERIFY_PUBLIC_KEY` 验签
//!    ——独立注入，验签钥与签章钥可分离）；
//! 3. 测试构建（`ProfileKind::Test`）→ `keys-local/` 注册测试键；
//! 4. 其余（release/生产装配）→ Err（fail-closed：无键不构建）。

use std::fs;
use std::path::{Path, PathBuf};

use super::{invalid, DynResult};

/// 签名/装配 profile 分类（test = qemu-test 等允许注册测试键）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// qemu-test / host 测试等允许 `keys-local/` 注册测试键。
    Test,
    /// qemu-prod / board-prod / release 等——必须显式键（fail-closed）。
    Production,
}

/// 键文件相对路径（全部在 gitignore 的 `keys-local/` 目录；不再有任何
/// 源码默认引用指向可被误提交的位置）。
pub const DEV_SIGNING_KEY_REL: &str = "product/security/keys-local/dev-signing.key";
/// UEFI 测试键（RSA-3072；仅测试签章）。
pub const UEFI_TEST_KEY_REL: &str = "product/security/keys-local/uefi-test.key";
/// UEFI 测试证书（Authenticode；仅测试签章）。
pub const UEFI_TEST_PEM_REL: &str = "product/security/keys-local/uefi-test.pem";

/// 环境变量：显式签章键路径（绝对或相对仓库根）。
pub const ENV_SIGN_KEY: &str = "ENEROS_SIGN_KEY";
/// 环境变量：独立验签公钥（32B hex；验签与签章可分离配置）。
pub const ENV_VERIFY_PUBLIC_KEY: &str = "ENEROS_VERIFY_PUBLIC_KEY";

/// 解析签章键路径（优先级：显式 > 环境变量 > 测试键；生产无键 → Err）。
pub fn resolve_signing_key(
    root: &Path,
    explicit: Option<&Path>,
    profile: ProfileKind,
) -> DynResult<PathBuf> {
    if let Some(p) = explicit {
        if !p.is_file() {
            return Err(invalid(format!("--sign-key 路径不存在: {}", p.display())));
        }
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var(ENV_SIGN_KEY) {
        let pb = if p.starts_with('/') {
            PathBuf::from(&p)
        } else {
            root.join(&p)
        };
        if !pb.is_file() {
            return Err(invalid(format!(
                "{ENV_SIGN_KEY} 路径不存在: {}",
                pb.display()
            )));
        }
        return Ok(pb);
    }
    if profile == ProfileKind::Test {
        let pb = root.join(DEV_SIGNING_KEY_REL);
        if pb.is_file() {
            return Ok(pb);
        }
        return Err(invalid(format!(
            "测试签名键缺失: {}（先运行 tools/gen-local-keys.sh）",
            pb.display()
        )));
    }
    Err(invalid(format!(
        "生产装配签名键必须显式传入：--sign-key <path> 或 {ENV_SIGN_KEY}（或 KMS_DIR 提供）——fail-closed"
    )))
}

/// 读取 32B Ed25519 签章键（长度校验）。
pub fn load_signing_key_bytes(
    root: &Path,
    explicit: Option<&Path>,
    profile: ProfileKind,
) -> DynResult<[u8; 32]> {
    let path = resolve_signing_key(root, explicit, profile)?;
    let bytes = fs::read(&path)?;
    let key: [u8; 32] = bytes
        .get(..32)
        .ok_or_else(|| invalid("signing key too short (need 32B)"))?
        .try_into()
        .map_err(|_| invalid("signing key invalid length"))?;
    Ok(key)
}

/// 验签公钥（独立注入）：`ENEROS_VERIFY_PUBLIC_KEY`（32B hex）优先；
/// 未注入而存在 keys-local 测试键 → 由测试键派生（host 开发验签）；
/// 两者皆无 → None（调用方登记「无法验签」，不武断 PASS/FAIL）。
pub fn verify_public_key(root: &Path) -> Option<[u8; 32]> {
    if let Ok(hex) = std::env::var(ENV_VERIFY_PUBLIC_KEY) {
        let b = super::evidence_chain::hex_decode(hex.trim());
        if b.len() != 32 {
            return None; // 长度不符 → 不注入（fail-closed，不猜）
        }
        return <[u8; 32]>::try_from(b).ok();
    }
    let key_path = root.join(DEV_SIGNING_KEY_REL);
    let bytes = fs::read(&key_path).ok()?;
    let key_bytes: [u8; 32] = bytes.get(..32)?.try_into().ok()?;
    Some(
        ed25519_dalek::SigningKey::from_bytes(&key_bytes)
            .verifying_key()
            .to_bytes(),
    )
}

/// 测试键是否就位（gate keys 检查项之一）。
pub fn test_keys_present(root: &Path) -> bool {
    root.join(DEV_SIGNING_KEY_REL).is_file()
}

fn getrandom_fill(buf: &mut [u8]) -> crate::DynResult<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(buf)?;
    Ok(())
}

/// 70.5（WP-70.5.2）：生成 OTA 信任锚（随机 SM2 标量 → 未压缩公钥的
/// 64B 坐标 hex——`--root-key-hex` 注入形态）。私钥**不落盘不打印**
/// （锚分发生成器：本命令只产出验签公钥；签名方（签发 station/发布
/// 系统）另行持有对应私钥——产线注入面 71.2 承接）。stdout = 128 hex。
pub fn gen_ota_anchor(root: &Path) -> crate::DynResult<()> {
    let _ = root;
    // 随机标量（平台熵；host 测试面失败即 Err——不回退固定值）。
    let mut seed = [0u8; 32];
    getrandom_fill(&mut seed)?;
    if seed.iter().all(|b| *b == 0) {
        return Err(crate::invalid("OTA 锚标量全零（熵不可用——fail-closed）"));
    }
    let sk = eneros_crypto::Sm2PrivateKey::from_bytes(&seed)
        .map_err(|e| crate::invalid(format!("标量无效: {e:?}")))?;
    let kp = eneros_crypto::Sm2KeyPair::from_private_key(&sk)
        .map_err(|e| crate::invalid(format!("公钥派生失败: {e:?}")))?;
    let bytes = kp.public_key.to_bytes_uncompressed();
    let hex: String = bytes[1..65].iter().map(|b| format!("{b:02x}")).collect();
    println!("{hex}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_requires_explicit_or_env_for_production() {
        let root = Path::new("/nonexistent");
        let err = resolve_signing_key(root, None, ProfileKind::Production);
        assert!(err.is_err(), "生产无键必须 fail-closed");
        let err = resolve_signing_key(
            root,
            Some(Path::new("/no/such/key")),
            ProfileKind::Production,
        );
        assert!(err.is_err(), "显式路径不存在必须失败");
    }

    #[test]
    fn resolve_test_profile_requires_registered_key() {
        let root = Path::new("/nonexistent");
        let err = resolve_signing_key(root, None, ProfileKind::Test);
        assert!(err.is_err(), "测试键缺失必须显式失败（不静默跳过）");
    }
}
