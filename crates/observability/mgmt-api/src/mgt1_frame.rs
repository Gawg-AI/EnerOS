//! MGT1 管理帧 **文法单源**（蓝图 76 卷二 WP-76.2.5/6；体检 R17 候选 6）。
//!
//! 本模块只承载**布局知识**：哪一段在第几个字节、多长、小端、形状判别、
//! 纯借用访问器与构帧辅助。**不承载**验签（ECDSA P-256 + SHA-256 + 墙钟
//! ⇒ `eneros-business-services::mgmt_principal::verify_v2_tail`）、角色语
//! 义、RBAC 判据——那些是消费方的裁决，不是字节在哪的问题。
//!
//! 宿主选择理由（卷二 §1B 依赖图裁决）：文法的两端是 svc-mgmt bin
//! （business-services）与 root 探针（root-task），而 root-task **不依赖**
//! business-services ⇒ 唯一能同时供两家的位置是 no_std 叶 crate
//! `eneros-mgmt-api`（其现成定位就是「版本化管理 API 契约」，`Role::to_u8`
//! /`from_u8` 这些被 4B 角色字段消费的枚举已在其中）。零环、零重量。
//!
//! # wire（MGT1，逐字节冻结——收编前后字节零移动）
//!
//! ```text
//! [0..4]   "MGT1"        头魔数
//! [4]      0x00          保留字节（会话段自 offset 5 起）
//! ── v1 会话块 48B（v2 亦以它开头）
//! [5..21]    principal  16B  NUL 填充
//! [21..25]   roles       4B  mgmt-api Role::to_u8 码，余位补零
//! [25..29]   tenant      4B  u32 LE
//! [29..45]   channel    16B  NUL 填充
//! [45..53]   expiry      8B  u64 LE
//! ── 会话块之后：v1 形直接是请求尾；v2 形先插 token ‖ sig 字段
//! [53..117]  token 64B   = 魔数 8 ‖ principal 16 ‖ roles 4 ‖ tenant 4
//!                         ‖ channel 16 ‖ expiry 8 ‖ issued 8
//! [117..191] sig 字段 74B = u16 LE 实长 ‖ DER(≤72) ‖ 零填充
//! ── 请求尾（两形同构，仅起点不同）
//! request_id 8B u64 LE ‖ action_len 1B ‖ action ‖ arg
//! ```
//!
//! # 命名冲突（刻意）
//!
//! [`TOKEN_LEN`] 此处 = **64**（MGT1 v2 token）；crate 根的 `TOKEN_LEN`
//! （[`crate::TOKEN_LEN`]，113B SM2 签名 Token，见 [`crate::auth`]）是**另一
//! 个域**。本模块刻意**不**在 crate 根 glob re-export——MGT1 文法一律经
//! `eneros_mgmt_api::mgt1_frame::…` 模块路径消费，两者永不靠"哪个 TOKEN_LEN"
//! 猜。

use alloc::vec::Vec;

/// token 魔数（"MGTPTK1"）。
pub const TOKEN_MAGIC: [u8; 8] = *b"MGTPTK1\0";
/// token 编码长度（canonical 定长）。
pub const TOKEN_LEN: usize = 64;
/// 签名 wire 字段 = 2B 实长（LE）+ DER（ECDSA P-256 DER ≤ 72B——整数
/// 各带高位补零最多 33B）= 74B。
pub const SIG_FIELD_LEN: usize = 74;
/// 旧无签会话长度（v1；仅 station 公钥未部署的开发模式合法）。
pub const SESSION_V1_LEN: usize = 48;
/// v2 会话长度 = v1 ‖ token ‖ sig 字段。
pub const SESSION_V2_LEN: usize = SESSION_V1_LEN + TOKEN_LEN + SIG_FIELD_LEN;
/// MGT1 wire 头长度（magic "MGT1" 4B ‖ 1B）。
pub const MGT1_HEADER_LEN: usize = 5;
/// v1 最短请求：头 ‖ 会话 48 ‖ request_id 8 ‖ action_len 1。
pub const V1_MIN_REQ_LEN: usize = MGT1_HEADER_LEN + SESSION_V1_LEN + 8 + 1;
/// v2 最短请求：头 ‖ 会话 48 ‖ token 64 ‖ sig 74 ‖ request_id 8 ‖ action_len 1。
pub const V2_MIN_REQ_LEN: usize = MGT1_HEADER_LEN + SESSION_V2_LEN + 8 + 1;

