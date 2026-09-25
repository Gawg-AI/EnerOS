//! installer-uefi host 可测核心层（蓝图 74 卷三 R15-C；manifest 验签面
//! = WP-74.3.1，ISO 解析面下沉 = WP-74.3.2）。
//!
//! UEFI 无关面：安装 manifest 验签（唯一入口 =
//! [`eneros_boot_protocol::signature::BootSignatureProvider`]——installer
//! 不再自带任何密钥/算法路径）与失败码词表。bin（`main.rs`）持 UEFI IO
//! 管道（`ScopedProtocol<BlockIO>` adapter、目标选择、写盘与回读），
//! 本 crate 纯逻辑供 host 测试（CI `-p eneros-installer-uefi`）。
//!
//! 词形冻结：[`InstallFailure`] 的 `code()` 集即
//! `eneros.install-rejection.v1` JSON 的 code 集，逐字冻结，不新增拒绝码。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use eneros_boot_protocol::signature::{
    check_signature_domain, BootSignatureProvider, Ed25519SignatureProvider, KeyDomain,
    SignatureAlgorithm, SignatureDomain,
};
use eneros_boot_protocol::{InstallManifest, INSTALL_MANIFEST_SIGNED_SIZE};
use sha2::{Digest, Sha256};

/// 安装失败码（`eneros.install-rejection.v1` code 集——词形逐字冻结）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallFailure {
    SourceNotFound,
    SourceOpen,
    SourceRead,
    ManifestFormat,
    Platform,
    KeyId,
    Rollback,
    Signature,
    PayloadDigest,
    TargetMissing,
    TargetAmbiguous,
    TargetGeometry,
    TargetNotBlank,
    TargetWrite,
    TargetReadback,
}

impl InstallFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SourceNotFound => "SOURCE_NOT_FOUND",
            Self::SourceOpen => "SOURCE_OPEN",
            Self::SourceRead => "SOURCE_READ",
            Self::ManifestFormat => "INSTALL_MANIFEST",
            Self::Platform => "PLATFORM",
            Self::KeyId => "KEY_ID",
            Self::Rollback => "ROLLBACK",
            Self::Signature => "SIGNATURE",
            Self::PayloadDigest => "PAYLOAD_DIGEST",
            Self::TargetMissing => "TARGET_MISSING",
            Self::TargetAmbiguous => "TARGET_AMBIGUOUS",
            Self::TargetGeometry => "TARGET_GEOMETRY",
            Self::TargetNotBlank => "TARGET_NOT_BLANK",
            Self::TargetWrite => "TARGET_WRITE",
            Self::TargetReadback => "TARGET_READBACK",
        }
    }
}

/// 安装信任锚（bin 侧由 build.rs 生成的 `installer_trust.rs` 常量构造；
/// host 测试现配。build.rs 信任生成链不动——WP-74.3.1 边界）.
#[derive(Debug, Clone, Copy)]
pub struct TrustAnchor {
    /// 信任公钥（32 字节）.
    pub public_key: [u8; 32],
    /// 信任 key ID（16 字节）.
    pub key_id: [u8; 16],
    /// 平台 ID（16 字节）.
    pub platform_id: [u8; 16],
    /// 最低安全版本（下限，低于即 Rollback）.
    pub minimum_security_version: u64,
}

