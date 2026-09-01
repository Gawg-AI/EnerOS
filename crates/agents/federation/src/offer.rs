//! # Phase 7 联邦协议产物（蓝图 61 §12-Phase7 §2.2/§2.4）
//!
//! 联邦只交换签名的能力、报价、目标、承诺和执行证据；站内 Snapshot、
//! Certifier、Arbiter、Gateway、AuthorizedWriter 保持最终权威。
//!
//! 本模块定义协议 wire 产物与确定性编解码：
//!
//! - [`FedEnvelope`]：所有消息的签名信封——`FED1` magic + 版本 + kind +
//!   epoch/sequence（防重放）+ tenant/site + created/deadline + payload
//!   digest（SM3）+ signer_key_id + SM2 签名。签名覆盖除签名自身外的全部
//!   canonical 字节（含 digest），保证任何字段篡改都会验签失败。
//! - [`CapabilityOffer`] / [`OfferRevocation`] / [`AllocationProposal`] /
//!   [`AllocationCommitment`] / [`CommitmentAck`] / [`ProgressEvidence`] /
//!   [`SettlementEvidence`]：蓝图 §2.2 产物 + 流程所需 [`OfferReject`]。
//! - 全部 revision append-only；解码 fail-closed：非有限浮点（NaN/Inf）、
//!   截断、超长、magic/版本不符、digest 不符一律结构化拒绝。
//!
//! 隐私/数据最小化（蓝图 §2.5）：offer 只携带聚合能力（p/energy/ramp/
//! reserve 边界）+ 价格曲线 [`BlobRefV1`] 引用 + capability_root 摘要；
//! 本模块 wire 中不存在任何原始点位/工艺配方/prompt 字段（`data_minimized`
//! 测试固化）。

use alloc::vec::Vec;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{sm2_sign, sm2_verify, Sm2KeyPair, Sm2PublicKey, Sm2Signature};
use eneros_crypto::sm3::hash as sm3_hash;

/// 帧魔数：`FED1`。
pub const MAGIC: [u8; 4] = *b"FED1";
/// 本版本 wire schema 版本号（.47 冻结，任何字段增删/语义变更须 bump）。
pub const WIRE_VERSION: u8 = 1;
/// 单帧最大字节数（解码上限，防恶意超大帧挤占内存）。
pub const MAX_FRAME: usize = 65_536;
/// 单个 AllocationProposal 最大站点数（一个 proposal 只定向一个站点）。
pub const MAX_SITES_PER_PROPOSAL: u16 = 1;
/// 单条时域曲线最大区间数（96/288 时段资格上限）。
pub const MAX_INTERVALS: u16 = 288;
/// 时钟偏移容忍（毫秒）：`now < created_ns - skew` 视为未来消息拒绝。
pub const CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;

/// 消息种类（wire kind octet）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FedKind {
    /// 站点发布/修订 CapabilityOffer。
    Offer = 1,
    /// 站点结构化拒绝（OfferReject/CommitmentReject 共用）。
    Reject = 2,
    /// 协调器站间分配提议（每站点一条）。
    Proposal = 3,
    /// 双方签 AllocationCommitment（站点发起、协调器确认）。
    Commitment = 4,
    /// 协调器回执 CommitmentAck。
    Ack = 5,
    /// 站内执行聚合进度 ProgressEvidence。
    Progress = 6,
    /// 结算证据 SettlementEvidence。
    Settlement = 7,
    /// 能力变化撤销/修订 offer（append-only revocation）。
    Revoke = 8,
}

impl FedKind {
    /// 从 wire 字节解析；未知/保留值拒绝。
    pub fn from_u8(v: u8) -> Result<Self, WireError> {
        match v {
            1 => Ok(Self::Offer),
            2 => Ok(Self::Reject),
            3 => Ok(Self::Proposal),
            4 => Ok(Self::Commitment),
            5 => Ok(Self::Ack),
            6 => Ok(Self::Progress),
            7 => Ok(Self::Settlement),
            8 => Ok(Self::Revoke),
            _ => Err(WireError::BadKind),
        }
    }
}

/// 大对象引用（价格曲线/目标曲线/证据工件）：schema + digest + len。
/// 不内联大 payload，只引用；具体 artifact 由接收方经受控 BlobRef 取用
/// （蓝图 §2.5「调查时用受控 BlobRef、ACL」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobRefV1 {
    /// 引用 schema 版本（如 1=price-curve、2=target-curve、3=evidence）。
    pub schema: u8,
    /// SM3 digest（canonical 内容）。
    pub digest: [u8; 32],
    /// 内容字节长度。
    pub len: u64,
}

impl BlobRefV1 {
    /// 构造引用；不允许零 digest（引用必须可解析）。
    pub fn new(schema: u8, contents: &[u8]) -> Self {
        Self {
            schema,
            digest: sm3_hash(contents),
            len: contents.len() as u64,
        }
    }

    /// 校验引用指向的 contents 是否相符（digest + len 双检）。
    pub fn matches(&self, contents: &[u8]) -> bool {
        self.len == contents.len() as u64 && sm3_hash(contents) == self.digest
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.schema);
        out.extend_from_slice(&self.digest);
        out.extend_from_slice(&self.len.to_be_bytes());
    }

    fn decode(p: &mut Parser) -> Result<Self, WireError> {
        let schema = p.u8()?;
        let digest = p.bytes_32()?;
        let len = p.u64()?;
        if digest == [0u8; 32] {
            return Err(WireError::ZeroDigest);
        }
        Ok(Self {
            schema,
            digest,
            len,
        })
    }
}

