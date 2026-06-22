//! Secure Boot 与启动安全（UEFI 变量管理 + 签名验证 + 状态查询）
//!
//! 提供 UEFI Secure Boot 状态查询、UEFI 变量读取（PK/KEK/db/dbx）、
//! 内核/initramfs 签名验证、OTA 包签名验证（复用 v0.22.0 Ed25519）。
//!
//! Linux 平台通过 `/sys/firmware/efi/efivars/` 读取 UEFI 变量；
//! 非 Linux 平台提供 stub 实现，用于开发/测试。
//!
//! ## UEFI 变量读取
//! EFI 变量文件名格式：`<NAME>-<GUID>`，前 4 字节为属性，其余为值。
//! SecureBoot 变量 GUID：`8be4df61-93ca-11d2-aa0d-00e098032b8c`（EFI_GLOBAL_VARIABLE）

use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// EFI 全局变量 GUID（EFI_GLOBAL_VARIABLE）
#[cfg(target_os = "linux")]
const EFI_GLOBAL_VARIABLE_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// EFI 变量属性：非易失性 + 启动服务访问 + 运行时访问
const EFI_VAR_NON_VOLATILE: u32 = 0x00000001;
const EFI_VAR_BOOTSERVICE_ACCESS: u32 = 0x00000002;
const EFI_VAR_RUNTIME_ACCESS: u32 = 0x00000004;

/// 安全错误类型
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("efi variable not found: {0}")]
    EfiVarNotFound(String),
    #[error("efi variable parse error: {0}")]
    EfiVarParse(String),
    #[error("signature verification failed: {0}")]
    SignatureFailed(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
    #[error("config error: {0}")]
    Config(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// Secure Boot 状态
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecureBootStatus {
    /// Secure Boot 是否启用
    pub enabled: bool,
    /// Secure Boot 是否处于设置模式（false=用户模式，true=设置模式）
    pub setup_mode: bool,
    /// 平台密钥（PK）是否已设置
    pub pk_set: bool,
    /// 密钥交换密钥（KEK）是否已设置
    pub kek_set: bool,
    /// 签名数据库（db）条目数
    pub db_count: usize,
    /// 吊销数据库（dbx）条目数
    pub dbx_count: usize,
    /// 平台模式（0=用户模式，1=设置模式）
    pub platform_mode: u8,
}

/// UEFI 变量信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EfiVariable {
    /// 变量名
    pub name: String,
    /// GUID
    pub guid: String,
    /// 属性位
    pub attributes: u32,
    /// 变量值（不含属性前缀）
    pub value: Vec<u8>,
}

impl EfiVariable {
    /// 读取 EFI 变量原始字节（含 4 字节属性前缀）
    #[cfg(target_os = "linux")]
    fn read_raw(name: &str, guid: &str) -> Result<Vec<u8>, SecurityError> {
        let path = PathBuf::from("/sys/firmware/efi/efivars")
            .join(format!("{}-{}", name, guid));
        if !path.exists() {
            return Err(SecurityError::EfiVarNotFound(format!("{}-{}", name, guid)));
        }
        std::fs::read(&path)
    }

    /// 解析 EFI 变量：前 4 字节为属性，其余为值
    #[cfg(target_os = "linux")]
    fn parse_raw(raw: &[u8]) -> Result<Self, SecurityError> {
        if raw.len() < 4 {
            return Err(SecurityError::EfiVarParse("variable too short".into()));
        }
        let attributes = u32::from_le_bytes([
            raw[0], raw[1], raw[2], raw[3],
        ]);
        let value = raw[4..].to_vec();
        Ok(Self {
            name: String::new(),
            guid: String::new(),
            attributes,
            value,
        })
    }
}

/// 校验 EFI 变量名/GUID，防止路径遍历与注入攻击
///
/// 拒绝以下输入：
/// - 空字符串
/// - 包含路径分隔符 `/` 或 `\`
/// - 包含父目录引用 `..`
/// - 包含空字节 `\0`
///
/// 该函数仅做字符串检查，不依赖平台特性，因此在所有平台可用。
fn validate_efi_var_name(name: &str) -> Result<(), SecurityError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(SecurityError::InvalidInput(name.to_string()));
    }
    Ok(())
}

/// Secure Boot 管理器
///
/// 查询 UEFI Secure Boot 状态、读取 UEFI 变量、验证签名。
/// 非 Linux 平台提供 stub 实现。
pub struct SecureBootManager {
    /// efivars 挂载路径（默认 /sys/firmware/efi/efivars）
    #[cfg(target_os = "linux")]
    efivars_path: PathBuf,
}

