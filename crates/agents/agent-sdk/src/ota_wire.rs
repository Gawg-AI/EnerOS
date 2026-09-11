//! OTA1 探针 wire（agent-sdk 共享模块，蓝图 58 C3）。
//!
//! 单一事实：root-task 探针（storage_persist_probe / ota_download_probe）
//! 的手写请求构造 + 响应解析，与 svc-ota handler 的手写请求解析曾各自实现
//! 同一布局——漂移只能在 QEMU 日志现形。本模块收归一处，root 与 svc-ota
//! 共同消费（WP-45.5 sample_wire 先例疗法）。
//!
//! 布局（小端；magic = `OTA1`）：
//! - 0x01 CHECK-DESC（70.5 v2）：req = magic‖0x01‖描述 158B‖desc_sig 64B；
//!   resp = verdict u8(1=Pass) 或 `ERR:...`。旧 52B 无签名形 = 协议违例。
//! - 0x02 DOWNLOAD-DESC（70.5 v2）：req = magic‖0x02‖描述 158B‖
//!   desc_sig 64B‖image_sig 64B‖chunk_size u32‖image_len u32‖image；
//!   resp = 子码回显 u8(=2) 或 `ERR:...`。旧无描述形失败关闭
//!   （WP-70.5.3：生产不回退到不绑定 metadata 的写路径）。
//! - 0x06 PERSIST-TRIAL：req = magic‖0x06‖remaining u32‖version 16B；
//!   resp = gen u64 或 `ERR:...`
//! - 0x07 LOAD-TRIAL：req = magic‖0x07；
//!   resp = found u8(=1)‖remaining u32‖version 16B 或 `ERR:...`
//! - 0x08 IDENTITY（70.5）：req = magic‖0x08‖capsule 136B（boot-protocol
//!   `IdentityCapsule` 编码）；resp = ack u8(=1) 或 `ERR:...`。root 独占
//!   分发（可信启动路径投影）；服务侧仅接受一次（代次单调）。
//! - 0x03 TRIAL：req = magic‖0x03‖flag u8‖version 32B；resp 回显 3
//! - 0x04 COMMIT：req = magic‖0x04；resp 回显 4
//!
//! version 字段为定长 NUL 填充 UTF-8（截断到字段宽）。签名 = SM2 对
//! SM3(被签字节)（64B r‖s）；签名验证不在此模块（无加密依赖纪律），
//! 见 `boot-protocol` capsule / `ota_service` 描述验签。

use alloc::string::String;
use alloc::vec::Vec;

/// wire magic。
pub const MAGIC: [u8; 4] = *b"OTA1";
/// CHECK-DESC 子码（70.5 v2：签名描述准入检查）。
pub const SUB_CHECK: u8 = 0x01;
/// DOWNLOAD 子码。
pub const SUB_DOWNLOAD: u8 = 0x02;
/// TRIAL 子码。
pub const SUB_TRIAL: u8 = 0x03;
/// COMMIT 子码。
pub const SUB_COMMIT: u8 = 0x04;
/// PERSIST-TRIAL 子码。
pub const SUB_PERSIST_TRIAL: u8 = 0x06;
/// LOAD-TRIAL 子码。
pub const SUB_LOAD_TRIAL: u8 = 0x07;
/// IDENTITY 子码（root → svc-ota 可信身份分发）。
pub const SUB_IDENTITY: u8 = 0x08;
/// POLICY-INFO 子码（root 查询策略代次 + 锚指纹——描述签发锚定；
/// 只读观测面，零密钥材料出域）。
pub const SUB_POLICY: u8 = 0x09;

/// version 字段宽（persist/load 用短版）。
pub const VERSION_LEN_SHORT: usize = 16;
/// version 字段宽（download/trial 用长版）。
pub const VERSION_LEN_LONG: usize = 32;
/// sku 字段宽。
pub const SKU_LEN: usize = 16;
/// 服务标识字段宽。
pub const SERVICE_LEN: usize = 16;

/// 解析失败类型（`ERR:` 前缀语义随迁——41 卷实弹教训：
/// 不检查前缀会把错误串前 8 字节解成数字打印假成功行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaWireError {
    /// 服务侧拒绝响应（`ERR:` 前缀，载荷为可读消息）。
    Rejected(String),
    /// 响应短于布局要求。
    Short(usize),
    /// 子码回显不符。
    UnexpectedSubcode(u8),
    /// found 标志非 1（trial 未找到）。
    NotFound,
    /// 定长字符串含非 UTF-8。
    BadUtf8,
    /// 字段超宽（描述编码拒绝静默截断——截断版本 = 元数据绑定失真）。
    FieldTooLong,
}