/// 解码/校验错误（fail-closed 枚举，全部可观测）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// magic/版本不符。
    BadMagic,
    /// 版本号超前或未知。
    BadVersion,
    /// kind 未知/保留。
    BadKind,
    /// 帧超上限或剩余字节不足。
    Truncated,
    /// 帧超过 [`MAX_FRAME`]。
    Oversized,
    /// 浮点非有限（NaN/Inf）——严格拒绝。
    NonFinite,
    /// payload digest 与 SM3(body) 不符。
    DigestMismatch,
    /// BlobRef 零 digest。
    ZeroDigest,
    /// 业务字段越界（区间倒挂/计数超限/保留字段非零）。
    InvalidField,
}

/// 确定性 canonical 编码辅助（大端序写）。
struct Writer(Vec<u8>);

impl Writer {
    fn new() -> Self {
        Writer(Vec::new())
    }
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn f64(&mut self, v: f64) -> Result<(), WireError> {
        if !v.is_finite() {
            return Err(WireError::NonFinite);
        }
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// 确定性 canonical 解码辅助（大端序读，越界即 Truncated）。
struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Parser { buf, pos: 0 }
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        let v = *self.buf.get(self.pos).ok_or(WireError::Truncated)?;
        self.pos += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn f64(&mut self) -> Result<f64, WireError> {
        let b = self.take(8)?;
        let v = f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        if !v.is_finite() {
            return Err(WireError::NonFinite);
        }
        Ok(v)
    }
    fn bytes_32(&mut self) -> Result<[u8; 32], WireError> {
        let b = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(b);
        Ok(out)
    }
    fn bytes_16(&mut self) -> Result<[u8; 16], WireError> {
        let b = self.take(16)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(b);
        Ok(out)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        self.take(n)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.buf.len() - self.pos < n {
            return Err(WireError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn done(&self) -> Result<(), WireError> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(WireError::Truncated)
        }
    }
}

/// 签名信封：所有联邦消息的统一外层。
///
/// 防重放字段：`epoch`（协调器/站点纪元，重启递增）+ `sequence`
/// （同 epoch 内单调递增，接收方按 (peer, epoch) 记录 last_seq）。
/// deadline：接收方 `now_ns > deadline_ns` 即拒收（过期不投递）。
#[derive(Clone, Debug, PartialEq)]
pub struct FedEnvelope {
    /// 消息种类。
    pub kind: FedKind,
    /// 发送方纪元（防陈旧 leader/重启重放）。
    pub epoch: u64,
    /// 同纪元单调序列号。
    pub sequence: u64,
    /// 租户隔离键。
    pub tenant_id: u32,
    /// 发送方站点/协调器标识（协调器约定为 0）。
    pub site_id: u32,
    /// 发送时刻（接收方校验时钟偏移）。
    pub created_ns: u64,
    /// 响应 deadline（`now > deadline` 拒收）。
    pub deadline_ns: u64,
    /// 消息体（kind 专用 canonical 字节）。
    pub body: Vec<u8>,
    /// 签名 key id（接收方经 KeyRegistry 校验状态+验签）。
    pub signer_key_id: u32,
    /// SM2 签名（64B r‖s）。
    pub signature: [u8; 64],
}

/// 待签名信封内容（构造 → 签名 → [`FedEnvelope::into_signed`]）。
pub struct UnsignedEnvelope {
    pub kind: FedKind,
    pub epoch: u64,
    pub sequence: u64,
    pub tenant_id: u32,
    pub site_id: u32,
    pub created_ns: u64,
    pub deadline_ns: u64,
    pub body: Vec<u8>,
    pub signer_key_id: u32,
}

impl UnsignedEnvelope {
    /// 由 typed body 构造信封（digest 自动计算并随 canonical 一起签名）。
    pub fn from_body(kind: FedKind, hdr: &EnvelopeHdr, body: &[u8]) -> Self {
        Self {
            kind,
            epoch: hdr.epoch,
            sequence: hdr.sequence,
            tenant_id: hdr.tenant_id,
            site_id: hdr.site_id,
            created_ns: hdr.created_ns,
            deadline_ns: hdr.deadline_ns,
            body: body.to_vec(),
            signer_key_id: hdr.signer_key_id,
        }
    }

    /// canonical 待签字节（含 payload digest，不含 signature 本体）。
    pub fn canonical(&self) -> Result<Vec<u8>, WireError> {
        encode_canonical(
            self.kind,
            self.epoch,
            self.sequence,
            self.tenant_id,
            self.site_id,
            self.created_ns,
            self.deadline_ns,
            &self.body,
            self.signer_key_id,
        )
    }

    /// 签名（SM2，rng 注入）并封成 [`FedEnvelope`]。
    pub fn into_signed(self, kp: &Sm2KeyPair, rng: &mut CsRng) -> Result<FedEnvelope, WireError> {
        let canon = self.canonical()?;
        let sig = sm2_sign(&canon, &kp.private_key, &kp.public_key, rng)
            .map_err(|_| WireError::InvalidField)?;
        Ok(FedEnvelope {
            kind: self.kind,
            epoch: self.epoch,
            sequence: self.sequence,
            tenant_id: self.tenant_id,
            site_id: self.site_id,
            created_ns: self.created_ns,
            deadline_ns: self.deadline_ns,
            body: self.body,
            signer_key_id: self.signer_key_id,
            signature: sig.to_bytes(),
        })
    }
}

/// 信封头部（构造辅助）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeHdr {
    pub epoch: u64,
    pub sequence: u64,
    pub tenant_id: u32,
    pub site_id: u32,
    pub created_ns: u64,
    pub deadline_ns: u64,
    pub signer_key_id: u32,
}

/// canonical 编码（含 body digest，不含签名）。
/// `too_many_arguments`：8 个定长 wire 头字段为协议布局的直接映射，
/// 打包成结构体只会增加一层编解码且不影响调用面（保持确定性扁平布局）。
#[allow(clippy::too_many_arguments)]
pub fn encode_canonical(
    kind: FedKind,
    epoch: u64,
    sequence: u64,
    tenant_id: u32,
    site_id: u32,
    created_ns: u64,
    deadline_ns: u64,
    body: &[u8],
    signer_key_id: u32,
) -> Result<Vec<u8>, WireError> {
    let digest = sm3_hash(body);
    let mut w = Writer::new();
    w.bytes(&MAGIC);
    w.u8(WIRE_VERSION);
    w.u8(kind as u8);
    w.u64(epoch);
    w.u64(sequence);
    w.u32(tenant_id);
    w.u32(site_id);
    w.u64(created_ns);
    w.u64(deadline_ns);
    w.u32(body.len() as u32);
    w.bytes(&digest);
    w.u32(signer_key_id);
    w.bytes(body);
    Ok(w.finish())
}

/// 完整帧编码（信封 → 在线传输帧）；解码由 [`FedFrame::decode`] 承担。
pub struct FedFrame;

impl FedFrame {
    /// 帧 = canonical（含签名追加）。
    pub fn encode(env: &FedEnvelope) -> Result<Vec<u8>, WireError> {
        let canon = encode_canonical(
            env.kind,
            env.epoch,
            env.sequence,
            env.tenant_id,
            env.site_id,
            env.created_ns,
            env.deadline_ns,
            &env.body,
            env.signer_key_id,
        )?;
        if canon.len() + 64 > MAX_FRAME {
            return Err(WireError::Oversized);
        }
        let mut out = canon;
        out.extend_from_slice(&env.signature);
        Ok(out)
    }