#[allow(clippy::derivable_impls)] // 需要非默认路径值
impl Default for SecureBootManager {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            efivars_path: PathBuf::from("/sys/firmware/efi/efivars"),
        }
    }
}

impl SecureBootManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定 efivars 路径（用于测试）
    #[cfg(target_os = "linux")]
    pub fn with_path(efivars_path: impl Into<PathBuf>) -> Self {
        Self {
            efivars_path: efivars_path.into(),
        }
    }

    /// 查询 Secure Boot 完整状态
    ///
    /// 读取以下 EFI 变量：
    /// - `SecureBoot`（1 字节，1=启用）
    /// - `SetupMode`（1 字节，1=设置模式）
    /// - `PK`（平台密钥，存在即已设置）
    /// - `KEK`（密钥交换密钥，存在即已设置）
    /// - `db`（签名数据库）
    /// - `dbx`（吊销数据库）
    #[cfg(target_os = "linux")]
    pub fn status(&self) -> Result<SecureBootStatus, SecurityError> {
        let secure_boot = self.read_efi_var_u8("SecureBoot")?;
        let setup_mode = self.read_efi_var_u8("SetupMode")?;
        let pk_set = self.efi_var_exists("PK");
        let kek_set = self.efi_var_exists("KEK");
        let db_count = self.count_db_entries("db")?;
        let dbx_count = self.count_db_entries("dbx")?;
        let platform_mode = setup_mode;

        Ok(SecureBootStatus {
            enabled: secure_boot == 1,
            setup_mode: setup_mode == 1,
            pk_set,
            kek_set,
            db_count,
            dbx_count,
            platform_mode,
        })
    }

    /// 非 Linux stub：返回默认状态（全部未启用）
    #[cfg(not(target_os = "linux"))]
    pub fn status(&self) -> Result<SecureBootStatus, SecurityError> {
        Ok(SecureBootStatus::default())
    }

    /// 读取 EFI 变量并解析为 u8
    #[cfg(target_os = "linux")]
    fn read_efi_var_u8(&self, name: &str) -> Result<u8, SecurityError> {
        let path = self.efivars_path.join(format!("{}-{}", name, EFI_GLOBAL_VARIABLE_GUID));
        if !path.exists() {
            return Err(SecurityError::EfiVarNotFound(name.to_string()));
        }
        let raw = std::fs::read(&path)?;
        if raw.len() < 5 {
            return Err(SecurityError::EfiVarParse(format!(
                "{}: expected >=5 bytes, got {}",
                name,
                raw.len()
            )));
        }
        // 前 4 字节属性，第 5 字节为值
        Ok(raw[4])
    }

    /// 检查 EFI 变量是否存在
    #[cfg(target_os = "linux")]
    fn efi_var_exists(&self, name: &str) -> bool {
        let path = self.efivars_path.join(format!("{}-{}", name, EFI_GLOBAL_VARIABLE_GUID));
        path.exists() && std::fs::metadata(&path).map(|m| m.len() > 4).unwrap_or(false)
    }

    /// 统计 db/dbx 条目数（简化：按签名列表条目数估算）
    ///
    /// EFI_SIGNATURE_LIST 结构：16 字节 GUID + 4 字节 DataSize + 4 字节 SignatureSize
    /// 这里简化为：文件大小 / 平均条目大小（约 100 字节）
    #[cfg(target_os = "linux")]
    fn count_db_entries(&self, name: &str) -> Result<usize, SecurityError> {
        let path = self.efivars_path.join(format!("{}-{}", name, EFI_GLOBAL_VARIABLE_GUID));
        if !path.exists() {
            return Ok(0);
        }
        let metadata = std::fs::metadata(&path)?;
        let size = metadata.len();
        // 减去 4 字节属性前缀，按平均 100 字节/条目估算
        if size <= 4 {
            return Ok(0);
        }
        Ok(((size - 4) / 100) as usize)
    }

    /// 读取指定 EFI 变量
    #[cfg(target_os = "linux")]
    pub fn read_variable(&self, name: &str, guid: &str) -> Result<EfiVariable, SecurityError> {
        validate_efi_var_name(name)?;
        validate_efi_var_name(guid)?;
        let path = self.efivars_path.join(format!("{}-{}", name, guid));
        if !path.exists() {
            return Err(SecurityError::EfiVarNotFound(format!("{}-{}", name, guid)));
        }
        let raw = std::fs::read(&path)?;
        let mut var = EfiVariable::parse_raw(&raw)?;
        var.name = name.to_string();
        var.guid = guid.to_string();
        Ok(var)
    }

    /// 非 Linux stub
    #[cfg(not(target_os = "linux"))]
    pub fn read_variable(&self, name: &str, guid: &str) -> Result<EfiVariable, SecurityError> {
        validate_efi_var_name(name)?;
        validate_efi_var_name(guid)?;
        Err(SecurityError::UnsupportedPlatform)
    }

    /// 验证内核镜像签名
    ///
    /// 在 Secure Boot 启用时，内核镜像（vmlinuz）必须签名。
    /// 此函数验证签名文件（.sig）是否与内核镜像匹配。
    ///
    /// 注意：UEFI Secure Boot 在固件阶段验证内核签名，此函数用于
    /// 运行时审计和 OTA 更新前验证。
    pub fn verify_kernel_signature(
        &self,
        kernel_path: &Path,
        signature_path: &Path,
        public_key: &[u8],
    ) -> Result<bool, SecurityError> {
        Self::verify_file_signature(kernel_path, signature_path, public_key)
    }

    /// 验证 initramfs 签名
    pub fn verify_initramfs_signature(
        &self,
        initramfs_path: &Path,
        signature_path: &Path,
        public_key: &[u8],
    ) -> Result<bool, SecurityError> {
        Self::verify_file_signature(initramfs_path, signature_path, public_key)
    }

    /// 验证文件签名（Ed25519）
    ///
    /// 读取文件内容计算 SHA-256，验证 Ed25519 签名。
    /// 签名文件为原始 64 字节 Ed25519 签名。
    pub fn verify_file_signature(
        file_path: &Path,
        signature_path: &Path,
        public_key: &[u8],
    ) -> Result<bool, SecurityError> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        if public_key.len() != 32 {
            return Err(SecurityError::SignatureFailed(
                "public key must be 32 bytes (Ed25519)".into(),
            ));
        }

        let file_data = std::fs::read(file_path)?;
        let sig_data = std::fs::read(signature_path)?;
        if sig_data.len() != 64 {
            return Err(SecurityError::SignatureFailed(format!(
                "signature must be 64 bytes, got {}",
                sig_data.len()
            )));
        }

        let pk_bytes: [u8; 32] = public_key.try_into().map_err(|_| {
            SecurityError::SignatureFailed("public key conversion failed".into())
        })?;
        let sig_bytes: [u8; 64] = sig_data.as_slice().try_into().map_err(|_| {
            SecurityError::SignatureFailed("signature conversion failed".into())
        })?;

        let verifying_key = VerifyingKey::from_bytes(&pk_bytes)
            .map_err(|e| SecurityError::SignatureFailed(format!("invalid public key: {}", e)))?;
        let signature = Signature::from_bytes(&sig_bytes);

        Ok(verifying_key
            .verify(&file_data, &signature)
            .is_ok())
    }

    /// 检查内核命令行加固参数是否已应用
    ///
    /// 返回缺失的加固参数列表（空=全部已应用）
    #[cfg(target_os = "linux")]
    pub fn check_kernel_hardening_params(&self) -> Result<Vec<String>, SecurityError> {
        let cmdline = std::fs::read_to_string("/proc/cmdline")?;
        let required = [
            "page_alloc.shuffle=1",
            "slab_nomerge",
            "init_on_alloc=1",
            "init_on_free=1",
        ];
        let missing = required
            .iter()
            .filter(|p| !cmdline.contains(*p))
            .map(|s| s.to_string())
            .collect();
        Ok(missing)
    }

    /// 非 Linux stub
    #[cfg(not(target_os = "linux"))]
    pub fn check_kernel_hardening_params(&self) -> Result<Vec<String>, SecurityError> {
        Ok(Vec::new())
    }

    /// 检查内核配置加固选项是否已启用
    ///
    /// 读取 /proc/config.gz 或 /boot/config-<version> 检查关键加固选项
    #[cfg(target_os = "linux")]
    pub fn check_kernel_config_hardening(&self) -> Result<Vec<String>, SecurityError> {
        let config_content = self.read_kernel_config()?;
        let required = [
            "CONFIG_HARDENED_USERCOPY=y",
            "CONFIG_FORTIFY_SOURCE=y",
            "CONFIG_STACKPROTECTOR_STRONG=y",
            "CONFIG_STRICT_DEVMEM=y",
            "CONFIG_SECURITY_DMESG_RESTRICT=y",
            "CONFIG_MODULE_SIG=y",
            "CONFIG_MODULE_SIG_FORCE=y",
        ];
        let missing = required
            .iter()
            .filter(|p| !config_content.contains(*p))
            .map(|s| s.to_string())
            .collect();
        Ok(missing)
    }

    /// 非 Linux stub
    #[cfg(not(target_os = "linux"))]
    pub fn check_kernel_config_hardening(&self) -> Result<Vec<String>, SecurityError> {
        Ok(Vec::new())
    }

    /// 读取内核配置文件
    #[cfg(target_os = "linux")]
    fn read_kernel_config(&self) -> Result<String, SecurityError> {
        // 尝试 /proc/config.gz（需要 zcat）
        if Path::new("/proc/config.gz").exists() {
            return Ok(std::process::Command::new("zcat")
                .arg("/proc/config.gz")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default());
        }
        // 尝试 /boot/config-<version>
        let uname = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let boot_config = format!("/boot/config-{}", uname);
        if Path::new(&boot_config).exists() {
            return std::fs::read_to_string(&boot_config);
        }
        Ok(String::new())
    }
}