/// MGT1 头魔数字节（wire 史实：版本位固定 0）。
pub const MGT1_MAGIC: [u8; 4] = *b"MGT1";
/// MGT1 头保留/版本字节。
pub const MGT1_VERSION_BYTE: u8 = 0x00;
/// 请求尾 request_id 字段宽度。
pub const REQUEST_ID_LEN: usize = 8;
/// 请求尾 action_len 字段宽度（1B ⇒ action 文本可达上限 255B）。
pub const ACTION_LEN_FIELD_LEN: usize = 1;
/// action_len 字段可表达的字节数上限（域检在调用方，此处只给宽度界）。
pub const ACTION_MAX: usize = u8::MAX as usize;

// ── v1 48B 会话块字段（偏移相对会话块首）────────────────────────────
/// 会话块 principal 段起点。
pub const SESSION_PRINCIPAL_AT: usize = 0;
/// 会话块 principal 段宽度。
pub const SESSION_PRINCIPAL_LEN: usize = 16;
/// 会话块 roles 段起点。
pub const SESSION_ROLES_AT: usize = SESSION_PRINCIPAL_AT + SESSION_PRINCIPAL_LEN;
/// 会话块 roles 段宽度（mgmt-api `Role::to_u8` 码 + 3B 补零）。
pub const SESSION_ROLES_LEN: usize = 4;
/// 会话块 tenant 段起点。
pub const SESSION_TENANT_AT: usize = SESSION_ROLES_AT + SESSION_ROLES_LEN;
/// 会话块 tenant 段宽度。
pub const SESSION_TENANT_LEN: usize = 4;
/// 会话块 channel 段起点。
pub const SESSION_CHANNEL_AT: usize = SESSION_TENANT_AT + SESSION_TENANT_LEN;
/// 会话块 channel 段宽度。
pub const SESSION_CHANNEL_LEN: usize = 16;
/// 会话块 expiry 段起点。
pub const SESSION_EXPIRY_AT: usize = SESSION_CHANNEL_AT + SESSION_CHANNEL_LEN;
/// 会话块 expiry 段宽度。
pub const SESSION_EXPIRY_LEN: usize = 8;

const _: () = assert!(
    SESSION_EXPIRY_AT + SESSION_EXPIRY_LEN == SESSION_V1_LEN,
    "v1 会话块字段宽度必须恰铺满 48B"
);

// ── v2 token 64B 内部字段（探针构建侧与 verify_v2_tail 解析侧共用）──
/// token 魔数段起点。
pub const TOKEN_MAGIC_AT: usize = 0;
/// token 魔数段宽度。
pub const TOKEN_MAGIC_LEN: usize = 8;
/// token principal 段起点。
pub const TOKEN_PRINCIPAL_AT: usize = TOKEN_MAGIC_AT + TOKEN_MAGIC_LEN;
/// token principal 段宽度（与 v1 会话块同宽——镜像史实，非设计）。
pub const TOKEN_PRINCIPAL_LEN: usize = SESSION_PRINCIPAL_LEN;
/// token roles 段起点。
pub const TOKEN_ROLES_AT: usize = TOKEN_PRINCIPAL_AT + TOKEN_PRINCIPAL_LEN;
/// token roles 段宽度。
pub const TOKEN_ROLES_LEN: usize = SESSION_ROLES_LEN;
/// token tenant 段起点。
pub const TOKEN_TENANT_AT: usize = TOKEN_ROLES_AT + TOKEN_ROLES_LEN;
/// token tenant 段宽度。
pub const TOKEN_TENANT_LEN: usize = SESSION_TENANT_LEN;
/// token channel 段起点。
pub const TOKEN_CHANNEL_AT: usize = TOKEN_TENANT_AT + TOKEN_TENANT_LEN;
/// token channel 段宽度。
pub const TOKEN_CHANNEL_LEN: usize = SESSION_CHANNEL_LEN;
/// token expiry 段起点。
pub const TOKEN_EXPIRY_AT: usize = TOKEN_CHANNEL_AT + TOKEN_CHANNEL_LEN;
/// token expiry 段宽度。
pub const TOKEN_EXPIRY_LEN: usize = SESSION_EXPIRY_LEN;
/// token issued 段起点（签发时刻——纯记账字段，时序判据见 verify 侧）。
pub const TOKEN_ISSUED_AT: usize = TOKEN_EXPIRY_AT + TOKEN_EXPIRY_LEN;
/// token issued 段宽度。
pub const TOKEN_ISSUED_LEN: usize = 8;

const _: () = assert!(
    TOKEN_ISSUED_AT + TOKEN_ISSUED_LEN == TOKEN_LEN,
    "token 字段宽度必须恰铺满 64B"
);

// ── v2 签名字段 74B 内部布局 ────────────────────────────────────────
/// 签名字段内「实长」前缀起点（相对签名字段首）。
pub const SIG_LEN_FIELD_AT: usize = 0;
/// 签名字段内「实长」前缀宽度（u16 LE）。
pub const SIG_LEN_FIELD_LEN: usize = 2;
/// 签名字段内 DER 起点。
pub const SIG_DER_AT: usize = SIG_LEN_FIELD_AT + SIG_LEN_FIELD_LEN;
/// DER 可达上限（74 − 2）。
pub const SIG_DER_MAX: usize = SIG_FIELD_LEN - SIG_DER_AT;