/// NUL 填充定长字段编码（超长截断）。
fn fill_field(buf: &mut Vec<u8>, value: &[u8], width: usize) {
    let n = core::cmp::min(value.len(), width);
    buf.extend_from_slice(&value[..n]);
    buf.resize(buf.len() + (width - n), 0);
}

/// 定长字段解码为去 NUL 字符串。
fn field_to_string(raw: &[u8]) -> Result<String, OtaWireError> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..end])
        .map(alloc::string::String::from)
        .map_err(|_| OtaWireError::BadUtf8)
}

/// 公共响应检查：`ERR:` 前缀 → Rejected。
fn check_err_prefix(resp: &[u8]) -> Result<(), OtaWireError> {
    if resp.starts_with(b"ERR:") {
        return Err(OtaWireError::Rejected(alloc::string::String::from(
            core::str::from_utf8(resp).unwrap_or("<bad-utf8>"),
        )));
    }
    Ok(())
}

/// 0x06 PERSIST-TRIAL 请求：remaining ‖ version(16)。
pub fn encode_persist_trial(remaining: u32, version: &str) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_PERSIST_TRIAL);
    req.extend_from_slice(&remaining.to_le_bytes());
    fill_field(&mut req, version.as_bytes(), VERSION_LEN_SHORT);
    req
}

/// 服务侧解析 0x06 请求 → `(remaining, version)`。
pub fn parse_persist_trial_req(req: &[u8]) -> Result<(u32, String), OtaWireError> {
    // 布局：magic(4)+sub(1)+remaining(4)+version(16)。
    if req.len() != 5 + 4 + VERSION_LEN_SHORT || req[..4] != MAGIC || req[4] != SUB_PERSIST_TRIAL {
        return Err(OtaWireError::Short(req.len()));
    }
    let p = &req[5..];
    let remaining = u32::from_le_bytes(p[0..4].try_into().expect("u32"));
    let version = field_to_string(&p[4..20])?;
    Ok((remaining, version))
}

/// 0x06 响应解析 → 持久化代次。
pub fn parse_persist_resp(resp: &[u8]) -> Result<u64, OtaWireError> {
    check_err_prefix(resp)?;
    if resp.len() < 8 {
        return Err(OtaWireError::Short(resp.len()));
    }
    Ok(u64::from_le_bytes(resp[..8].try_into().expect("u64")))
}

/// 0x07 LOAD-TRIAL 请求。
pub fn encode_load_trial() -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_LOAD_TRIAL);
    req
}

/// 0x07 响应解析 → `(remaining, version)`（found != 1 → NotFound）。
pub fn parse_load_resp(resp: &[u8]) -> Result<(u32, String), OtaWireError> {
    check_err_prefix(resp)?;
    if resp.len() < 1 + 4 {
        return Err(OtaWireError::Short(resp.len()));
    }
    if resp[0] != 1 {
        return Err(OtaWireError::NotFound);
    }
    let remaining = u32::from_le_bytes(resp[1..5].try_into().expect("u32"));
    let version = field_to_string(&resp[5..])?;
    Ok((remaining, version))
}

/// 子码回显响应校验（0x01/0x02/0x03/0x04 通用）。
pub fn expect_subcode_resp(resp: &[u8], subcode: u8) -> Result<(), OtaWireError> {
    check_err_prefix(resp)?;
    if resp.is_empty() {
        return Err(OtaWireError::Short(0));
    }
    if resp[0] != subcode {
        return Err(OtaWireError::UnexpectedSubcode(resp[0]));
    }
    Ok(())
}

/// 0x03 TRIAL 请求：flag ‖ version(32)。
pub fn encode_trial(flag: u8, version: &[u8]) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_TRIAL);
    req.push(flag);
    fill_field(&mut req, version, VERSION_LEN_LONG);
    req
}

/// 0x04 COMMIT 请求。
pub fn encode_commit() -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_COMMIT);
    req
}

// ===========================================================================
// 70.5（WP-70.5.1/70.5.3/70.5.4）：签名更新描述 + 可信身份分发 wire。
//
// 描述（UpdateDescription）= 版本化签名更新描述（方向 3 §4.2 方案 B）：
// 一份已验签事实贯穿 CHECK 与写盘——签名绑定用途、SKU、目标/基线身份、
// 安全版本、长度、镜像摘要与策略代次；镜像摘要入签 = 元数据↔镜像绑定。
// 编码定长 canonical（158B ≤ 4KiB 描述预算，方向 3 §7）。
// ===========================================================================