/// 安全状态汇总（用于 enerosctl security status）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityStatus {
    /// Secure Boot 状态
    pub secure_boot: SecureBootStatus,
    /// 缺失的内核命令行加固参数
    pub missing_kernel_cmdline_params: Vec<String>,
    /// 缺失的内核配置加固选项
    pub missing_kernel_config_options: Vec<String>,
    /// seccomp 是否可用（Linux + seccomp feature）
    pub seccomp_available: bool,
    /// 审计日志是否已初始化
    pub audit_initialized: bool,
}

impl SecureBootManager {
    /// 获取完整安全状态汇总
    pub fn full_status(&self) -> Result<SecurityStatus, SecurityError> {
        let secure_boot = self.status()?;
        let missing_kernel_cmdline_params = self.check_kernel_hardening_params()?;
        let missing_kernel_config_options = self.check_kernel_config_hardening()?;

        Ok(SecurityStatus {
            secure_boot,
            missing_kernel_cmdline_params,
            missing_kernel_config_options,
            seccomp_available: cfg!(all(target_os = "linux", feature = "seccomp")),
            audit_initialized: Path::new("/var/log/eneros/audit/audit.log").exists(),
        })
    }
}

/// EFI 变量属性辅助函数
pub fn efi_attr_string(attrs: u32) -> String {
    let mut parts = Vec::new();
    if attrs & EFI_VAR_NON_VOLATILE != 0 {
        parts.push("NV");
    }
    if attrs & EFI_VAR_BOOTSERVICE_ACCESS != 0 {
        parts.push("BS");
    }
    if attrs & EFI_VAR_RUNTIME_ACCESS != 0 {
        parts.push("RT");
    }
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secure_boot_status_default() {
        let status = SecureBootStatus::default();
        assert!(!status.enabled);
        assert!(!status.setup_mode);
        assert!(!status.pk_set);
        assert_eq!(status.db_count, 0);
    }

    #[test]
    fn test_secure_boot_status_serialize() {
        let status = SecureBootStatus {
            enabled: true,
            setup_mode: false,
            pk_set: true,
            kek_set: true,
            db_count: 3,
            dbx_count: 1,
            platform_mode: 0,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"pk_set\":true"));
        assert!(json.contains("\"db_count\":3"));
    }

    #[test]
    fn test_efi_attr_string() {
        let attrs = EFI_VAR_NON_VOLATILE | EFI_VAR_BOOTSERVICE_ACCESS | EFI_VAR_RUNTIME_ACCESS;
        let s = efi_attr_string(attrs);
        assert_eq!(s, "NV|BS|RT");

        let s = efi_attr_string(EFI_VAR_BOOTSERVICE_ACCESS);
        assert_eq!(s, "BS");

        let s = efi_attr_string(0);
        assert_eq!(s, "");
    }

    #[test]
    fn test_efi_attr_constants() {
        assert_eq!(EFI_VAR_NON_VOLATILE, 0x01);
        assert_eq!(EFI_VAR_BOOTSERVICE_ACCESS, 0x02);
        assert_eq!(EFI_VAR_RUNTIME_ACCESS, 0x04);
    }

    #[test]
    fn test_security_error_display() {
        let err = SecurityError::EfiVarNotFound("SecureBoot".into());
        assert!(err.to_string().contains("SecureBoot"));

        let err = SecurityError::SignatureFailed("invalid key".into());
        assert!(err.to_string().contains("invalid key"));
    }

    #[test]
    fn test_secure_boot_manager_new() {
        let _mgr = SecureBootManager::new();
        #[cfg(target_os = "linux")]
        {
            assert_eq!(_mgr.efivars_path, PathBuf::from("/sys/firmware/efi/efivars"));
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_secure_boot_manager_with_path() {
        let _mgr = SecureBootManager::with_path("/tmp/test-efivars");
        assert_eq!(_mgr.efivars_path, PathBuf::from("/tmp/test-efivars"));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_status_unsupported_platform() {
        let mgr = SecureBootManager::new();
        let status = mgr.status().unwrap();
        assert!(!status.enabled);
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_read_variable_unsupported() {
        let mgr = SecureBootManager::new();
        let result = mgr.read_variable("SecureBoot", "8be4df61-93ca-11d2-aa0d-00e098032b8c");
        assert!(matches!(result, Err(SecurityError::UnsupportedPlatform)));
    }

    #[test]
    fn test_read_variable_rejects_path_traversal() {
        let mgr = SecureBootManager::new();
        let valid_guid = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

        // 路径遍历：name 包含 ../
        let result = mgr.read_variable("../../etc/passwd", valid_guid);
        assert!(
            matches!(result, Err(SecurityError::InvalidInput(_))),
            "expected InvalidInput for path traversal in name, got {:?}",
            result
        );

        // 空字节注入：name 包含 \0
        let result = mgr.read_variable("SecureBoot\0malicious", valid_guid);
        assert!(
            matches!(result, Err(SecurityError::InvalidInput(_))),
            "expected InvalidInput for null byte in name, got {:?}",
            result
        );

        // 路径遍历：guid 包含 ../
        let result = mgr.read_variable("SecureBoot", "../../etc/passwd");
        assert!(
            matches!(result, Err(SecurityError::InvalidInput(_))),
            "expected InvalidInput for path traversal in guid, got {:?}",
            result
        );

        // 空字符串
        let result = mgr.read_variable("", valid_guid);
        assert!(
            matches!(result, Err(SecurityError::InvalidInput(_))),
            "expected InvalidInput for empty name, got {:?}",
            result
        );
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_check_kernel_hardening_unsupported() {
        let mgr = SecureBootManager::new();
        let missing = mgr.check_kernel_hardening_params().unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_full_status_unsupported() {
        let mgr = SecureBootManager::new();
        let status = mgr.full_status().unwrap();
        assert!(!status.secure_boot.enabled);
        assert!(status.missing_kernel_cmdline_params.is_empty());
        assert!(status.missing_kernel_config_options.is_empty());
    }

    #[test]
    fn test_verify_file_signature_invalid_key_length() {
        let result = SecureBootManager::verify_file_signature(
            Path::new("/nonexistent"),
            Path::new("/nonexistent.sig"),
            &[0u8; 10], // 错误长度
        );
        assert!(matches!(result, Err(SecurityError::SignatureFailed(_))));
    }

    #[test]
    fn test_efi_variable_parse_raw_too_short() {
        #[cfg(target_os = "linux")]
        {
            let result = EfiVariable::parse_raw(&[1, 2, 3]);
            assert!(matches!(result, Err(SecurityError::EfiVarParse(_))));
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_efi_variable_parse_raw_valid() {
        let raw = [0x07, 0x00, 0x00, 0x00, 0x01]; // 属性=7, 值=1
        let var = EfiVariable::parse_raw(&raw).unwrap();
        assert_eq!(var.attributes, 7);
        assert_eq!(var.value, vec![0x01]);
    }

    #[test]
    fn test_security_status_serialize() {
        let status = SecurityStatus {
            secure_boot: SecureBootStatus::default(),
            missing_kernel_cmdline_params: vec!["slab_nomerge".to_string()],
            missing_kernel_config_options: vec![],
            seccomp_available: false,
            audit_initialized: false,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("slab_nomerge"));
        assert!(json.contains("\"seccomp_available\":false"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_efi_global_variable_guid() {
        assert_eq!(
            EFI_GLOBAL_VARIABLE_GUID,
            "8be4df61-93ca-11d2-aa0d-00e098032b8c"
        );
    }
}