/// MGT1 请求会话形状（长度 + token 魔数位置判据——svc-mgmt v1/v2 判别
/// 单源）。三臂必须互斥完备：带 token 魔数却尾段不足的帧**不得**落回
/// v1 无签解析——70.5 复查 P0：station 部署面 62..199B 短尾帧曾三臂全不
/// 命中（非 v2 形状拒绝臂/完整 v2 验签臂/`ERR:proto` 短帧臂）直落 v1
/// 无签解析，伪造 48B 会话块里的 admin 角色直达 `rpc.handle`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionShape {
    /// 连 48B 会话块 ‖ request_id ‖ action_len 都不够（`ERR:proto` 面）。
    Short,
    /// 旧 48B 无签形（仅 station 公钥未部署的开发模式合法）。
    V1,
    /// 带 token 魔数但不足完整 v2 长度——协议违例（任何模式都拒绝，
    /// 不落回 v1）。
    V2Truncated,
    /// 完整 v2 形状（尾段之后至少含 request_id 8 ‖ action_len 1）。
    V2,
}

/// 会话形状分类（判别 = v1 会话块之后的 token 魔数位置，与变长 action
/// 无关；长度下界见 [`V1_MIN_REQ_LEN`]/[`V2_MIN_REQ_LEN`]）。
pub fn classify_session(req: &[u8]) -> SessionShape {
    if req.len() < V1_MIN_REQ_LEN {
        return SessionShape::Short;
    }
    let magic_at = MGT1_HEADER_LEN + SESSION_V1_LEN;
    if req[magic_at..magic_at + TOKEN_MAGIC.len()] != TOKEN_MAGIC {
        return SessionShape::V1;
    }
    if req.len() < V2_MIN_REQ_LEN {
        return SessionShape::V2Truncated;
    }
    SessionShape::V2
}

// ── 偏移访问器（消费端与探针端不再手拼算术）─────────────────────────
/// 会话块起点 = 头之后（`req[session_start()..]` 喂 [`v1_session`]）。
pub const fn session_start() -> usize {
    MGT1_HEADER_LEN
}

/// 会话块尾 = 请求段起点：v1 形此偏移是 request_id，v2 形是 token 段
/// （两形共用——48B 会话块是共同前缀，这正是"镜像约定"曾经断裂的位置）。
pub const fn session_tail_start() -> usize {
    MGT1_HEADER_LEN + SESSION_V1_LEN
}

/// v2 形 request_id 偏移（= 头 ‖ 会话 ‖ token ‖ sig 字段之后）。
///
/// 两种等价写法都在此处（`SESSION_V2_LEN` 含 48B 会话块——差一处即 48B 错
/// 位，正是本卷要消灭的那类手抄）：`头 ‖ SESSION_V2_LEN` == `会话尾 ‖ token
/// ‖ sig 字段`。
pub const fn v2_request_id_offset() -> usize {
    MGT1_HEADER_LEN + SESSION_V2_LEN
}

/// 两种等价写法必须重合（`SESSION_V2_LEN` 含 48B 会话块——差一处即 48B 错
/// 位，正是本卷要消灭的那类手抄）。
const _: () = assert!(
    MGT1_HEADER_LEN + SESSION_V2_LEN == session_tail_start() + TOKEN_LEN + SIG_FIELD_LEN,
    "v2 request_id 偏移的两种写法分叉"
);

/// request_id 字段偏移（按形状）。
///
/// `Short`/`V2Truncated` 在判别处即被调用方拒绝（`ERR:proto`），**没有合
/// 法消费面**——此处按"该帧最接近的解析形"返回，只为 match 穷尽，值永不
/// 被消费。
pub const fn request_id_offset(shape: SessionShape) -> usize {
    match shape {
        SessionShape::V1 | SessionShape::Short => session_tail_start(),
        SessionShape::V2 | SessionShape::V2Truncated => v2_request_id_offset(),
    }
}

/// action_len 字段偏移（按形状；判据同 [`request_id_offset`]）。
pub const fn action_len_offset(shape: SessionShape) -> usize {
    request_id_offset(shape) + REQUEST_ID_LEN
}

/// action 文本段起点（按形状；判据同 [`request_id_offset`]）。
pub const fn action_start_offset(shape: SessionShape) -> usize {
    action_len_offset(shape) + ACTION_LEN_FIELD_LEN
}