/// 描述魔数（"ENRUDC01"）。
pub const DESC_MAGIC: [u8; 8] = *b"ENRUDC01";
/// 描述协议版本。
pub const DESC_VERSION: u8 = 1;
/// 用途：ota-ab 镜像更新（唯一登记用途；其他值验签面拒绝）。
pub const DESC_USAGE_OTA_AB_IMAGE: u8 = 1;
/// 描述 sku 字段宽（= 平台绑定指纹 hex 形态 32 chars——identity 面同源；
/// 70.5 实弹订正：16B 装不下 hex32 → CHECK sku-mismatch 假拒）。
pub const DESC_SKU_LEN: usize = 32;
/// 描述编码长度（字节）。
pub const DESC_SIZE: usize = 158;
/// SM2 签名长度（r‖s）。
pub const SIG_LEN: usize = 64;
/// boot-protocol IdentityCapsule 编码长度（0x08 载荷）。
pub const IDENTITY_CAPSULE_WIRE_LEN: usize = 136;

/// 版本化签名更新描述（codec 面：纯数据 + canonical 编解码；验签归
/// 消费方 ota_service）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDescription {
    /// 用途（[`DESC_USAGE_OTA_AB_IMAGE`]；防"同名 root_key 跨用途信任"）。
    pub usage: u8,
    /// 目标设备 SKU。
    pub sku: String,
    /// 目标版本。
    pub target_version: String,
    /// 基线版本（签名时认可的当前版本——防旧描述漂移复用）。
    pub baseline_version: String,
    /// 目标安全版本（≥ 水位才准入；与产品语义版本分域）。
    pub security_version: u32,
    /// 镜像长度（字节；与实际传输长度强等）。
    pub image_len: u64,
    /// 镜像 SM3 摘要（入签 = 元数据↔镜像绑定）。
    pub image_hash: [u8; 32],
    /// 签发时 OTA 策略代次（配置束 generation；代次回退 = 拒绝）。
    pub policy_generation: u64,
}

/// NUL 填充定长字段编码（超宽拒绝——描述域不容截断）。
fn fill_field_strict(buf: &mut Vec<u8>, value: &str, width: usize) -> Result<(), OtaWireError> {
    if value.len() > width {
        return Err(OtaWireError::FieldTooLong);
    }
    buf.extend_from_slice(value.as_bytes());
    buf.resize(buf.len() + (width - value.len()), 0);
    Ok(())
}

impl UpdateDescription {
    /// canonical 编码（定长 158B；字段超宽 = `FieldTooLong`）。
    pub fn encode(&self) -> Result<[u8; DESC_SIZE], OtaWireError> {
        let mut b = Vec::with_capacity(DESC_SIZE);
        b.extend_from_slice(&DESC_MAGIC);
        b.push(DESC_VERSION);
        b.push(self.usage);
        fill_field_strict(&mut b, &self.sku, DESC_SKU_LEN)?;
        fill_field_strict(&mut b, &self.target_version, VERSION_LEN_LONG)?;
        fill_field_strict(&mut b, &self.baseline_version, VERSION_LEN_LONG)?;
        b.extend_from_slice(&self.security_version.to_le_bytes());
        b.extend_from_slice(&self.image_len.to_le_bytes());
        b.extend_from_slice(&self.image_hash);
        b.extend_from_slice(&self.policy_generation.to_le_bytes());
        let out: [u8; DESC_SIZE] = b.as_slice().try_into().expect("定长描述");
        Ok(out)
    }

    /// 严格解码：定长 + magic + 版本 + canonical 重编码一致（未登记
    /// 字节形态一律拒绝——与 capsule decode 同纪律）。
    pub fn decode(bytes: &[u8]) -> Result<Self, OtaWireError> {
        if bytes.len() != DESC_SIZE {
            return Err(OtaWireError::Short(bytes.len()));
        }
        if bytes[0..8] != DESC_MAGIC {
            return Err(OtaWireError::Short(bytes.len()));
        }
        if bytes[8] != DESC_VERSION {
            return Err(OtaWireError::UnexpectedSubcode(bytes[8]));
        }
        let desc = Self {
            usage: bytes[9],
            sku: field_to_string(&bytes[10..42])?,
            target_version: field_to_string(&bytes[42..74])?,
            baseline_version: field_to_string(&bytes[74..106])?,
            security_version: u32::from_le_bytes(bytes[106..110].try_into().expect("u32")),
            image_len: u64::from_le_bytes(bytes[110..118].try_into().expect("u64")),
            image_hash: bytes[118..150].try_into().expect("32B hash"),
            policy_generation: u64::from_le_bytes(bytes[150..158].try_into().expect("u64")),
        };
        if desc.encode()?.as_slice() != bytes {
            return Err(OtaWireError::Short(bytes.len()));
        }
        Ok(desc)
    }
}