    /// 解码帧并校验 magic/版本/长度/digest（签名验证由调用方经 KeyRegistry 做）。
    pub fn decode(frame: &[u8]) -> Result<FedEnvelope, WireError> {
        let mut p = Parser::new(frame);
        if p.bytes(4)? != &MAGIC[..] {
            return Err(WireError::BadMagic);
        }
        let version = p.u8()?;
        if version != WIRE_VERSION {
            return Err(WireError::BadVersion);
        }
        let kind = FedKind::from_u8(p.u8()?)?;
        let epoch = p.u64()?;
        let sequence = p.u64()?;
        let tenant_id = p.u32()?;
        let site_id = p.u32()?;
        let created_ns = p.u64()?;
        let deadline_ns = p.u64()?;
        let body_len = p.u32()? as usize;
        // 帧 = 86 头 + body + 64 签名；与 encode 侧 MAX_FRAME 对齐（P2-7）。
        if body_len > MAX_FRAME - 150 {
            return Err(WireError::Oversized);
        }
        let digest = p.bytes_32()?;
        let signer_key_id = p.u32()?;
        let body = p.bytes(body_len)?.to_vec();
        if sm3_hash(&body) != digest {
            return Err(WireError::DigestMismatch);
        }
        let signature_bytes = p.bytes(64)?;
        let mut signature = [0u8; 64];
        signature.copy_from_slice(signature_bytes);
        p.done()?;
        Ok(FedEnvelope {
            kind,
            epoch,
            sequence,
            tenant_id,
            site_id,
            created_ns,
            deadline_ns,
            body,
            signer_key_id,
            signature,
        })
    }

    /// 重新计算 canonical 待验签字节（与签名侧一致）。
    pub fn canonical(env: &FedEnvelope) -> Result<Vec<u8>, WireError> {
        encode_canonical(
            env.kind,
            env.epoch,
            env.sequence,
            env.tenant_id,
            env.site_id,
            env.created_ns,
            env.deadline_ns,
            &env.body,
            env.signer_key_id,
        )
    }

    /// 验签（`sm2_verify` 的布尔结果必须显式检查——`Ok(false)` 也是拒绝）。
    pub fn verify_signature(env: &FedEnvelope, pk: &Sm2PublicKey) -> Result<bool, WireError> {
        let canon = Self::canonical(env)?;
        let sig = Sm2Signature::from_bytes(&env.signature);
        sm2_verify(&canon, &sig, pk).map_err(|_| WireError::InvalidField)
    }
}

// ---------------------------------------------------------------
// 消息体（typed bodies）
// ---------------------------------------------------------------

/// 站点能力报价（蓝图 §2.2 原结构 + 防重放/租户/revision 扩展字段）。
///
/// 只含聚合能力与引用：`price_curve_ref` 指向价格曲线工件，
/// `capability_root` 为站内 Snapshot/Registry 能力根摘要——站点可随时
/// 用本地最新快照重新验证。`revision` 从 1 起递增，同 offer_id 更新
/// append-only。
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityOffer {
    pub offer_id: [u8; 16],
    pub interval_start_ns: u64,
    pub interval_end_ns: u64,
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub energy_min_kwh: f64,
    pub energy_max_kwh: f64,
    pub ramp_kw_s: f64,
    pub reserve_up_kw: f64,
    pub reserve_down_kw: f64,
    pub price_curve_ref: BlobRefV1,
    pub capability_root: [u8; 32],
    pub valid_until_ns: u64,
    pub revision: u64,
}

impl CapabilityOffer {
    /// canonical 编码（供签名/BlobRef 计算）。
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.revision == 0 {
            return Err(WireError::InvalidField);
        }
        if self.interval_end_ns <= self.interval_start_ns {
            return Err(WireError::InvalidField);
        }
        if self.p_max_kw < self.p_min_kw || self.p_min_kw < 0.0 {
            return Err(WireError::InvalidField);
        }
        if self.energy_max_kwh < self.energy_min_kwh || self.energy_min_kwh < 0.0 {
            return Err(WireError::InvalidField);
        }
        let mut w = Writer::new();
        w.bytes(&self.offer_id);
        w.u64(self.interval_start_ns);
        w.u64(self.interval_end_ns);
        w.f64(self.p_min_kw)?;
        w.f64(self.p_max_kw)?;
        w.f64(self.energy_min_kwh)?;
        w.f64(self.energy_max_kwh)?;
        w.f64(self.ramp_kw_s)?;
        w.f64(self.reserve_up_kw)?;
        w.f64(self.reserve_down_kw)?;
        self.price_curve_ref.encode(&mut w.0);
        w.bytes(&self.capability_root);
        w.u64(self.valid_until_ns);
        w.u64(self.revision);
        Ok(w.finish())
    }