/// 安装 manifest 验签（WP-74.3.1 重写：验签只经 `BootSignatureProvider`
/// 唯一入口；函数名与 [`InstallFailure`] 映射保持不变）.
///
/// 前置检查原序保留：`platform_id`/`key_id`/`security_version < min` →
/// `Platform`/`KeyId`/`Rollback`（词形与拒绝码不变）；随后签名域门
/// （算法/签名长度/密钥域/版本下限/dev-key 拒绝——新拒绝面）+ provider
/// 密码学验证，错误一律映射 `Signature`（code 集冻结，不新增拒绝码）。
///
/// domain 语义登记：installer 的被签消息是 manifest 被签区自身
/// （`encoded[..INSTALL_MANIFEST_SIGNED_SIZE]`），`payload_digest` 字段
/// 约束 PAYLOAD.RAW（不在被签消息内）——故 domain 取被签区现算摘要。
/// 对 installer 而言 provider 的摘要一致性检查是平凡真（domain 从同一段
/// 字节现算）；实质增益 = dev-key 拒绝面 + 算法/长度/域统一门 + 单点
/// 密码学路径 + 「唯一入口」宣称成真（蓝图 74 卷三 §1）.
pub fn verify_install_manifest(
    encoded: &[u8],
    anchor: &TrustAnchor,
) -> Result<InstallManifest, InstallFailure> {
    let manifest = InstallManifest::decode(encoded).map_err(|_| InstallFailure::ManifestFormat)?;
    if manifest.platform_id != anchor.platform_id {
        return Err(InstallFailure::Platform);
    }
    if manifest.key_id != anchor.key_id {
        return Err(InstallFailure::KeyId);
    }
    if manifest.security_version < anchor.minimum_security_version {
        // 与 check_signature_domain 的 `< recorded → Downgrade` 同向；
        // 拒绝码保持 Rollback（版本语义同向，蓝图 74 卷三 §1）.
        return Err(InstallFailure::Rollback);
    }
    let payload_digest: [u8; 32] = Sha256::digest(&encoded[..INSTALL_MANIFEST_SIGNED_SIZE]).into();
    // 16→32 位身份提升：manifest.key_id（16B）进 SignatureDomain（32B）。
    // 高半区以 0xDD 填充——谓词保持（x 全 0xDD ⇔ x‖0xDD 全 0xDD），使
    // check_signature_domain 的 dev-key 门对 16B 全 0xDD 的构建期测试
    // 签名产物真实可达（零填充会静默绕过该门——红测实锤）；key_id 在
    // 域门中仅消费 dev-key 谓词，填充不引入其他语义。
    let mut key_id_32 = [0xDDu8; 32];
    key_id_32[..16].copy_from_slice(&manifest.key_id);
    let domain = SignatureDomain {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: key_id_32,
        security_version: manifest.security_version,
        key_domain: KeyDomain::Normal,
        payload_digest,
    };
    check_signature_domain(
        &domain,
        SignatureAlgorithm::Ed25519,
        KeyDomain::Normal,
        anchor.minimum_security_version,
        false, // 安装链等同 release：dev/test key（key_id 全 0xDD）拒绝
        &manifest.signature,
    )
    .map_err(|_| InstallFailure::Signature)?;
    let provider = Ed25519SignatureProvider {
        public_key: anchor.public_key,
    };
    provider
        .verify(
            &domain,
            &encoded[..INSTALL_MANIFEST_SIGNED_SIZE],
            &manifest.signature,
        )
        .map_err(|_| InstallFailure::Signature)?;
    Ok(manifest)
}

/// ISO9660 逻辑扇区大小（安装介质固定 2048B——bin 侧介质过滤同值）.
pub const ISO_SECTOR_SIZE: usize = 2048;
/// 目录解析上限（越界即 `SourceOpen`——防解析放大）.
pub const ISO_MAX_DIRECTORY_SIZE: usize = 64 * 1024;

/// ISO 介质 IO 错误（bin 侧 BlockIO 失败映射；lib 侧再映射
/// [`InstallFailure`]）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoIo {
    /// 底层读失败.
    Read,
}

/// ISO 介质 seam（WP-74.3.2）：lib 解析逻辑只依赖本 trait——bin 以
/// `ScopedProtocol<BlockIO>` adapter 实现（UEFI 读管道），host 测试以
/// 内存扇区介质实现。`buf.len()` 必须为 [`ISO_SECTOR_SIZE`] 整数倍
///（非对齐读由 [`iso_read_exact`] 的 scratch 路径处理）.
pub trait IsoMedia {
    /// 从 `lba` 起读取连续扇区到 `buf`.
    fn read_exact(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IsoIo>;
}

/// ISO 目录记录命中的文件定位（首 LBA + 字节长度）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoFile {
    /// 起始逻辑扇区号.
    pub first_lba: u64,
    /// 文件字节长度.
    pub length: u64,
}