/// 0x01 CHECK-DESC 请求：描述 158B ‖ desc_sig 64B。
pub fn encode_check_desc(desc: &UpdateDescription, desc_sig: &[u8; SIG_LEN]) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_CHECK);
    req.extend_from_slice(&desc.encode().expect("描述字段已在构造域校验"));
    req.extend_from_slice(desc_sig);
    req
}

/// 服务侧解析 0x01 CHECK-DESC → `(描述, desc_sig)`。
pub fn parse_check_desc_req(
    req: &[u8],
) -> Result<(UpdateDescription, [u8; SIG_LEN]), OtaWireError> {
    const BODY: usize = DESC_SIZE + SIG_LEN;
    if req.len() != 5 + BODY || req[..4] != MAGIC || req[4] != SUB_CHECK {
        return Err(OtaWireError::Short(req.len()));
    }
    let desc = UpdateDescription::decode(&req[5..5 + DESC_SIZE])?;
    let sig = req[5 + DESC_SIZE..5 + BODY].try_into().expect("64B sig");
    Ok((desc, sig))
}

/// 0x02 DOWNLOAD-DESC（v2）：描述 ‖ desc_sig ‖ image_sig ‖ chunk_size ‖
/// image_len u32 ‖ image。
pub fn encode_download_v2(
    desc: &UpdateDescription,
    desc_sig: &[u8; SIG_LEN],
    image_sig: &[u8; SIG_LEN],
    chunk_size: u32,
    image: &[u8],
) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_DOWNLOAD);
    req.extend_from_slice(&desc.encode().expect("描述字段已在构造域校验"));
    req.extend_from_slice(desc_sig);
    req.extend_from_slice(image_sig);
    req.extend_from_slice(&chunk_size.to_le_bytes());
    req.extend_from_slice(&(image.len() as u32).to_le_bytes());
    req.extend_from_slice(image);
    req
}

/// 0x02 v2 解析产物（描述 + 双签 + chunk + 借自请求的镜像字节）。
pub struct DownloadReq<'a> {
    pub desc: UpdateDescription,
    pub desc_sig: [u8; SIG_LEN],
    pub image_sig: [u8; SIG_LEN],
    pub chunk_size: u32,
    pub image: &'a [u8],
}

/// 服务侧解析 0x02 v2 请求。镜像字节以 `&[u8]` 借自请求（零拷贝）。
pub fn parse_download_v2_req(req: &[u8]) -> Result<DownloadReq<'_>, OtaWireError> {
    let hdr = 5 + DESC_SIZE + SIG_LEN + SIG_LEN + 4 + 4;
    if req.len() < hdr || req[..4] != MAGIC || req[4] != SUB_DOWNLOAD {
        return Err(OtaWireError::Short(req.len()));
    }
    let mut p = 5;
    let desc = UpdateDescription::decode(&req[p..p + DESC_SIZE])?;
    p += DESC_SIZE;
    let desc_sig = req[p..p + SIG_LEN].try_into().expect("64B");
    p += SIG_LEN;
    let image_sig = req[p..p + SIG_LEN].try_into().expect("64B");
    p += SIG_LEN;
    let chunk_size = u32::from_le_bytes(req[p..p + 4].try_into().expect("u32"));
    p += 4;
    let image_len = u32::from_le_bytes(req[p..p + 4].try_into().expect("u32")) as usize;
    p += 4;
    if req.len() - p != image_len {
        // 声明长度与实际载荷强等（长度漂移 = 请求残缺）。
        return Err(OtaWireError::Short(req.len()));
    }
    Ok(DownloadReq {
        desc,
        desc_sig,
        image_sig,
        chunk_size,
        image: &req[p..],
    })
}

/// 0x08 IDENTITY 请求：boot-protocol `IdentityCapsule` 编码（136B）。
pub fn encode_identity(capsule: &[u8; IDENTITY_CAPSULE_WIRE_LEN]) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_IDENTITY);
    req.extend_from_slice(capsule);
    req
}