    /// 解码 + 字段校验（fail-closed）。
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let offer_id = p.bytes_16()?;
        let interval_start_ns = p.u64()?;
        let interval_end_ns = p.u64()?;
        let p_min_kw = p.f64()?;
        let p_max_kw = p.f64()?;
        let energy_min_kwh = p.f64()?;
        let energy_max_kwh = p.f64()?;
        let ramp_kw_s = p.f64()?;
        let reserve_up_kw = p.f64()?;
        let reserve_down_kw = p.f64()?;
        let price_curve_ref = BlobRefV1::decode(&mut p)?;
        let capability_root = p.bytes_32()?;
        let valid_until_ns = p.u64()?;
        let revision = p.u64()?;
        p.done()?;
        let offer = Self {
            offer_id,
            interval_start_ns,
            interval_end_ns,
            p_min_kw,
            p_max_kw,
            energy_min_kwh,
            energy_max_kwh,
            ramp_kw_s,
            reserve_up_kw,
            reserve_down_kw,
            price_curve_ref,
            capability_root,
            valid_until_ns,
            revision,
        };
        // 复用 encode 的业务校验（区间倒挂/负值/revision=0）。
        offer.encode()?;
        Ok(offer)
    }
}

/// 结构化拒绝/撤销原因码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RejectReason {
    /// 目标超出站点能力包络（本地拒绝）。
    OutOfEnvelope = 1,
    /// 单位/时段网格与 tolerance catalog 不兼容。
    GridOrUnitMismatch = 2,
    /// 承诺/提议已过期。
    Expired = 3,
    /// offer 已撤销/修订（stale revision）。
    Superseded = 4,
    /// 本地重解/重认证失败（站内拒绝）。
    LocalReplanFailed = 5,
    /// 租户/身份不符。
    IdentityMismatch = 6,
    /// 其他/人工。
    Other = 7,
}

impl RejectReason {
    pub fn from_u8(v: u8) -> Result<Self, WireError> {
        match v {
            1 => Ok(Self::OutOfEnvelope),
            2 => Ok(Self::GridOrUnitMismatch),
            3 => Ok(Self::Expired),
            4 => Ok(Self::Superseded),
            5 => Ok(Self::LocalReplanFailed),
            6 => Ok(Self::IdentityMismatch),
            7 => Ok(Self::Other),
            _ => Err(WireError::InvalidField),
        }
    }
}

/// OfferReject（蓝图 §2.2）——结构化拒绝，detail 为人类可读说明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferReject {
    pub offer_id: [u8; 16],
    pub reason: RejectReason,
    pub detail: alloc::vec::Vec<u8>,
}

impl OfferReject {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.detail.len() > 1024 {
            return Err(WireError::Oversized);
        }
        let mut w = Writer::new();
        w.bytes(&self.offer_id);
        w.u8(self.reason as u8);
        w.u16(self.detail.len() as u16);
        w.bytes(&self.detail);
        Ok(w.finish())
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let id = p.bytes_16()?;
        let reason = RejectReason::from_u8(p.u8()?)?;
        let len = p.u16()? as usize;
        let detail = p.bytes(len)?.to_vec();
        p.done()?;
        // detail 上限与 encode 同限（decode/encode 对称）。
        if detail.len() > 1024 {
            return Err(WireError::Oversized);
        }
        Ok(Self {
            offer_id: id,
            reason,
            detail,
        })
    }
}

/// 站间分配提议（协调器 → 单一站点）：该站点目标曲线 + 备用 + tolerance。
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationProposal {
    pub proposal_id: [u8; 16],
    pub offer_id: [u8; 16],
    pub site_id: u32,
    pub tolerance_profile_id: u32,
    /// 曲线网格：`interval_start_ns` 起、`interval_ns` 步长、`count` 点。
    pub interval_start_ns: u64,
    pub interval_ns: u64,
    /// 逐区间目标功率（kW），有限、非负。
    pub points_kw: alloc::vec::Vec<f64>,
    pub reserve_up_kw: f64,
    pub reserve_down_kw: f64,
    /// 目标曲线工件引用（digest 与 points 的 canonical 一致）。
    pub target_curve_ref: BlobRefV1,
}