/// v2 尾段切片 = token 64B ‖ sig 字段 74B（[`crate`] 外 `verify_v2_tail`
/// 的入参形状）。长度不足 → None（截断 v2 形不落回 v1）。
pub fn v2_tail(req: &[u8]) -> Option<&[u8]> {
    let at = session_tail_start();
    req.get(at..at + TOKEN_LEN + SIG_FIELD_LEN)
}

/// 解 request_id（按形状）。
pub fn request_id(req: &[u8], shape: SessionShape) -> Option<u64> {
    let at = request_id_offset(shape);
    let raw: &[u8; REQUEST_ID_LEN] = req.get(at..at + REQUEST_ID_LEN)?.try_into().ok()?;
    Some(u64::from_le_bytes(*raw))
}

/// 解 action_len 字段（wire 上的 1B 文本长度）。
pub fn action_len(req: &[u8], shape: SessionShape) -> Option<usize> {
    Some(*req.get(action_len_offset(shape))? as usize)
}

/// 借出 action 文本段与 arg 段（零拷贝）。
///
/// 拒绝条件与收编前的消费端逐字相同：`action_start + action_len >
/// req.len()` ⇒ None（调用方把它映射成 `ERR:len` 响应——错误词形/状态映射是
/// 裁决，留在调用方，本模块只说"这段字节借不出来"）。
pub fn action_and_arg(req: &[u8], shape: SessionShape) -> Option<(&[u8], &[u8])> {
    let start = action_start_offset(shape);
    let end = start + action_len(req, shape)?;
    let action = req.get(start..end)?;
    Some((action, &req[end..]))
}

/// v1 48B 会话块解构视图（纯借用 + 整型解码，零 alloc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V1Session<'a> {
    /// principal 原始 16B（NUL 填充；文本化判据在调用方）。
    pub principal: &'a [u8; SESSION_PRINCIPAL_LEN],
    /// roles 原始 4B（mgmt-api 角色码 + 补零；兼容旧字符串面判据在调用方）。
    pub roles: &'a [u8; SESSION_ROLES_LEN],
    /// 租户作用域（u32 LE）。
    pub tenant: u32,
    /// 通道绑定原始 16B。
    pub channel: &'a [u8; SESSION_CHANNEL_LEN],
    /// 过期时刻（u64 LE；单位裁决在调用方）。
    pub expiry: u64,
}

/// 解 v1 会话块（入参 = **自会话块首起**的切片，即
/// `&req[session_start()..]`）。不足 48B → None。
pub fn v1_session(p: &[u8]) -> Option<V1Session<'_>> {
    Some(V1Session {
        principal: field(p, SESSION_PRINCIPAL_AT)?,
        roles: field(p, SESSION_ROLES_AT)?,
        tenant: u32::from_le_bytes(*field(p, SESSION_TENANT_AT)?),
        channel: field(p, SESSION_CHANNEL_AT)?,
        expiry: u64::from_le_bytes(*field(p, SESSION_EXPIRY_AT)?),
    })
}

/// token 64B 解构视图（纯借用 + 整型解码，零 alloc）。
///
/// `verify_v2_tail` 解析本视图（字段语义/时序判据在它那边），root 探针经
/// [`build_v2_frame`] 构造同一视图的逆运算——两端从此共用一份宽度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenView<'a> {
    /// 魔数段原始 8B（等值检 = 调用方的 Malformed 判据）。
    pub magic: &'a [u8; TOKEN_MAGIC_LEN],
    /// principal 原始 16B。
    pub principal: &'a [u8; TOKEN_PRINCIPAL_LEN],
    /// roles 原始 4B（逐字节角色码，0 = 终止）。
    pub roles: &'a [u8; TOKEN_ROLES_LEN],
    /// 租户作用域（u32 LE）。
    pub tenant: u32,
    /// 通道绑定原始 16B。
    pub channel: &'a [u8; TOKEN_CHANNEL_LEN],
    /// 过期时刻（u64 LE）。
    pub expiry: u64,
    /// 签发时刻（u64 LE）。
    pub issued: u64,
}

/// 解 token 段（要求切片恰 [`TOKEN_LEN`] 长——定长 wire 无余位语义）。
pub fn token_view(t: &[u8]) -> Option<TokenView<'_>> {
    if t.len() != TOKEN_LEN {
        return None;
    }
    Some(TokenView {
        magic: field(t, TOKEN_MAGIC_AT)?,
        principal: field(t, TOKEN_PRINCIPAL_AT)?,
        roles: field(t, TOKEN_ROLES_AT)?,
        tenant: u32::from_le_bytes(*field(t, TOKEN_TENANT_AT)?),
        channel: field(t, TOKEN_CHANNEL_AT)?,
        expiry: u64::from_le_bytes(*field(t, TOKEN_EXPIRY_AT)?),
        issued: u64::from_le_bytes(*field(t, TOKEN_ISSUED_AT)?),
    })
}