/// 服务侧解析 0x08 请求 → capsule 编码字节（校验归消费方 boot-protocol）。
pub fn parse_identity_req(req: &[u8]) -> Result<&[u8; IDENTITY_CAPSULE_WIRE_LEN], OtaWireError> {
    if req.len() != 5 + IDENTITY_CAPSULE_WIRE_LEN || req[..4] != MAGIC || req[4] != SUB_IDENTITY {
        return Err(OtaWireError::Short(req.len()));
    }
    Ok(req[5..].try_into().expect("136B capsule"))
}

/// 0x09 POLICY-INFO 请求（无载荷）。
pub fn encode_policy_query() -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_POLICY);
    req
}

/// 0x09 响应解析 → `(策略代次, 锚指纹 8B)`。
pub fn parse_policy_resp(resp: &[u8]) -> Result<(u64, [u8; 8]), OtaWireError> {
    check_err_prefix(resp)?;
    if resp.len() < 16 {
        return Err(OtaWireError::Short(resp.len()));
    }
    let gen = u64::from_le_bytes(resp[..8].try_into().expect("u64"));
    let fp = resp[8..16].try_into().expect("8B fp");
    Ok((gen, fp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_round_trip_both_sides_one_type() {
        // root 编码 → 服务侧解析（同一模块 = 布局不可能漂移）。
        let req = encode_persist_trial(2, "1.2.0");
        assert_eq!(req.len(), 5 + 20);
        let (remaining, version) = parse_persist_trial_req(&req).expect("parse");
        assert_eq!(remaining, 2);
        assert_eq!(version, "1.2.0");
    }

    #[test]
    fn persist_resp_gen_and_error_paths() {
        assert_eq!(parse_persist_resp(&7u64.to_le_bytes()).expect("gen"), 7);
        assert_eq!(
            parse_persist_resp(b"ERR:store-unavailable"),
            Err(OtaWireError::Rejected(String::from(
                "ERR:store-unavailable"
            )))
        );
        assert_eq!(parse_persist_resp(&[1, 2]), Err(OtaWireError::Short(2)));
    }

    #[test]
    fn load_round_trip_not_found_and_short() {
        let mut resp = vec![1u8];
        resp.extend_from_slice(&2u32.to_le_bytes());
        let mut v = vec![b'1', b'.', b'2', b'.', b'0'];
        v.resize(16, 0);
        resp.extend_from_slice(&v);
        assert_eq!(
            parse_load_resp(&resp).expect("load"),
            (2, String::from("1.2.0"))
        );
        assert_eq!(
            parse_load_resp(&[0, 0, 0, 0, 0]),
            Err(OtaWireError::NotFound)
        );
        assert_eq!(
            parse_load_resp(b"ERR:x"),
            Err(OtaWireError::Rejected(String::from("ERR:x")))
        );
    }

    #[test]
    fn download_request_carries_all_fields_in_order() {
        let hash = [0xAB; 32];
        let desc_sig = [0x11; 64];
        let image_sig = [0x22; 64];
        let desc = UpdateDescription {
            usage: DESC_USAGE_OTA_AB_IMAGE,
            sku: String::from("qemu"),
            target_version: String::from("0.124.0"),
            baseline_version: String::from("0.123.0"),
            security_version: 2,
            image_len: 512,
            image_hash: hash,
            policy_generation: 1,
        };
        let image = [0u8; 512];
        let req = encode_download_v2(&desc, &desc_sig, &image_sig, 512, &image);
        // 布局钉扎：magic+sub+desc(158)+sig(64)+sig(64)+chunk(4)+len(4)+image
        let fixed = 5 + 158 + 64 + 64 + 4 + 4;
        assert_eq!(req.len(), fixed + image.len());
        assert_eq!(&req[..5], b"OTA1\x02");
        // 描述段可独立解码且逐字段保真。
        let parsed = parse_download_v2_req(&req).expect("parse v2");
        assert_eq!(parsed.desc, desc);
        assert_eq!(parsed.desc_sig, desc_sig);
        assert_eq!(parsed.image_sig, image_sig);
        assert_eq!(parsed.chunk_size, 512);
        assert_eq!(parsed.image, &image[..]);
    }

    /// 70.5：旧 v1 无描述 DOWNLOAD 形（52B 定长头）必须被 v2 解析拒绝
    /// （WP-70.5.3：生产不回退到不绑定 metadata 的旧 wire 写路径）。
    #[test]
    fn legacy_v1_download_layout_rejected() {
        // 旧 v1 形：target 32B‖sku 16B‖secver u32‖len u64‖hash‖sig‖chunk‖len。
        let mut req = Vec::from(MAGIC);
        req.push(0x02);
        req.resize(5 + 52 + 8, 0xEE);
        assert!(
            parse_download_v2_req(&req).is_err(),
            "旧 v1 形必须在描述魔数/长度域检被拒"
        );
    }

    #[test]
    fn desc_roundtrip_and_canonical_reject() {
        let desc = UpdateDescription {
            usage: DESC_USAGE_OTA_AB_IMAGE,
            sku: String::from("qemu"),
            target_version: String::from("1.2.3"),
            baseline_version: String::from("1.2.2"),
            security_version: 5,
            image_len: 1024,
            image_hash: [7; 32],
            policy_generation: 3,
        };
        let bytes = desc.encode().expect("encode");
        assert_eq!(bytes.len(), 158);
        assert_eq!(UpdateDescription::decode(&bytes), Ok(desc.clone()));
        // 字段超宽拒绝（不静默截断）。
        let mut wide = desc.clone();
        wide.target_version = String::from("012345678901234567890123456789012345");
        assert_eq!(wide.encode(), Err(OtaWireError::FieldTooLong));
        // 非 canonical 字节：字符串 NUL 填充区混入非零（重编码归零不等）
        // → 拒绝。数值域（secver/hash/gen）无冗余——防篡改由 desc_sig
        // 签名承担，codec 不重复（分层诚实）。
        let mut bad = desc.encode().unwrap();
        bad[15] ^= 0x01; // sku 填充区（sku hex32 占 [10..42]，尾部为零填充）
        assert!(UpdateDescription::decode(&bad).is_err());
        // 错误用途值可解码（语义拒绝在验签面——codec 不做策略）。
        let mut other_usage = desc.clone();
        other_usage.usage = 9;
        assert_eq!(
            UpdateDescription::decode(&other_usage.encode().unwrap()),
            Ok(other_usage)
        );
    }

    #[test]
    fn check_desc_roundtrip() {
        let desc = UpdateDescription {
            usage: DESC_USAGE_OTA_AB_IMAGE,
            sku: String::from("qemu"),
            target_version: String::from("2.0.0"),
            baseline_version: String::from("1.9.9"),
            security_version: 7,
            image_len: 0,
            image_hash: [1; 32],
            policy_generation: 2,
        };
        let sig = [0x5A; 64];
        let req = encode_check_desc(&desc, &sig);
        assert_eq!(req.len(), 5 + 158 + 64);
        let (d, s) = parse_check_desc_req(&req).expect("parse");
        assert_eq!(d, desc);
        assert_eq!(s, sig);
        // 旧 52B CHECK 形拒绝。
        let mut legacy = Vec::from(MAGIC);
        legacy.push(0x01);
        legacy.resize(5 + 52, 0);
        assert!(parse_check_desc_req(&legacy).is_err());
    }

    #[test]
    fn identity_roundtrip() {
        let mut capsule = [0u8; 136];
        capsule[..8].copy_from_slice(b"ENRIDC1\0");
        capsule[12] = 1; // slot A
        let req = encode_identity(&capsule);
        assert_eq!(req.len(), 5 + 136);
        assert_eq!(&req[..5], b"OTA1\x08");
        assert_eq!(parse_identity_req(&req), Ok(&capsule));
        // 长度/子码不符拒绝。
        assert!(parse_identity_req(&req[..20]).is_err());
        let mut wrong = req;
        wrong[4] = 0x07;
        assert!(parse_identity_req(&wrong).is_err());
    }

    #[test]
    fn subcode_echo_validation() {
        assert_eq!(expect_subcode_resp(&[3], 3), Ok(()));
        assert_eq!(
            expect_subcode_resp(&[9], 3),
            Err(OtaWireError::UnexpectedSubcode(9))
        );
        assert_eq!(expect_subcode_resp(&[], 3), Err(OtaWireError::Short(0)));
    }

    #[test]
    fn trial_and_commit_requests_shape() {
        let t = encode_trial(1, b"0.123.0");
        assert_eq!(&t[..6], b"OTA1\x03\x01");
        assert_eq!(t.len(), 5 + 1 + 32);
        assert_eq!(encode_commit(), Vec::from(&b"OTA1\x04"[..]));
    }
}