/// 定位安装介质上的 INSTALL.MAN / PAYLOAD.RAW（WP-74.3.2 泛型化迁入；
/// PVD magic 不符返回 `Ok(None)`——bin 侧跳过该介质继续扫描）.
pub fn find_iso_payload<M: IsoMedia + ?Sized>(
    media: &mut M,
    last_lba: u64,
) -> Result<Option<(IsoFile, IsoFile)>, InstallFailure> {
    let mut descriptor = [0u8; ISO_SECTOR_SIZE];
    media
        .read_exact(16, &mut descriptor)
        .map_err(|_| InstallFailure::SourceRead)?;
    if descriptor[0] != 1 || descriptor[1..6] != *b"CD001" || descriptor[6] != 1 {
        return Ok(None);
    }
    let root = parse_iso_record(&descriptor[156..], last_lba)?.ok_or(InstallFailure::SourceOpen)?;
    let eneros = find_iso_entry(media, root, last_lba, b"ENEROS", true)?
        .ok_or(InstallFailure::SourceNotFound)?;
    let manifest = find_iso_entry(media, eneros, last_lba, b"INSTALL.MAN", false)?
        .ok_or(InstallFailure::SourceNotFound)?;
    let payload = find_iso_entry(media, eneros, last_lba, b"PAYLOAD.RAW", false)?
        .ok_or(InstallFailure::SourceNotFound)?;
    Ok(Some((manifest, payload)))
}

/// 在目录文件内按名定位条目（WP-74.3.2 泛型化迁入；名称 `;1` 版本后缀
/// 剥离、目录标志位过滤、record_length=0 扇区跳步语义原样保留）.
pub fn find_iso_entry<M: IsoMedia + ?Sized>(
    media: &mut M,
    directory: IsoFile,
    last_lba: u64,
    expected: &[u8],
    expected_directory: bool,
) -> Result<Option<IsoFile>, InstallFailure> {
    let length = usize::try_from(directory.length).map_err(|_| InstallFailure::SourceOpen)?;
    if length == 0 || length > ISO_MAX_DIRECTORY_SIZE {
        return Err(InstallFailure::SourceOpen);
    }
    let mut bytes = alloc::vec![0u8; length];
    iso_read_exact(media, directory, 0, &mut bytes)?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let record_length = usize::from(bytes[offset]);
        if record_length == 0 {
            offset = offset
                .checked_add(ISO_SECTOR_SIZE)
                .map(|next| next / ISO_SECTOR_SIZE * ISO_SECTOR_SIZE)
                .ok_or(InstallFailure::SourceOpen)?;
            continue;
        }
        let record = bytes
            .get(offset..offset + record_length)
            .ok_or(InstallFailure::SourceOpen)?;
        let file = parse_iso_record(record, last_lba)?.ok_or(InstallFailure::SourceOpen)?;
        let name_length = usize::from(*record.get(32).ok_or(InstallFailure::SourceOpen)?);
        let mut name = record
            .get(33..33 + name_length)
            .ok_or(InstallFailure::SourceOpen)?;
        if let Some(index) = name.iter().position(|byte| *byte == b';') {
            name = &name[..index];
        }
        let is_directory = record.get(25).is_some_and(|flags| flags & 0x02 != 0);
        if name == expected && is_directory == expected_directory {
            return Ok(Some(file));
        }
        offset = offset
            .checked_add(record_length)
            .ok_or(InstallFailure::SourceOpen)?;
    }
    Ok(None)
}

/// 解析一条 ISO9660 目录记录（WP-74.3.2 原样迁入——已纯，唯一改动是
/// 返回错误类型随 lib 词表）.
pub fn parse_iso_record(record: &[u8], last_lba: u64) -> Result<Option<IsoFile>, InstallFailure> {
    let length = usize::from(*record.first().ok_or(InstallFailure::SourceOpen)?);
    if length == 0 {
        return Ok(None);
    }
    if length < 34 || record.len() < length {
        return Err(InstallFailure::SourceOpen);
    }
    let first_lba = u64::from(u32::from_le_bytes(
        record[2..6]
            .try_into()
            .map_err(|_| InstallFailure::SourceOpen)?,
    ));
    let file_length = u64::from(u32::from_le_bytes(
        record[10..14]
            .try_into()
            .map_err(|_| InstallFailure::SourceOpen)?,
    ));
    let blocks = file_length
        .checked_add(ISO_SECTOR_SIZE as u64 - 1)
        .map(|value| value / ISO_SECTOR_SIZE as u64)
        .ok_or(InstallFailure::SourceOpen)?;
    if file_length == 0
        || first_lba
            .checked_add(blocks)
            .filter(|end| *end <= last_lba + 1)
            .is_none()
    {
        return Err(InstallFailure::SourceOpen);
    }
    Ok(Some(IsoFile {
        first_lba,
        length: file_length,
    }))
}