impl AllocationProposal {
    /// 目标曲线 canonical 字节（供 BlobRef 计算与站侧校验）。
    pub fn curve_canonical(&self) -> Result<Vec<u8>, WireError> {
        let mut w = Writer::new();
        w.u64(self.interval_start_ns);
        w.u64(self.interval_ns);
        w.u16(self.points_kw.len() as u16);
        for v in &self.points_kw {
            w.f64(*v)?;
        }
        Ok(w.finish())
    }

    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.points_kw.len() as u16 > MAX_INTERVALS {
            return Err(WireError::InvalidField);
        }
        if self.interval_ns == 0 {
            return Err(WireError::InvalidField);
        }
        if self.reserve_up_kw < 0.0 || self.reserve_down_kw < 0.0 {
            return Err(WireError::InvalidField); // 负备用拒绝（P2-2）
        }
        let curve = self.curve_canonical()?;
        if !self.target_curve_ref.matches(&curve) {
            return Err(WireError::DigestMismatch);
        }
        let mut w = Writer::new();
        w.bytes(&self.proposal_id);
        w.bytes(&self.offer_id);
        w.u32(self.site_id);
        w.u32(self.tolerance_profile_id);
        self.target_curve_ref.encode(&mut w.0);
        w.f64(self.reserve_up_kw)?;
        w.f64(self.reserve_down_kw)?;
        w.bytes(&curve);
        Ok(w.finish())
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let pid = p.bytes_16()?;
        let oid = p.bytes_16()?;
        let site_id = p.u32()?;
        let tolerance_profile_id = p.u32()?;
        let target_curve_ref = BlobRefV1::decode(&mut p)?;
        let reserve_up_kw = p.f64()?;
        let reserve_down_kw = p.f64()?;
        if reserve_up_kw < 0.0 || reserve_down_kw < 0.0 {
            return Err(WireError::InvalidField);
        }
        let interval_start_ns = p.u64()?;
        let interval_ns = p.u64()?;
        if interval_ns == 0 {
            return Err(WireError::InvalidField);
        }
        let count = p.u16()?;
        if count > MAX_INTERVALS {
            return Err(WireError::InvalidField);
        }
        let mut points_kw = alloc::vec::Vec::with_capacity(count as usize);
        for _ in 0..count {
            let v = p.f64()?;
            if v < 0.0 {
                return Err(WireError::InvalidField);
            }
            points_kw.push(v);
        }
        // 重建 curve canonical 并与 BlobRef 比对。
        let proposal = Self {
            proposal_id: pid,
            offer_id: oid,
            site_id,
            tolerance_profile_id,
            interval_start_ns,
            interval_ns,
            points_kw,
            reserve_up_kw,
            reserve_down_kw,
            target_curve_ref,
        };
        let curve = proposal.curve_canonical()?;
        if !proposal.target_curve_ref.matches(&curve) {
            return Err(WireError::DigestMismatch);
        }
        p.done()?;
        Ok(proposal)
    }
}

/// AllocationCommitment（蓝图 §2.2）：站点签署接受目标，协调器确认。
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationCommitment {
    pub commitment_id: [u8; 16],
    pub offer_id: [u8; 16],
    pub coordinator_id: u32,
    pub site_id: u32,
    pub tolerance_profile_id: u32,
    pub interval_start_ns: u64,
    pub interval_ns: u64,
    pub points_kw: alloc::vec::Vec<f64>,
    pub target_curve_ref: BlobRefV1,
    pub accepted_at_ns: u64,
    pub expires_at_ns: u64,
    pub revision: u64,
}

impl AllocationCommitment {
    pub fn curve_canonical(&self) -> Result<Vec<u8>, WireError> {
        let mut w = Writer::new();
        w.u64(self.interval_start_ns);
        w.u64(self.interval_ns);
        w.u16(self.points_kw.len() as u16);
        for v in &self.points_kw {
            w.f64(*v)?;
        }
        Ok(w.finish())
    }

    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.points_kw.len() as u16 > MAX_INTERVALS || self.interval_ns == 0 {
            return Err(WireError::InvalidField);
        }
        if self.expires_at_ns <= self.accepted_at_ns || self.revision == 0 {
            return Err(WireError::InvalidField);
        }
        let curve = self.curve_canonical()?;
        if !self.target_curve_ref.matches(&curve) {
            return Err(WireError::DigestMismatch);
        }
        let mut w = Writer::new();
        w.bytes(&self.commitment_id);
        w.bytes(&self.offer_id);
        w.u32(self.coordinator_id);
        w.u32(self.site_id);
        w.u32(self.tolerance_profile_id);
        self.target_curve_ref.encode(&mut w.0);
        w.u64(self.accepted_at_ns);
        w.u64(self.expires_at_ns);
        w.u64(self.revision);
        w.bytes(&curve);
        Ok(w.finish())
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let cid = p.bytes_16()?;
        let oid = p.bytes_16()?;
        let coordinator_id = p.u32()?;
        let site_id = p.u32()?;
        let tolerance_profile_id = p.u32()?;
        let target_curve_ref = BlobRefV1::decode(&mut p)?;
        let accepted_at_ns = p.u64()?;
        let expires_at_ns = p.u64()?;
        let revision = p.u64()?;
        let interval_start_ns = p.u64()?;
        let interval_ns = p.u64()?;
        if interval_ns == 0 {
            return Err(WireError::InvalidField);
        }
        let count = p.u16()?;
        if count > MAX_INTERVALS {
            return Err(WireError::InvalidField);
        }
        let mut points_kw = alloc::vec::Vec::with_capacity(count as usize);
        for _ in 0..count {
            let v = p.f64()?;
            if v < 0.0 {
                return Err(WireError::InvalidField);
            }
            points_kw.push(v);
        }
        let commitment = Self {
            commitment_id: cid,
            offer_id: oid,
            coordinator_id,
            site_id,
            tolerance_profile_id,
            interval_start_ns,
            interval_ns,
            points_kw,
            target_curve_ref,
            accepted_at_ns,
            expires_at_ns,
            revision,
        };
        let curve = commitment.curve_canonical()?;
        if !commitment.target_curve_ref.matches(&curve) {
            return Err(WireError::DigestMismatch);
        }
        p.done()?;
        Ok(commitment)
    }
}

/// CommitmentAck（协调器确认）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitmentAck {
    pub commitment_id: [u8; 16],
    pub acked_at_ns: u64,
}

impl CommitmentAck {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        let mut w = Writer::new();
        w.bytes(&self.commitment_id);
        w.u64(self.acked_at_ns);
        Ok(w.finish())
    }
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let cid = p.bytes_16()?;
        let acked_at_ns = p.u64()?;
        p.done()?;
        Ok(Self {
            commitment_id: cid,
            acked_at_ns,
        })
    }
}

/// ProgressEvidence：站内只上送聚合进度（蓝图 §2.3 第 5 步、§2.5 数据最小化）。
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressEvidence {
    pub commitment_id: [u8; 16],
    pub site_id: u32,
    pub interval_start_ns: u64,
    pub interval_end_ns: u64,
    pub delivered_kwh: f64,
    pub deviation_kw: f64,
    pub evidence_ref: BlobRefV1,
    /// 同一 commitment 内进度序号（单调递增，防重放）。
    pub seq: u64,
}