/// 定长字段借出（越界 → None）。
fn field<const N: usize>(p: &[u8], at: usize) -> Option<&[u8; N]> {
    p.get(at..at + N).and_then(|s| s.try_into().ok())
}

// ── 构帧辅助（alloc 可用；探针侧 build_frame 与 host 互锁测试共用）────

/// 构帧入参：会话/token 字段组（**数据**侧——文法只知宽度，不知取值；
/// 探针固定数据因此留在 root 侧）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFields<'a> {
    /// principal（16B，NUL 填充由调用方负责）。
    pub principal: &'a [u8; SESSION_PRINCIPAL_LEN],
    /// roles 码 4B。
    pub roles: &'a [u8; SESSION_ROLES_LEN],
    /// 租户 4B（u32 LE 字节）。
    pub tenant: &'a [u8; SESSION_TENANT_LEN],
    /// 通道绑定 16B。
    pub channel: &'a [u8; SESSION_CHANNEL_LEN],
    /// 过期时刻。
    pub expiry: u64,
    /// 签发时刻（只进 token 段——v1 会话块无此字段）。
    pub issued: u64,
}

/// canonical token 64B（[`SessionFields`] → 字节；[`token_view`] 的逆运算）。
pub fn build_token(f: &SessionFields<'_>) -> [u8; TOKEN_LEN] {
    let mut t = [0u8; TOKEN_LEN];
    t[TOKEN_MAGIC_AT..TOKEN_MAGIC_AT + TOKEN_MAGIC_LEN].copy_from_slice(&TOKEN_MAGIC);
    t[TOKEN_PRINCIPAL_AT..TOKEN_PRINCIPAL_AT + TOKEN_PRINCIPAL_LEN].copy_from_slice(f.principal);
    t[TOKEN_ROLES_AT..TOKEN_ROLES_AT + TOKEN_ROLES_LEN].copy_from_slice(f.roles);
    t[TOKEN_TENANT_AT..TOKEN_TENANT_AT + TOKEN_TENANT_LEN].copy_from_slice(f.tenant);
    t[TOKEN_CHANNEL_AT..TOKEN_CHANNEL_AT + TOKEN_CHANNEL_LEN].copy_from_slice(f.channel);
    t[TOKEN_EXPIRY_AT..TOKEN_EXPIRY_AT + TOKEN_EXPIRY_LEN].copy_from_slice(&f.expiry.to_le_bytes());
    t[TOKEN_ISSUED_AT..TOKEN_ISSUED_AT + TOKEN_ISSUED_LEN].copy_from_slice(&f.issued.to_le_bytes());
    t
}

/// 签名字段 74B（`u16 LE 实长 ‖ der ‖ 零填充`）。
///
/// `declared_len` 与 `der` 长度**刻意分离**：规范面两者相等；畸形签名夹具
/// （root 探针 `build_forged_frame`）要的就是"声明 70B 而字段内 72B 非零"
/// 这种不自洽形态，故不由 der 长度反推。越宽 → None。
pub fn build_sig_field(declared_len: u16, der: &[u8]) -> Option<[u8; SIG_FIELD_LEN]> {
    if der.len() > SIG_DER_MAX || declared_len as usize > der.len() {
        return None;
    }
    let mut f = [0u8; SIG_FIELD_LEN];
    f[SIG_LEN_FIELD_AT..SIG_LEN_FIELD_AT + SIG_LEN_FIELD_LEN]
        .copy_from_slice(&declared_len.to_le_bytes());
    f[SIG_DER_AT..SIG_DER_AT + der.len()].copy_from_slice(der);
    Some(f)
}

/// 请求尾三段（两形同构）。action 超过 1B 可表达宽度 → None。
fn push_request_tail(
    out: &mut Vec<u8>,
    request_id: u64,
    action: &[u8],
    arg: &[u8],
    shape: SessionShape,
) -> Option<()> {
    if action.len() > ACTION_MAX {
        return None;
    }
    debug_assert_eq!(
        out.len(),
        request_id_offset(shape),
        "构帧偏移与文法偏移分叉"
    );
    out.extend_from_slice(&request_id.to_le_bytes());
    out.push(action.len() as u8);
    out.extend_from_slice(action);
    out.extend_from_slice(arg);
    Some(())
}

/// 头 + v1 48B 会话块（两形共用前缀）。
fn push_header_and_session(out: &mut Vec<u8>, f: &SessionFields<'_>) {
    out.extend_from_slice(&MGT1_MAGIC);
    out.push(MGT1_VERSION_BYTE);
    out.extend_from_slice(f.principal);
    out.extend_from_slice(f.roles);
    out.extend_from_slice(f.tenant);
    out.extend_from_slice(f.channel);
    out.extend_from_slice(&f.expiry.to_le_bytes());
}