/// ISO 文件内按字节偏移读满 `output`（WP-74.3.2 泛型化迁入；非对齐/
/// 跨扇区段经单扇区 scratch 路径，整扇区段直读）.
pub fn iso_read_exact<M: IsoMedia + ?Sized>(
    media: &mut M,
    file: IsoFile,
    offset: u64,
    mut output: &mut [u8],
) -> Result<(), InstallFailure> {
    let requested = u64::try_from(output.len()).map_err(|_| InstallFailure::SourceRead)?;
    if offset
        .checked_add(requested)
        .filter(|end| *end <= file.length)
        .is_none()
    {
        return Err(InstallFailure::SourceRead);
    }
    let mut absolute = file
        .first_lba
        .checked_mul(ISO_SECTOR_SIZE as u64)
        .and_then(|start| start.checked_add(offset))
        .ok_or(InstallFailure::SourceRead)?;
    let mut scratch = [0u8; ISO_SECTOR_SIZE];
    while !output.is_empty() {
        let within = usize::try_from(absolute % ISO_SECTOR_SIZE as u64)
            .map_err(|_| InstallFailure::SourceRead)?;
        if within == 0 && output.len() >= ISO_SECTOR_SIZE {
            let direct = output.len() / ISO_SECTOR_SIZE * ISO_SECTOR_SIZE;
            let (head, tail) = output.split_at_mut(direct);
            media
                .read_exact(absolute / ISO_SECTOR_SIZE as u64, head)
                .map_err(|_| InstallFailure::SourceRead)?;
            absolute = absolute
                .checked_add(direct as u64)
                .ok_or(InstallFailure::SourceRead)?;
            output = tail;
        } else {
            media
                .read_exact(absolute / ISO_SECTOR_SIZE as u64, &mut scratch)
                .map_err(|_| InstallFailure::SourceRead)?;
            let count = output.len().min(ISO_SECTOR_SIZE - within);
            output[..count].copy_from_slice(&scratch[within..within + count]);
            absolute = absolute
                .checked_add(count as u64)
                .ok_or(InstallFailure::SourceRead)?;
            output = &mut output[count..];
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use ed25519_dalek::{Signer, SigningKey};
    use eneros_boot_protocol::INSTALL_MANIFEST_SIZE;

    use super::*;

    /// 测试用 payload 长度：≥16 MiB 下限且 512 对齐（encode 校验域）.
    const PAYLOAD_LENGTH: u64 = 128 * 1024 * 1024;

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn anchor_for(key: &SigningKey, key_id: [u8; 16]) -> TrustAnchor {
        TrustAnchor {
            public_key: key.verifying_key().to_bytes(),
            key_id,
            platform_id: [0x41; 16],
            minimum_security_version: 7,
        }
    }

    fn signed_manifest(
        key: &SigningKey,
        key_id: [u8; 16],
        security_version: u64,
    ) -> [u8; INSTALL_MANIFEST_SIZE] {
        let manifest = InstallManifest::new(
            [0x41; 16],
            key_id,
            security_version,
            1_785_465_257,
            PAYLOAD_LENGTH,
            512,
            [3; 16],
            [4; 16],
            [5; 32],
        );
        let unsigned = manifest.encode_unsigned().unwrap();
        let signature = key.sign(&unsigned).to_bytes();
        manifest.with_signature(signature).encode_signed().unwrap()
    }

    /// 正例：信任锚现签 manifest 全过（解码 + 前置检查 + 域门 + provider）.
    #[test]
    fn signed_install_manifest_is_accepted() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);
        let manifest = verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])).unwrap();
        assert_eq!(manifest.payload_length, PAYLOAD_LENGTH);
        assert_eq!(manifest.security_version, 7);
    }

    /// 负例：签名区字节翻转 → 域门通过（长度/域/版本合法）但 provider
    /// 密码学验证拒绝 → `Signature`（不新增拒绝码）.
    #[test]
    fn flipped_signature_byte_is_rejected() {
        let key = signing_key(0x37);
        let mut encoded = signed_manifest(&key, [2; 16], 7);
        encoded[INSTALL_MANIFEST_SIGNED_SIZE] ^= 1;
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：异钥现签（key_id 冒用锚值）→ decode/前置检查全过，密码学
    /// 验证拒绝 → `Signature`.
    #[test]
    fn foreign_key_signature_is_rejected() {
        let trusted = signing_key(0x37);
        let attacker = signing_key(0x99);
        let encoded = signed_manifest(&attacker, [2; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&trusted, [2; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：security_version 低于锚下限 → `Rollback`（前置检查原序保留，
    /// 拒绝码词形不变）.
    #[test]
    fn security_version_below_minimum_maps_to_rollback() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 6);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])),
            Err(InstallFailure::Rollback)
        );
    }

    /// 负例：platform/key_id 不符 → 原码 `Platform`/`KeyId`.
    #[test]
    fn platform_and_key_id_mismatches_keep_original_codes() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);

        let mut anchor = anchor_for(&key, [2; 16]);
        anchor.platform_id = [0x42; 16];
        assert_eq!(
            verify_install_manifest(&encoded, &anchor),
            Err(InstallFailure::Platform)
        );

        let mut anchor = anchor_for(&key, [2; 16]);
        anchor.key_id = [3; 16];
        assert_eq!(
            verify_install_manifest(&encoded, &anchor),
            Err(InstallFailure::KeyId)
        );
    }

    /// **dev-key 新拒绝面**：key_id 全 0xDD（构建期测试签名产物）现签，
    /// 锚亦持同 key_id（前置 key_id 相等才走到域门）→
    /// `check_signature_domain` DevKeyRejected → `Signature`。修复前该
    /// manifest 经裸 verify_strict 直接放行（R15-C 指控面）.
    #[test]
    fn dev_key_is_rejected_through_provider_domain_gate() {
        let key = signing_key(0xDD);
        let encoded = signed_manifest(&key, [0xDD; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [0xDD; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：长度不符（截断/超长）→ `ManifestFormat`（decode 长度域）.
    #[test]
    fn wrong_length_encoded_is_manifest_format() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded[..200], &anchor_for(&key, [2; 16])),
            Err(InstallFailure::ManifestFormat)
        );
        assert_eq!(
            verify_install_manifest(&encoded[..255], &anchor_for(&key, [2; 16])),
            Err(InstallFailure::ManifestFormat)
        );
    }

    /// 词形冻结锁：`eneros.install-rejection.v1` code 集逐字冻结
    ///（15 词形，不新增不 renaming）.
    #[test]
    fn install_rejection_code_set_is_frozen() {
        let expected: [(&'static str, &'static str); 15] = [
            ("SourceNotFound", "SOURCE_NOT_FOUND"),
            ("SourceOpen", "SOURCE_OPEN"),
            ("SourceRead", "SOURCE_READ"),
            ("ManifestFormat", "INSTALL_MANIFEST"),
            ("Platform", "PLATFORM"),
            ("KeyId", "KEY_ID"),
            ("Rollback", "ROLLBACK"),
            ("Signature", "SIGNATURE"),
            ("PayloadDigest", "PAYLOAD_DIGEST"),
            ("TargetMissing", "TARGET_MISSING"),
            ("TargetAmbiguous", "TARGET_AMBIGUOUS"),
            ("TargetGeometry", "TARGET_GEOMETRY"),
            ("TargetNotBlank", "TARGET_NOT_BLANK"),
            ("TargetWrite", "TARGET_WRITE"),
            ("TargetReadback", "TARGET_READBACK"),
        ];
        for (variant, code) in expected {
            assert_eq!(failure_code_by_variant_name(variant), code);
        }
    }

    /// 按变体名取 code（仅供冻结锁使用，避免为测试暴露反射面）.
    fn failure_code_by_variant_name(name: &str) -> &'static str {
        match name {
            "SourceNotFound" => InstallFailure::SourceNotFound.code(),
            "SourceOpen" => InstallFailure::SourceOpen.code(),
            "SourceRead" => InstallFailure::SourceRead.code(),
            "ManifestFormat" => InstallFailure::ManifestFormat.code(),
            "Platform" => InstallFailure::Platform.code(),
            "KeyId" => InstallFailure::KeyId.code(),
            "Rollback" => InstallFailure::Rollback.code(),
            "Signature" => InstallFailure::Signature.code(),
            "PayloadDigest" => InstallFailure::PayloadDigest.code(),
            "TargetMissing" => InstallFailure::TargetMissing.code(),
            "TargetAmbiguous" => InstallFailure::TargetAmbiguous.code(),
            "TargetGeometry" => InstallFailure::TargetGeometry.code(),
            "TargetNotBlank" => InstallFailure::TargetNotBlank.code(),
            "TargetWrite" => InstallFailure::TargetWrite.code(),
            "TargetReadback" => InstallFailure::TargetReadback.code(),
            _ => unreachable!(),
        }
    }

    // ===== ISO9660 解析 fixture 套件（WP-74.3.2：合成字节，无外部文件） =====

    /// 内存扇区介质：按 ISO_SECTOR_SIZE 粒度服务 [`IsoMedia`] 读.
    struct MockMedia {
        sectors: Vec<Vec<u8>>,
        fail: bool,
    }

    impl MockMedia {
        fn new(sectors: Vec<Vec<u8>>) -> Self {
            Self {
                sectors,
                fail: false,
            }
        }

        fn failing(sectors: Vec<Vec<u8>>) -> Self {
            Self {
                sectors,
                fail: true,
            }
        }
    }

    impl IsoMedia for MockMedia {
        fn read_exact(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IsoIo> {
            if self.fail {
                return Err(IsoIo::Read);
            }
            assert_eq!(buf.len() % ISO_SECTOR_SIZE, 0, "trait 契约：整扇区读");
            let count = buf.len() / ISO_SECTOR_SIZE;
            for k in 0..count {
                let index = usize::try_from(lba).unwrap() + k;
                let sector = self.sectors.get(index).ok_or(IsoIo::Read)?;
                buf[k * ISO_SECTOR_SIZE..(k + 1) * ISO_SECTOR_SIZE].copy_from_slice(sector);
            }
            Ok(())
        }
    }

    /// 构造一条 ISO9660 目录记录（name 顶格；偶对齐填充）.
    fn iso_record(name: &[u8], first_lba: u32, length: u32, is_dir: bool) -> Vec<u8> {
        let total = 33 + name.len();
        let padded = total + (total & 1);
        let mut record = vec![0u8; padded];
        record[0] = padded as u8;
        record[1] = 0; // ext attr length
        record[2..6].copy_from_slice(&first_lba.to_le_bytes());
        record[10..14].copy_from_slice(&length.to_le_bytes());
        if is_dir {
            record[25] = 0x02;
        }
        record[28] = 1; // volume sequence number（ISO9660 §9.1；解析器不读，file version 是名称的 `;1` 后缀）
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    fn blank_sector() -> Vec<u8> {
        vec![0u8; ISO_SECTOR_SIZE]
    }

    fn pad_sector(record: &[u8]) -> Vec<u8> {
        let mut sector = blank_sector();
        sector[..record.len()].copy_from_slice(record);
        sector
    }

    /// 合成正例 ISO：系统区 16 扇区 + PVD（LBA16）+ 根目录（LBA17，指向
    /// ENEROS 目录 LBA18）+ ENEROS 目录（LBA18：INSTALL.MAN LBA19 /
    /// PAYLOAD.RAW LBA20..22）+ 文件内容扇区.
    fn fixture_iso() -> (MockMedia, IsoFile, IsoFile) {
        let mut sectors = Vec::new();
        for _ in 0..16 {
            sectors.push(blank_sector());
        }
        // PVD：type=1 / "CD001" / version=1；根目录记录 @156。
        let mut pvd = blank_sector();
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        let root_record = iso_record(b"\x00", 17, ISO_SECTOR_SIZE as u32, true);
        pvd[156..156 + root_record.len()].copy_from_slice(&root_record);
        sectors.push(pvd);
        // 根目录：一条 ENEROS 目录记录。
        let root_dir = iso_record(b"ENEROS", 18, ISO_SECTOR_SIZE as u32, true);
        sectors.push(pad_sector(&root_dir));
        // ENEROS 目录：INSTALL.MAN（1 扇区）+ PAYLOAD.RAW（3 扇区）。
        let mut eneros_dir = blank_sector();
        let man_record = iso_record(b"INSTALL.MAN", 19, ISO_SECTOR_SIZE as u32, false);
        let raw_record = iso_record(b"PAYLOAD.RAW", 20, (3 * ISO_SECTOR_SIZE) as u32, false);
        eneros_dir[..man_record.len()].copy_from_slice(&man_record);
        eneros_dir[man_record.len()..man_record.len() + raw_record.len()]
            .copy_from_slice(&raw_record);
        sectors.push(eneros_dir);
        // 文件内容：manifest 扇区 + payload 三扇区（可辨识数据）。
        let mut man_content = blank_sector();
        man_content[..8].copy_from_slice(b"ENRINST1");
        sectors.push(man_content);
        for k in 0..3u8 {
            sectors.push(vec![0xA0 + u64::from(k) as u8; ISO_SECTOR_SIZE]);
        }
        let media = MockMedia::new(sectors);
        let manifest = IsoFile {
            first_lba: 19,
            length: ISO_SECTOR_SIZE as u64,
        };
        let payload = IsoFile {
            first_lba: 20,
            length: 3 * ISO_SECTOR_SIZE as u64,
        };
        (media, manifest, payload)
    }

    /// 正例：PVD→根目录→ENEROS→INSTALL.MAN/PAYLOAD.RAW 全链定位.
    #[test]
    fn find_iso_payload_locates_manifest_and_payload() {
        let (mut media, manifest, payload) = fixture_iso();
        let last_lba = (media.sectors.len() - 1) as u64;
        let found = find_iso_payload(&mut media, last_lba).expect("正例必须定位成功");
        assert_eq!(found, Some((manifest, payload)));
    }

    /// 负例：PVD "CD001" magic 损坏 → Ok(None)（bin 侧跳过该介质）.
    #[test]
    fn find_iso_payload_skips_media_with_bad_pvd_magic() {
        let (mut media, _manifest, _payload) = fixture_iso();
        media.sectors[16][2] = b'X';
        let last_lba = (media.sectors.len() - 1) as u64;
        assert_eq!(find_iso_payload(&mut media, last_lba), Ok(None));
    }

    /// 负例：介质读失败 → `SourceRead`.
    #[test]
    fn find_iso_payload_maps_media_failure_to_source_read() {
        let (media, _manifest, _payload) = fixture_iso();
        let mut media = MockMedia::failing(media.sectors);
        let last_lba = (media.sectors.len() - 1) as u64;
        assert_eq!(
            find_iso_payload(&mut media, last_lba),
            Err(InstallFailure::SourceRead)
        );
    }

    /// 语义：名称 `;1` 版本后缀剥离（INSTALL.MAN;1 命中 INSTALL.MAN 查找）.
    #[test]
    fn find_iso_entry_strips_iso9660_version_suffix() {
        let mut sectors = Vec::new();
        for _ in 0..19 {
            sectors.push(blank_sector());
        }
        let mut dir = blank_sector();
        let record = iso_record(b"INSTALL.MAN;1", 19, ISO_SECTOR_SIZE as u32, false);
        dir[..record.len()].copy_from_slice(&record);
        sectors.push(dir);
        sectors.push(blank_sector());
        let mut media = MockMedia::new(sectors);
        let directory = IsoFile {
            first_lba: 19,
            length: ISO_SECTOR_SIZE as u64,
        };
        let found = find_iso_entry(&mut media, directory, 20, b"INSTALL.MAN", false).unwrap();
        assert_eq!(
            found,
            Some(IsoFile {
                first_lba: 19,
                length: ISO_SECTOR_SIZE as u64
            })
        );
    }

    /// 语义：目录标志位（0x02）过滤——文件查找不命中目录项，目录查找
    /// 不命中文件项（双向）.
    #[test]
    fn find_iso_entry_filters_directory_flag_both_ways() {
        let mut sectors = Vec::new();
        for _ in 0..19 {
            sectors.push(blank_sector());
        }
        let mut dir = blank_sector();
        let as_file = iso_record(b"ENEROS", 18, ISO_SECTOR_SIZE as u32, false);
        dir[..as_file.len()].copy_from_slice(&as_file);
        sectors.push(dir);
        sectors.push(blank_sector());
        let mut media = MockMedia::new(sectors);
        let directory = IsoFile {
            first_lba: 19,
            length: ISO_SECTOR_SIZE as u64,
        };
        // 期望目录：同名文件项不命中。
        assert_eq!(
            find_iso_entry(&mut media, directory, 20, b"ENEROS", true).unwrap(),
            None
        );
        // 期望文件：同一文件项命中。
        assert!(find_iso_entry(&mut media, directory, 20, b"ENEROS", false)
            .unwrap()
            .is_some());
    }

    /// 语义：record_length=0 的扇区跳步——目录首扇区全零（记录终止带）
    /// 时跳到下一扇区边界继续解析.
    #[test]
    fn find_iso_entry_skips_zero_length_records_to_next_sector() {
        let mut sectors = Vec::new();
        for _ in 0..19 {
            sectors.push(blank_sector());
        }
        sectors.push(blank_sector()); // LBA19：全零目录首扇区
        let mut second = blank_sector();
        let record = iso_record(b"PAYLOAD.RAW", 20, ISO_SECTOR_SIZE as u32, false);
        second[..record.len()].copy_from_slice(&record);
        sectors.push(second); // LBA20：记录在次扇区
        sectors.push(blank_sector());
        let mut media = MockMedia::new(sectors);
        let directory = IsoFile {
            first_lba: 19,
            length: 2 * ISO_SECTOR_SIZE as u64,
        };
        assert_eq!(
            find_iso_entry(&mut media, directory, 21, b"PAYLOAD.RAW", false).unwrap(),
            Some(IsoFile {
                first_lba: 20,
                length: ISO_SECTOR_SIZE as u64
            })
        );
    }

    /// 负例：目录长度越 64 KiB 上限 → `SourceOpen`.
    #[test]
    fn find_iso_entry_rejects_oversized_directory() {
        let mut media = MockMedia::new(vec![blank_sector()]);
        let directory = IsoFile {
            first_lba: 0,
            length: ISO_MAX_DIRECTORY_SIZE as u64 + ISO_SECTOR_SIZE as u64,
        };
        assert_eq!(
            find_iso_entry(&mut media, directory, 0, b"X", false),
            Err(InstallFailure::SourceOpen)
        );
    }

    /// 负例：记录长度 < 34（小于最短记录）→ `SourceOpen`.
    #[test]
    fn parse_iso_record_rejects_short_record() {
        let mut record = vec![0u8; 33];
        record[0] = 33;
        assert_eq!(
            parse_iso_record(&record, 0),
            Err(InstallFailure::SourceOpen)
        );
    }

    /// 负例：零长度文件 → `SourceOpen`（空文件不可安装）.
    #[test]
    fn parse_iso_record_rejects_zero_length_file() {
        let record = iso_record(b"PAYLOAD.RAW", 20, 0, false);
        assert_eq!(
            parse_iso_record(&record, 64),
            Err(InstallFailure::SourceOpen)
        );
    }

    /// 负例：越界 LBA（first_lba+blocks > last_lba+1）→ `SourceOpen`.
    #[test]
    fn parse_iso_record_rejects_out_of_bounds_lba() {
        // 3 扇区文件从 LBA 63 起 → 末端 66 > last_lba+1 = 65。
        let record = iso_record(b"PAYLOAD.RAW", 63, 3 * ISO_SECTOR_SIZE as u32, false);
        assert_eq!(
            parse_iso_record(&record, 64),
            Err(InstallFailure::SourceOpen)
        );
        // 恰好贴边（末端 == last_lba+1）合法。
        assert!(parse_iso_record(&record, 65).is_ok());
    }

    /// 负例：读越界（offset+len 超过文件长度）→ `SourceRead`.
    #[test]
    fn iso_read_exact_rejects_overrun() {
        let (mut media, manifest, _payload) = fixture_iso();
        let mut out = [0u8; ISO_SECTOR_SIZE];
        assert_eq!(
            iso_read_exact(&mut media, manifest, 1, &mut out),
            Err(InstallFailure::SourceRead)
        );
    }

    /// 正例：非对齐与跨扇区读逐字节正确（scratch 路径 + 直读路径混用）.
    #[test]
    fn iso_read_exact_handles_unaligned_and_cross_sector_reads() {
        let (mut media, _manifest, payload) = fixture_iso();
        // 非对齐 1 字节（offset 1，payload 内容 = 0xA0）。
        let mut one = [0u8; 1];
        iso_read_exact(&mut media, payload, 1, &mut one).unwrap();
        assert_eq!(one[0], 0xA0);
        // 跨 3 扇区整段读回（2048 非整除长度：1 + 2×2048，恰至文件末端）。
        let mut span = [0u8; 2 * ISO_SECTOR_SIZE + 1];
        iso_read_exact(&mut media, payload, ISO_SECTOR_SIZE as u64 - 1, &mut span).unwrap();
        assert_eq!(span[0], 0xA0);
        assert_eq!(span[1], 0xA1);
        assert_eq!(span[ISO_SECTOR_SIZE], 0xA1);
        assert_eq!(span[span.len() - 1], 0xA2);
    }
}