impl ProgressEvidence {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.interval_end_ns <= self.interval_start_ns {
            return Err(WireError::InvalidField);
        }
        let mut w = Writer::new();
        w.bytes(&self.commitment_id);
        w.u32(self.site_id);
        w.u64(self.interval_start_ns);
        w.u64(self.interval_end_ns);
        w.f64(self.delivered_kwh)?;
        w.f64(self.deviation_kw)?;
        self.evidence_ref.encode(&mut w.0);
        w.u64(self.seq);
        Ok(w.finish())
    }
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let cid = p.bytes_16()?;
        let site_id = p.u32()?;
        let interval_start_ns = p.u64()?;
        let interval_end_ns = p.u64()?;
        let delivered_kwh = p.f64()?;
        let deviation_kw = p.f64()?;
        let evidence_ref = BlobRefV1::decode(&mut p)?;
        let seq = p.u64()?;
        p.done()?;
        let ev = Self {
            commitment_id: cid,
            site_id,
            interval_start_ns,
            interval_end_ns,
            delivered_kwh,
            deviation_kw,
            evidence_ref,
            seq,
        };
        // decode/encode 同限：decode 放行 encode 拒的体会让 journal 重编码
        // 失败被静默吞（证据丢失），这里复用 encode 的业务校验。
        ev.encode()?;
        Ok(ev)
    }
}

/// SettlementEvidence：结算期聚合实测 + 质量（fail-closed 解码）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementEvidence {
    pub commitment_id: [u8; 16],
    pub site_id: u32,
    pub settle_start_ns: u64,
    pub settle_end_ns: u64,
    /// 实测电量（1000 分位 kWh，定点避免跨节点浮点差异）。
    pub measured_milli_kwh: u64,
    /// 质量万分比（0..=10000）。
    pub quality_bp: u16,
    pub note: alloc::vec::Vec<u8>,
}

impl SettlementEvidence {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.settle_end_ns <= self.settle_start_ns || self.quality_bp > 10000 {
            return Err(WireError::InvalidField);
        }
        if self.note.len() > 512 {
            return Err(WireError::Oversized);
        }
        let mut w = Writer::new();
        w.bytes(&self.commitment_id);
        w.u32(self.site_id);
        w.u64(self.settle_start_ns);
        w.u64(self.settle_end_ns);
        w.u64(self.measured_milli_kwh);
        w.u16(self.quality_bp);
        w.u16(self.note.len() as u16);
        w.bytes(&self.note);
        Ok(w.finish())
    }
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let cid = p.bytes_16()?;
        let site_id = p.u32()?;
        let settle_start_ns = p.u64()?;
        let settle_end_ns = p.u64()?;
        let measured_milli_kwh = p.u64()?;
        let quality_bp = p.u16()?;
        let note_len = p.u16()? as usize;
        let note = p.bytes(note_len)?.to_vec();
        p.done()?;
        // note 上限与 encode 同限（decode/encode 对称，journal 重编码不丢）。
        if note.len() > 512 {
            return Err(WireError::Oversized);
        }
        if settle_end_ns <= settle_start_ns || quality_bp > 10000 {
            return Err(WireError::InvalidField);
        }
        Ok(Self {
            commitment_id: cid,
            site_id,
            settle_start_ns,
            settle_end_ns,
            measured_milli_kwh,
            quality_bp,
            note,
        })
    }
}

/// OfferRevocation：能力变化时站点撤销/修订 offer（append-only）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferRevocation {
    pub offer_id: [u8; 16],
    pub reason: RejectReason,
    pub detail: alloc::vec::Vec<u8>,
}

impl OfferRevocation {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.detail.len() > 1024 {
            return Err(WireError::Oversized);
        }
        let mut w = Writer::new();
        w.bytes(&self.offer_id);
        w.u8(self.reason as u8);
        w.u16(self.detail.len() as u16);
        w.bytes(&self.detail);
        Ok(w.finish())
    }
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut p = Parser::new(body);
        let id = p.bytes_16()?;
        let reason = RejectReason::from_u8(p.u8()?)?;
        let len = p.u16()? as usize;
        let detail = p.bytes(len)?.to_vec();
        p.done()?;
        // detail 上限与 encode 同限（decode/encode 对称）。
        if detail.len() > 1024 {
            return Err(WireError::Oversized);
        }
        Ok(Self {
            offer_id: id,
            reason,
            detail,
        })
    }
}

/// typed body 解码入口（按 kind 分派）。
pub fn decode_body(kind: FedKind, body: &[u8]) -> Result<FedBody, WireError> {
    match kind {
        FedKind::Offer => Ok(FedBody::Offer(CapabilityOffer::decode(body)?)),
        FedKind::Reject => Ok(FedBody::Reject(OfferReject::decode(body)?)),
        FedKind::Proposal => Ok(FedBody::Proposal(AllocationProposal::decode(body)?)),
        FedKind::Commitment => Ok(FedBody::Commitment(AllocationCommitment::decode(body)?)),
        FedKind::Ack => Ok(FedBody::Ack(CommitmentAck::decode(body)?)),
        FedKind::Progress => Ok(FedBody::Progress(ProgressEvidence::decode(body)?)),
        FedKind::Settlement => Ok(FedBody::Settlement(SettlementEvidence::decode(body)?)),
        FedKind::Revoke => Ok(FedBody::Revoke(OfferRevocation::decode(body)?)),
    }
}

/// typed 消息体（对应 [`FedKind`] 全种类）。
#[derive(Clone, Debug, PartialEq)]
pub enum FedBody {
    Offer(CapabilityOffer),
    Reject(OfferReject),
    Proposal(AllocationProposal),
    Commitment(AllocationCommitment),
    Ack(CommitmentAck),
    Progress(ProgressEvidence),
    Settlement(SettlementEvidence),
    Revoke(OfferRevocation),
}