/// 构 v1 无签帧（头 ‖ 会话 48 ‖ request_id ‖ action_len ‖ action ‖ arg）。
pub fn build_v1_frame(
    f: &SessionFields<'_>,
    request_id: u64,
    action: &[u8],
    arg: &[u8],
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(V1_MIN_REQ_LEN + action.len() + arg.len());
    push_header_and_session(&mut out, f);
    push_request_tail(&mut out, request_id, action, arg, SessionShape::V1)?;
    Some(out)
}

/// 构 v2 有签帧（规范形：签名字段实长 = DER 实长）。
pub fn build_v2_frame(
    f: &SessionFields<'_>,
    sig_der: &[u8],
    request_id: u64,
    action: &[u8],
    arg: &[u8],
) -> Option<Vec<u8>> {
    build_v2_frame_with_sig_field(f, sig_der.len() as u16, sig_der, request_id, action, arg)
}

/// 构 v2 帧（签名字段逐字节自定义——畸形签名夹具用）。
pub fn build_v2_frame_with_sig_field(
    f: &SessionFields<'_>,
    declared_sig_len: u16,
    sig_bytes: &[u8],
    request_id: u64,
    action: &[u8],
    arg: &[u8],
) -> Option<Vec<u8>> {
    let sig_field = build_sig_field(declared_sig_len, sig_bytes)?;
    let mut out = Vec::with_capacity(V2_MIN_REQ_LEN + action.len() + arg.len());
    push_header_and_session(&mut out, f);
    out.extend_from_slice(&build_token(f));
    out.extend_from_slice(&sig_field);
    push_request_tail(&mut out, request_id, action, arg, SessionShape::V2)?;
    Some(out)
}

/// 构**截断** v2 帧（带 token 魔数却缺完整尾段——70.5 复查 P0 的 62..199B
/// 缝隙攻击帧文法）：头 ‖ 会话 48 ‖ 魔数 8 ‖ request_id ‖ action_len ‖
/// action。落 [`SessionShape::V2Truncated`]，任何模式下都不得回到 v1 解析。
pub fn build_truncated_v2_frame(
    f: &SessionFields<'_>,
    request_id: u64,
    action: &[u8],
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(V1_MIN_REQ_LEN + action.len());
    push_header_and_session(&mut out, f);
    out.extend_from_slice(&TOKEN_MAGIC);
    debug_assert_eq!(out.len(), session_tail_start() + TOKEN_MAGIC_LEN);
    out.extend_from_slice(&request_id.to_le_bytes());
    if action.len() > ACTION_MAX {
        return None;
    }
    out.push(action.len() as u8);
    out.extend_from_slice(action);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用固定字段（宽度 = wire；取值任意且与产品探针无关）。
    fn fields() -> SessionFields<'static> {
        SessionFields {
            principal: b"station-ops\0\0\0\0\0",
            roles: &[2, 0, 0, 0],
            tenant: &[1, 0, 0, 0],
            channel: b"probe\0\0\0\0\0\0\0\0\0\0\0",
            expiry: 4_102_444_800_000,
            issued: 1_700_000_000_000,
        }
    }

    /// 70.5 复查 P0 回归锁（自 business-services/mgmt_principal.rs 随文法
    /// 迁入，判定函数与断言逐字保留）：形状判别三臂互斥完备——带 token 魔
    /// 数却尾段不足（62..199B）的帧 = `V2Truncated`（修复前三臂全不命中直
    /// 落 v1 无签解析：伪造 48B 会话块 admin 角色 ‖ 8B 魔数 ‖ 1B action_len
    /// ‖ action = 75B 攻击帧即达 `rpc.handle`）；完整 v2/旧 v1/短帧各归其位。
    #[test]
    fn classify_session_covers_truncated_v2_gap() {
        let magic_at = MGT1_HEADER_LEN + SESSION_V1_LEN;
        // 攻击帧：头 5 ‖ 伪造会话 48（roles[0]=2=admin）‖ 魔数 8 ‖ action_len ‖ action。
        let mut forged = vec![0u8; magic_at];
        forged[..4].copy_from_slice(b"MGT1");
        forged[MGT1_HEADER_LEN + 16] = 2;
        forged.extend_from_slice(&TOKEN_MAGIC);
        forged.push(13);
        forged.extend_from_slice(b"history-query");
        assert_eq!(forged.len(), 75, "攻击帧长度落在 62..200 缝隙内");
        assert_eq!(classify_session(&forged), SessionShape::V2Truncated);
        // 缝隙全域：带魔数且 len ∈ [V1_MIN, V2_MIN) 一律截断 v2。
        for len in V1_MIN_REQ_LEN..V2_MIN_REQ_LEN {
            let mut req = vec![0u8; len];
            req[magic_at..magic_at + 8].copy_from_slice(&TOKEN_MAGIC);
            assert_eq!(
                classify_session(&req),
                SessionShape::V2Truncated,
                "len={len}"
            );
        }
        // 完整 v2（恰 200B 与更长）。
        let mut full = vec![0u8; V2_MIN_REQ_LEN];
        full[magic_at..magic_at + 8].copy_from_slice(&TOKEN_MAGIC);
        assert_eq!(classify_session(&full), SessionShape::V2);
        full.extend_from_slice(b"extra-action-bytes");
        assert_eq!(classify_session(&full), SessionShape::V2);
        // 旧 v1（无魔数，任意 ≥62B）与短帧（<62B）。
        let v1 = vec![0u8; V1_MIN_REQ_LEN];
        assert_eq!(classify_session(&v1), SessionShape::V1);
        let v1_long = vec![0u8; V2_MIN_REQ_LEN + 7];
        assert_eq!(classify_session(&v1_long), SessionShape::V1);
        assert_eq!(
            classify_session(&v1[..V1_MIN_REQ_LEN - 1]),
            SessionShape::Short
        );
        assert_eq!(classify_session(&[]), SessionShape::Short);
        // 长度常量锚（svc-mgmt 判别下界同源）。
        assert_eq!(V1_MIN_REQ_LEN, 62);
        assert_eq!(V2_MIN_REQ_LEN, 200);
    }

    /// 布局常量锚：token/sig 字段内部宽度与 wire 逐字节史实一致（探针构建
    /// 侧与 verify 解析侧共用这些值）。
    #[test]
    fn layout_widths_are_anchored_to_wire() {
        assert_eq!(SESSION_ROLES_AT, 16);
        assert_eq!(SESSION_TENANT_AT, 20);
        assert_eq!(SESSION_CHANNEL_AT, 24);
        assert_eq!(SESSION_EXPIRY_AT, 40);
        assert_eq!(TOKEN_PRINCIPAL_AT, 8);
        assert_eq!(TOKEN_ROLES_AT, 24);
        assert_eq!(TOKEN_TENANT_AT, 28);
        assert_eq!(TOKEN_CHANNEL_AT, 32);
        assert_eq!(TOKEN_EXPIRY_AT, 48);
        assert_eq!(TOKEN_ISSUED_AT, 56);
        assert_eq!(SIG_DER_AT, 2);
        assert_eq!(SIG_DER_MAX, 72);
        assert_eq!(session_tail_start(), 53);
        assert_eq!(v2_request_id_offset(), 191);
        assert_eq!(request_id_offset(SessionShape::V1), 53);
        assert_eq!(action_start_offset(SessionShape::V1), V1_MIN_REQ_LEN);
        assert_eq!(action_start_offset(SessionShape::V2), V2_MIN_REQ_LEN);
    }

    /// **host 互锁测试**（卷二 §7 步骤 5——本卷的核心牙齿）：构帧 → 形状判
    /// 别 → 访问器 → 逐字段往返相等。此前探针与解码器靠"约定的镜像"对齐，
    /// 一端改版另一端静默错位且 host 零牙齿。
    #[test]
    fn v1_frame_roundtrips_through_accessors() {
        let f = fields();
        let frame = build_v1_frame(&f, 10, b"config-get", b"control-write").expect("build v1");
        assert_eq!(frame[..4], MGT1_MAGIC);
        assert_eq!(frame[4], MGT1_VERSION_BYTE);
        assert_eq!(classify_session(&frame), SessionShape::V1);
        let sess = v1_session(&frame[session_start()..]).expect("v1 会话块");
        assert_eq!(sess.principal, f.principal);
        assert_eq!(sess.roles, f.roles);
        assert_eq!(sess.tenant, u32::from_le_bytes(*f.tenant));
        assert_eq!(sess.channel, f.channel);
        assert_eq!(sess.expiry, f.expiry);
        assert_eq!(request_id(&frame, SessionShape::V1), Some(10));
        assert_eq!(action_len(&frame, SessionShape::V1), Some(10));
        let (action, arg) = action_and_arg(&frame, SessionShape::V1).expect("尾段");
        assert_eq!(action, b"config-get");
        assert_eq!(arg, b"control-write");
    }

    /// v2 形往返（含 token 视图与签名字段），并锚死关键偏移的字面值——文
    /// 法改宽度必在此断（与迁移测试的 62/200 同纪律）。
    #[test]
    fn v2_frame_roundtrips_through_accessors() {
        let f = fields();
        let der = [0xA7u8; 70];
        let frame = build_v2_frame(&f, &der, 260, b"config-set", b"K").expect("build v2");
        // wire 字面锚（收编前后不变的那份真相）。
        assert_eq!(frame.len(), 211, "200B 最小帧 ‖ action 10B ‖ arg 1B");
        assert_eq!(&frame[53..61], b"MGTPTK1\0");
        assert_eq!(&frame[117..119], &70u16.to_le_bytes());
        assert_eq!(&frame[119..189], &[0xA7; 70]);
        assert_eq!(&frame[189..191], &[0, 0]);
        assert_eq!(classify_session(&frame), SessionShape::V2);
        let tail = v2_tail(&frame).expect("v2 尾段");
        assert_eq!(tail.len(), TOKEN_LEN + SIG_FIELD_LEN);
        let tok = token_view(&tail[..TOKEN_LEN]).expect("token");
        assert_eq!(tok.magic, &TOKEN_MAGIC);
        assert_eq!(tok.principal, f.principal);
        assert_eq!(tok.roles, f.roles);
        assert_eq!(tok.tenant, u32::from_le_bytes(*f.tenant));
        assert_eq!(tok.channel, f.channel);
        assert_eq!(tok.expiry, f.expiry);
        assert_eq!(tok.issued, f.issued);
        let sig_field = &tail[TOKEN_LEN..];
        assert_eq!(
            &sig_field[SIG_DER_AT..SIG_DER_AT + 70],
            der.as_slice(),
            "签名字段内 DER 段原位"
        );
        assert!(
            sig_field[SIG_DER_AT + 70..].iter().all(|b| *b == 0),
            "余位零填充"
        );
        assert_eq!(request_id(&frame, SessionShape::V2), Some(260));
        let (action, arg) = action_and_arg(&frame, SessionShape::V2).expect("尾段");
        assert_eq!(action, b"config-set");
        assert_eq!(arg, b"K");
        // 构帧与访问器同源于一份宽度：token 视图 == build_token 逆运算。
        assert_eq!(token_view(&build_token(&f)).expect("reparse"), tok);
    }

    /// 截断 v2（短尾）三态之一：带魔数却缺签名段 ⇒ `V2Truncated`，且尾段
    /// 访问器一律拒（不落回 v1 的解析面）。
    #[test]
    fn truncated_v2_frame_is_rejected_not_fallen_back() {
        let f = fields();
        let frame = build_truncated_v2_frame(&f, 0xBEEF, b"reboot").expect("build 短尾");
        assert_eq!(frame.len(), 76);
        assert_eq!(&frame[53..61], b"MGTPTK1\0");
        assert_eq!(classify_session(&frame), SessionShape::V2Truncated);
        assert!(v2_tail(&frame).is_none(), "短尾帧不得借出完整 v2 尾段");
        assert!(action_and_arg(&frame, SessionShape::V2).is_none());
        // 缝隙下段（62..191B）连完整 token ‖ sig 尾段都借不出；191..200 虽
        // 含完整尾段但缺 request_id/action_len——由 classify 判 V2Truncated
        // （见迁移测试的全域扫描），故此处只扫到尾段可借出之前。
        for len in V1_MIN_REQ_LEN..session_tail_start() + TOKEN_LEN + SIG_FIELD_LEN {
            assert!(v2_tail(&vec![0u8; len]).is_none(), "len={len}");
        }
    }

    /// 签名字段自洽判据：`declared_len > der.len()` 或 DER 超 72B → None
    /// （畸形夹具要的是"声明与实写不等"，那由两个参数各自表达）。
    #[test]
    fn sig_field_builder_is_width_checked() {
        assert!(build_sig_field(8, b"12345678").is_some());
        assert!(
            build_sig_field(9, b"12345678").is_none(),
            "声明超实写 = 越宽"
        );
        assert!(build_sig_field(72, &[9u8; 72]).is_some());
        assert!(build_sig_field(72, &[9u8; 73]).is_none(), "DER 超 72B");
        // root 探针伪造帧形态：声明 70、字段内 72B 非零（零填充检必拒）。
        let forged = build_sig_field(70, &[0xDE; 72]).expect("伪造签名字段");
        assert_eq!(&forged[..2], &70u16.to_le_bytes());
        assert!(forged[2..].iter().all(|b| *b == 0xDE));
    }

    /// action 超出 1B 可表达宽度 = 文法违例（拒构，不静默截断）。
    #[test]
    fn overlong_action_is_refused_by_builders() {
        let f = fields();
        let long = vec![b'x'; 256];
        assert!(build_v1_frame(&f, 1, &long, &[]).is_none());
        assert!(build_v2_frame(&f, b"", 1, &long, &[]).is_none());
        assert!(build_truncated_v2_frame(&f, 1, &long).is_none());
        assert!(build_v2_frame(&f, b"", 1, &long[..255], &[]).is_some());
    }
}