impl FedBody {
    /// 编码 body 字节。
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        match self {
            Self::Offer(o) => o.encode(),
            Self::Reject(r) => r.encode(),
            Self::Proposal(p) => p.encode(),
            Self::Commitment(c) => c.encode(),
            Self::Ack(a) => a.encode(),
            Self::Progress(p) => p.encode(),
            Self::Settlement(s) => s.encode(),
            Self::Revoke(r) => r.encode(),
        }
    }

    /// 对应 kind。
    pub fn kind(&self) -> FedKind {
        match self {
            Self::Offer(_) => FedKind::Offer,
            Self::Reject(_) => FedKind::Reject,
            Self::Proposal(_) => FedKind::Proposal,
            Self::Commitment(_) => FedKind::Commitment,
            Self::Ack(_) => FedKind::Ack,
            Self::Progress(_) => FedKind::Progress,
            Self::Settlement(_) => FedKind::Settlement,
            Self::Revoke(_) => FedKind::Revoke,
        }
    }
}

/// 便捷：由 typed body 构造已签信封。
pub fn sign_envelope(
    body: &FedBody,
    hdr: &EnvelopeHdr,
    kp: &Sm2KeyPair,
    rng: &mut CsRng,
) -> Result<FedEnvelope, WireError> {
    let bytes = body.encode()?;
    UnsignedEnvelope::from_body(body.kind(), hdr, &bytes).into_signed(kp, rng)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    fn header() -> EnvelopeHdr {
        EnvelopeHdr {
            epoch: 7,
            sequence: 42,
            tenant_id: 1,
            site_id: 9,
            created_ns: 1_000,
            deadline_ns: 2_000,
            signer_key_id: 3,
        }
    }

    fn kp() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let k = Sm2KeyPair::generate(&mut rng).expect("kp");
        (k, rng)
    }

    fn offer() -> CapabilityOffer {
        CapabilityOffer {
            offer_id: [1u8; 16],
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            p_min_kw: 0.0,
            p_max_kw: 100.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 100.0,
            ramp_kw_s: 10.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            price_curve_ref: BlobRefV1::new(1, b"price-curve"),
            capability_root: [7u8; 32],
            valid_until_ns: 9_999,
            revision: 1,
        }
    }

    #[test]
    fn offer_wire_roundtrip() {
        let o = offer();
        let bytes = o.encode().expect("encode");
        let dec = CapabilityOffer::decode(&bytes).expect("decode");
        assert_eq!(o, dec);
    }

    #[test]
    fn envelope_roundtrip_signature_ok() {
        let (k, mut rng) = kp();
        let body = FedBody::Offer(offer());
        let env = sign_envelope(&body, &header(), &k, &mut rng).expect("sign");
        let frame = FedFrame::encode(&env).expect("frame");
        let dec = FedFrame::decode(&frame).expect("decode");
        assert_eq!(env, dec);
        let ok = FedFrame::verify_signature(&dec, &k.public_key).expect("verify");
        assert!(ok, "valid signature must verify true");
    }

    #[test]
    fn tamper_rejected() {
        let (k, mut rng) = kp();
        let body = FedBody::Offer(offer());
        let env = sign_envelope(&body, &header(), &k, &mut rng).expect("sign");
        let frame = FedFrame::encode(&env).expect("frame");
        // 篡改 body（位于头部之后）→ payload digest 失败。
        let mut f_body = frame.clone();
        let body_off = 86;
        f_body[body_off] ^= 0x01;
        match FedFrame::decode(&f_body) {
            Err(WireError::DigestMismatch) => {}
            other => panic!("body tamper must fail digest check, got {other:?}"),
        }
        // 篡改信封头（deadline）→ 解码仍可（digest 只盖 body），但验签必假。
        let mut f_hdr = frame.clone();
        f_hdr[40] ^= 0x01;
        let dec = FedFrame::decode(&f_hdr).expect("header tamper decodes");
        let ok = FedFrame::verify_signature(&dec, &k.public_key).expect("verify");
        assert!(!ok, "header tamper must fail signature verification");
        // 篡改签名 → 验签必假。
        let mut f_sig = frame;
        let n = f_sig.len();
        f_sig[n - 1] ^= 0x80;
        let dec = FedFrame::decode(&f_sig).expect("sig tamper decodes");
        let ok = FedFrame::verify_signature(&dec, &k.public_key).expect("verify");
        assert!(!ok, "tampered signature must not verify");
    }

    #[test]
    fn non_finite_rejected() {
        let mut o = offer();
        o.p_max_kw = f64::NAN;
        assert_eq!(o.encode(), Err(WireError::NonFinite));
        o.p_max_kw = f64::INFINITY;
        assert_eq!(o.encode(), Err(WireError::NonFinite));
        // decode 侧同样拒绝
        let ok = offer().encode().expect("enc");
        let mut bad = ok.clone();
        // p_max 位于 offer: id(16)+start(8)+end(8)=32 → p_min(8)+p_max(8) 起点 40
        bad[48..56].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(CapabilityOffer::decode(&bad), Err(WireError::NonFinite));
    }

    #[test]
    fn truncated_and_oversized_rejected() {
        let full = offer().encode().expect("enc");
        for cut in [0usize, 10, full.len() - 1] {
            assert_eq!(
                CapabilityOffer::decode(&full[..cut]),
                Err(WireError::Truncated),
                "cut={cut}"
            );
        }
        let mut tail = full.clone();
        tail.push(0);
        assert_eq!(CapabilityOffer::decode(&tail), Err(WireError::Truncated));
    }

    #[test]
    fn bad_magic_version_kind_rejected() {
        let (k, mut rng) = kp();
        let env = sign_envelope(&FedBody::Offer(offer()), &header(), &k, &mut rng).expect("s");
        let frame = FedFrame::encode(&env).expect("f");
        let mut f1 = frame.clone();
        f1[0] = b'X';
        assert_eq!(FedFrame::decode(&f1), Err(WireError::BadMagic));
        let mut f2 = frame.clone();
        f2[4] = 99;
        assert_eq!(FedFrame::decode(&f2), Err(WireError::BadVersion));
        let mut f3 = frame.clone();
        f3[5] = 200;
        // kind 解析失败路径：200 未知 → BadKind
        assert_eq!(FedFrame::decode(&f3), Err(WireError::BadKind));
    }

    #[test]
    fn invalid_offer_fields_rejected() {
        let mut o = offer();
        o.revision = 0;
        assert_eq!(o.encode(), Err(WireError::InvalidField));
        let mut o2 = offer();
        o2.interval_end_ns = o2.interval_start_ns;
        assert_eq!(o2.encode(), Err(WireError::InvalidField));
        let mut o3 = offer();
        o3.p_max_kw = -1.0;
        assert_eq!(o3.encode(), Err(WireError::InvalidField));
    }

    #[test]
    fn progress_decode_validates_intervals() {
        // decode/encode 同限：倒挂区间的 progress 若 decode 放行，journal
        // 重编码会失败且被静默吞（证据丢失）。
        let mut w = Vec::new();
        w.extend_from_slice(&[1u8; 16]);
        w.extend_from_slice(&9u32.to_be_bytes());
        w.extend_from_slice(&2_000u64.to_be_bytes());
        w.extend_from_slice(&1_000u64.to_be_bytes()); // end < start
        w.extend_from_slice(&1.0f64.to_be_bytes());
        w.extend_from_slice(&0.0f64.to_be_bytes());
        w.push(3u8);
        w.extend_from_slice(&[9u8; 32]);
        w.extend_from_slice(&7u64.to_be_bytes());
        w.extend_from_slice(&1u64.to_be_bytes());
        assert_eq!(
            ProgressEvidence::decode(&w),
            Err(WireError::InvalidField),
            "decode 必须拒绝倒挂区间"
        );
        // 合法区间 roundtrip 不受影响。
        let ok = ProgressEvidence {
            commitment_id: [1; 16],
            site_id: 9,
            interval_start_ns: 1_000,
            interval_end_ns: 2_000,
            delivered_kwh: 1.0,
            deviation_kw: 0.0,
            evidence_ref: BlobRefV1::new(3, b"ev"),
            seq: 1,
        };
        let bytes = ok.encode().expect("enc");
        assert_eq!(ProgressEvidence::decode(&bytes), Ok(ok));
    }

    #[test]
    fn reject_and_settlement_decode_enforce_caps() {
        // OfferReject detail 与 Settlement note 的上限 decode/encode 对称。
        let mut w = Vec::new();
        w.extend_from_slice(&[1u8; 16]);
        w.push(RejectReason::Other as u8);
        w.extend_from_slice(&2000u16.to_be_bytes());
        w.extend_from_slice(&vec![0u8; 2000]);
        assert_eq!(OfferReject::decode(&w), Err(WireError::Oversized));

        let mut s = Vec::new();
        s.extend_from_slice(&[2u8; 16]);
        s.extend_from_slice(&9u32.to_be_bytes());
        s.extend_from_slice(&1_000u64.to_be_bytes());
        s.extend_from_slice(&2_000u64.to_be_bytes());
        s.extend_from_slice(&1_000u64.to_be_bytes());
        s.extend_from_slice(&10_000u16.to_be_bytes());
        s.extend_from_slice(&600u16.to_be_bytes());
        s.extend_from_slice(&vec![0u8; 600]); // > 512
        assert_eq!(SettlementEvidence::decode(&s), Err(WireError::Oversized));
    }

    #[test]
    fn data_minimized_wire_fields() {
        // 蓝图 §2.5：offer/progress/settlement 只携带聚合能力/引用，
        // 不携带原始点位。可实测的性质：证据与报价消息的 wire 大小
        // 与站内点位规模无关（O(1)），只受报价时域区间数影响。
        let (k, mut rng) = kp();
        let hdr = header();

        // 同一 progress：evidence 工件从 100 字节换成 100_000 字节，
        // wire 帧大小必须逐字节相同（BlobRef 是定长 41 字节引用）。
        let small_ref = BlobRefV1::new(3, &[0u8; 100]);
        let big_ref = BlobRefV1::new(3, &[9u8; 100_000]);
        let p_small = FedBody::Progress(ProgressEvidence {
            commitment_id: [2u8; 16],
            site_id: 9,
            interval_start_ns: 0,
            interval_end_ns: 1_000,
            delivered_kwh: 1.5,
            deviation_kw: 0.2,
            evidence_ref: small_ref,
            seq: 1,
        });
        let mut p_big_ev = match &p_small {
            FedBody::Progress(p) => p.clone(),
            _ => unreachable!(),
        };
        p_big_ev.evidence_ref = big_ref;
        let p_big = FedBody::Progress(p_big_ev);
        let e1 = sign_envelope(&p_small, &hdr, &k, &mut rng).expect("s");
        let e2 = sign_envelope(&p_big, &hdr, &k, &mut rng).expect("s");
        assert_eq!(
            FedFrame::encode(&e1).expect("f1").len(),
            FedFrame::encode(&e2).expect("f2").len(),
            "证据 wire 大小不得随站内点位/工件规模增长"
        );

        // offer：能力根是 32 字节 digest，而非点表内容。
        let o = FedBody::Offer(offer());
        let eo = sign_envelope(&o, &hdr, &k, &mut rng).expect("so");
        let frame = FedFrame::encode(&eo).expect("fo");
        // 常量信封开销（86B canonical 头 + 64B 签名）+ offer 定长体
        // （无点位字段；若有点位字段，长度将随点位规模变化）。
        assert_eq!(
            frame.len(),
            150 + offer().encode().expect("oe").len(),
            "offer 帧=常量信封 + 定长体"
        );
    }
}
