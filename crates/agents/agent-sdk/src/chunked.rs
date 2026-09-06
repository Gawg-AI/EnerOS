//! 蓝图 40 WP-40.8：EIPC 大载荷分块传输。
//!
//! seL4 ARM64 消息寄存器上限 120B（[`crate::ipc_sel4::MAX_MR_BYTES`]），
//! 而 ControlDispatch → device-agent 的 `ControlEnvelope` 为 270B（蓝图 40
//! WP-40.8 的 versioned wire envelope）。本模块定义确定性的三段协议：
//!
//! 1. **Header**（`Request` 消息）：`magic "ENV1" ‖ total_len u32 ‖
//!    chunk_count u8 ‖ chunk_size u16 ‖ epoch u64`（24B）——服务端确认
//!    容量并回 `ack`；
//! 2. **Data**（`Send` 消息，fire-and-forget）：`magic "ENVD" ‖ index u8
//!    ‖ data[..104]`——服务端按序组装（乱序/重复/越界 → Malformed）；
//! 3. **Final**（`Request` 消息）：`magic "ENVF"`——服务端处理完整载荷并
//!    以结构化响应回执（device-agent 返回 `OutcomeFrame`）。
//!
//! 任何一步超时/失败 → 明确错误，不猜成功（服务端未收到 Final 时丢弃
//! 半组装缓冲——重启/迟到消息由**传输代次 + 严格 index 序**拦截：代次由
//! Header 到达序定义、Data 帧不携带代次/epoch 字段；index=0 迟到帧的
//! 跨客户端窗口由服务端按 badge 分槽 + 双会话拒绝拦截，见
//! control-services `service_slots`。epoch 仅存在于 Header，用于会话
//! 身份，服务端不比较其值）。
//!
//! 纯编解码与状态机逻辑可在 host 单测；目标侧经
//! [`ChunkedClient`]（泛型 `RawIpc`）与 `Sel4IpcClient` 绑定。

use alloc::vec::Vec;

use crate::AgentError;

/// Header 消息 magic。
pub const CHUNK_MAGIC_HEADER: &[u8; 4] = b"ENV1";
/// Data 消息 magic。
pub const CHUNK_MAGIC_DATA: &[u8; 4] = b"ENVD";
/// Final 消息 magic。
pub const CHUNK_MAGIC_FINAL: &[u8; 4] = b"ENVF";
/// 服务端 ack（header 确认）。
pub const CHUNK_ACK: &[u8; 4] = b"ENVK";
/// 分页响应头 magic（Final 的响应；蓝图 40 收尾——响应方向分页）。
pub const CHUNK_MAGIC_PAGED: &[u8; 4] = b"ENVP";
/// 分页拉取请求 magic（`ENVN ‖ index u8`）。
pub const PULL_MAGIC: &[u8; 4] = b"ENVN";

/// 响应单页载荷上限（与请求方向 [`CHUNK_DATA_MAX`] 同预算）。
pub const RESP_PAGE_MAX: usize = 104;
/// 分页响应头定长：magic 4 + total u32 + chunk_count u8 + last_len u8。
pub const PAGED_HEADER_LEN: usize = 10;

/// MR 单帧回执 words 上限（69.1 I3 / REG-CHUNKED-MR-WORDS-CAP 单源：
/// `control_service_main` 组帧 break 与客户端 `request_raw` 组装 min 同引
/// 此常量；此前是 `14` 魔数两端手抄，超限即静默截断）。
/// **Z1 合规声明**：本常量与 [`MR_REPLY_BUDGET_BYTES`] 是 MR 传输预算
/// 常量，**不是** chunked 帧格式常量（magic/帧长/页大小/块数上限一律
/// 不动）。
pub const MR_REPLY_MAX_WORDS: usize = 14;
/// MR 单帧回执字节预算（= [`MR_REPLY_MAX_WORDS`] × word 宽 8B）。
pub const MR_REPLY_BUDGET_BYTES: usize = MR_REPLY_MAX_WORDS * 8;

// 69.1（I3）静态锁：回执预算与页机制的关系编译期固定。若未来新增超
// 预算回执形态，此处即编译失败——不得改为运行时检查后放行。（预算与
// 物理 MR 区"恰减 1 word"的关系锁在 [`crate::ipc_sel4`] 内——该模块
// cfg 门控，host 裸库构建不可达。）
const _: () = assert!(MR_REPLY_BUDGET_BYTES >= RESP_PAGE_MAX);
const _: () = assert!(MR_REPLY_BUDGET_BYTES % 8 == 0);
const _: () = assert!(MR_REPLY_BUDGET_BYTES / 8 == MR_REPLY_MAX_WORDS);

/// Header 消息定长。
pub const CHUNK_HEADER_LEN: usize = 24;
/// Data 消息定长（magic 4 + index 1 + 载荷 104）。
pub const CHUNK_FRAME_LEN: usize = 109;
/// 单块载荷上限（wire payload 预算 112B − 8B 头；保守取 104）。
pub const CHUNK_DATA_MAX: usize = 104;
/// 块数上限（270B 信封最多 3 块；留裕量防畸形声明）。
pub const CHUNK_COUNT_MAX: u8 = 16;

/// 响应方向页数上限（69.1 I4 / REG-CHUNKED-PAGE-U8-CAP：u8 页序域界）。
/// **域界 = 255 页（`u8::MAX`）× 104B = 26520B**——I4 的不变量是"`u8`
/// 表示静态证明不会截断"，非请求方向预算的美学对齐。卷册原拟 16 页
/// （1664B）被 VOL69.1-GATE 实弹证伪：svc-config 启动期分发 7099B 配置
/// 束（68 页）为既有生产面，16 页域检误拒 → 全窗 ACQ/决策链级联失败。
/// 请求方向 [`CHUNK_COUNT_MAX`]（16 块/1664B）不受本常量影响。
pub const RESP_COUNT_MAX: u8 = u8::MAX;

// 蓝图 61 [06] §4.2/§4.7：chunk 逻辑上限冻结 = 16 × 104 = 1664B——与
// telemetry-model::domain 的 `CHUNK_LOGICAL_CAP`（DomainEvent 最大帧
// 1158B ≤ 1664B 断言）互锁；任一侧容量漂移即编译失败。
const _: () = assert!((CHUNK_COUNT_MAX as usize) * CHUNK_DATA_MAX == 1664);
// 69.1（I4）静态锁：响应方向页数域界 = u8 全域（255 × 104 = 26520B）——
// 域检后 `u8` 表示数学上不可截断。
const _: () = assert!((RESP_COUNT_MAX as usize) * RESP_PAGE_MAX == 26520);

/// 分块协议错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkError {
    /// magic 不匹配。
    BadMagic,
    /// 长度不符。
    BadLength(usize),
    /// 块索引乱序/重复/越界。
    BadIndex { got: u8, expected: u8 },
    /// 载荷超预算。
    TooLarge { len: usize },
    /// 块数声明超上限。
    TooManyChunks(u8),
    /// total_len 与声明块数不符。
    LengthMismatch { total: u32, chunks: u8 },
    /// 半组装状态收到新 header（前一传输未完成）。
    Busy,
    /// 无传输进行中。
    NoTransfer,
    /// 目标不可达/超时（传输层）。
    Transport,
    /// 服务端拒绝（ack 不符）。
    ServerRejected,
}

/// Header 消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeader {
    /// 总载荷长度。
    pub total_len: u32,
    /// 块数。
    pub chunk_count: u8,
    /// 单块容量。
    pub chunk_size: u16,
    /// 发送方 epoch（重启计数；仅作会话身份——服务端不比较其值，迟到
    /// 帧拦截由传输代次 + 严格 index 序承担，见模块 doc）。
    pub epoch: u64,
}

impl ChunkHeader {
    /// 定长编码（24B）。
    pub fn encode(&self) -> [u8; CHUNK_HEADER_LEN] {
        let mut buf = [0u8; CHUNK_HEADER_LEN];
        buf[0..4].copy_from_slice(CHUNK_MAGIC_HEADER);
        buf[4..8].copy_from_slice(&self.total_len.to_le_bytes());
        buf[8] = self.chunk_count;
        buf[9..11].copy_from_slice(&self.chunk_size.to_le_bytes());
        buf[11..19].copy_from_slice(&self.epoch.to_le_bytes());
        buf
    }

    /// 解码。
    pub fn decode(buf: &[u8]) -> Result<Self, ChunkError> {
        if buf.len() != CHUNK_HEADER_LEN {
            return Err(ChunkError::BadLength(buf.len()));
        }
        if &buf[0..4] != CHUNK_MAGIC_HEADER {
            return Err(ChunkError::BadMagic);
        }
        let total_len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let chunk_count = buf[8];
        let chunk_size = u16::from_le_bytes([buf[9], buf[10]]);
        let epoch = u64::from_le_bytes(buf[11..19].try_into().expect("8B 定长切片"));
        if chunk_count > CHUNK_COUNT_MAX {
            return Err(ChunkError::TooManyChunks(chunk_count));
        }
        Ok(Self {
            total_len,
            chunk_count,
            chunk_size,
            epoch,
        })
    }
}

/// Data 消息帧（magic + index + data）。
pub fn encode_data(index: u8, data: &[u8]) -> Result<[u8; CHUNK_FRAME_LEN], ChunkError> {
    if data.len() > CHUNK_DATA_MAX {
        return Err(ChunkError::TooLarge { len: data.len() });
    }
    let mut buf = [0u8; CHUNK_FRAME_LEN];
    buf[0..4].copy_from_slice(CHUNK_MAGIC_DATA);
    buf[4] = index;
    buf[5..5 + data.len()].copy_from_slice(data);
    Ok(buf)
}

/// 解码 Data 消息（返回 index + 有效载荷切片）。
pub fn decode_data(buf: &[u8]) -> Result<(u8, &[u8]), ChunkError> {
    if buf.len() != CHUNK_FRAME_LEN {
        return Err(ChunkError::BadLength(buf.len()));
    }
    if &buf[0..4] != CHUNK_MAGIC_DATA {
        return Err(ChunkError::BadMagic);
    }
    let index = buf[4];
    let len = CHUNK_FRAME_LEN - 5;
    Ok((index, &buf[5..5 + len]))
}

/// 传输计划：按 CHUNK_DATA_MAX 切分载荷。
pub fn plan_chunks(payload_len: usize, epoch: u64) -> Result<(ChunkHeader, usize), ChunkError> {
    if payload_len == 0 {
        return Err(ChunkError::TooLarge { len: 0 });
    }
    let chunks = payload_len.div_ceil(CHUNK_DATA_MAX);
    if chunks > CHUNK_COUNT_MAX as usize {
        return Err(ChunkError::TooManyChunks(chunks as u8));
    }
    let header = ChunkHeader {
        total_len: payload_len as u32,
        chunk_count: chunks as u8,
        chunk_size: CHUNK_DATA_MAX as u16,
        epoch,
    };
    Ok((header, chunks))
}

/// 服务端接收事件（状态机输出）。
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkedEvent {
    /// 收到新传输 header（上一传输已被丢弃/完成）。
    Header { total_len: u32, chunk_count: u8 },
    /// 收到数据块（index）。
    Data { index: u8 },
    /// 组装完成（全部块就绪且长度一致）——载荷可处理。
    Complete(Vec<u8>),
    /// 协议违例（拒绝该传输）。
    Malformed(ChunkError),
}

/// 服务端分块接收状态机（无传输中状态 = Idle）。
#[derive(Debug, Clone)]
pub struct ChunkedReceiver {
    /// 期望的块数（None = Idle）。
    expected_chunks: Option<u8>,
    /// 期望的载荷长度。
    expected_len: u32,
    /// 已收到块数。
    received: u8,
    /// 组装缓冲。
    buf: Vec<u8>,
    /// 发送方 epoch（传输开始记录；仅作会话身份观测——见模块 doc）。
    epoch: u64,
    /// 传输代次（蓝图 68.1）：每收到 Header +1，由 **Header 到达序**定义
    /// （wire 不携带；Data 帧无代次字段）。"index 在当前代次内必须严格
    /// 连续从 0 起"的校验域即由此界定。
    generation: u64,
    /// 迟到/交错 Data 帧丢弃计数（index 违例即代次错位证据；累计不回零）。
    late_dropped: u32,
}

impl Default for ChunkedReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkedReceiver {
    /// 创建（Idle）。
    pub fn new() -> Self {
        Self {
            expected_chunks: None,
            expected_len: 0,
            received: 0,
            buf: Vec::new(),
            epoch: 0,
            generation: 0,
            late_dropped: 0,
        }
    }

    /// 是否空闲（无半组装传输）。
    pub fn idle(&self) -> bool {
        self.expected_chunks.is_none()
    }

    /// 当前传输代次（每 Header +1；观测面——服务端诊断与测试用）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 迟到/交错 Data 丢弃计数（累计；观测面——服务端诊断与测试用）。
    pub fn late_dropped(&self) -> u32 {
        self.late_dropped
    }

    /// 处理一条入站消息（header/data/final）。
    ///
    /// Final 前若块未齐 → `Malformed(NoTransfer/LengthMismatch)` 并复位；
    /// 完成后自动复位（下一次 header 从新传输开始）。
    pub fn on_message(&mut self, payload: &[u8], msg_is_request: bool) -> ChunkedEvent {
        if payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_HEADER {
            return self.on_header(payload);
        }
        if payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_DATA {
            return self.on_data(payload);
        }
        if payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_FINAL {
            return self.on_final(msg_is_request);
        }
        ChunkedEvent::Malformed(ChunkError::BadMagic)
    }

    fn on_header(&mut self, payload: &[u8]) -> ChunkedEvent {
        let header = match ChunkHeader::decode(payload) {
            Ok(h) => h,
            Err(e) => {
                self.reset();
                return ChunkedEvent::Malformed(e);
            }
        };
        // 进行中传输收到新 header：丢弃旧传输（明确协议：一客户端一传输）。
        // 代次 +1 = 旧传输全部在途帧从此与新传输的严格 index 序不相容
        // （除 index=0 的窗口帧——由服务端按 badge 分槽拦截，见模块 doc）。
        self.reset();
        self.generation = self.generation.wrapping_add(1);
        self.expected_chunks = Some(header.chunk_count);
        self.expected_len = header.total_len;
        self.epoch = header.epoch;
        self.buf.reserve(header.total_len as usize);
        ChunkedEvent::Header {
            total_len: header.total_len,
            chunk_count: header.chunk_count,
        }
    }

    fn on_data(&mut self, payload: &[u8]) -> ChunkedEvent {
        let Some(expected) = self.expected_chunks else {
            return ChunkedEvent::Malformed(ChunkError::NoTransfer);
        };
        let (index, data) = match decode_data(payload) {
            Ok(v) => v,
            Err(e) => {
                self.reset();
                return ChunkedEvent::Malformed(e);
            }
        };
        // 严格按序（乱序/重复/越界 → 拒绝整传输，fail-closed）。
        // index 违例 = 迟到/交错帧的直接证据 → 计数（蓝图 68.1 观测面）。
        if index != self.received {
            self.late_dropped = self.late_dropped.saturating_add(1);
            let e = ChunkError::BadIndex {
                got: index,
                expected: self.received,
            };
            self.reset();
            return ChunkedEvent::Malformed(e);
        }
        // 帧为定长零填充——按 total_len 裁剪实际载荷（末块短于块容量）。
        let expected_total = self.expected_len as usize;
        let remaining = expected_total.saturating_sub(self.received as usize * CHUNK_DATA_MAX);
        let actual = core::cmp::min(remaining, CHUNK_DATA_MAX);
        self.buf.extend_from_slice(&data[..actual]);
        self.received += 1;
        if self.received >= expected {
            // 块齐但长度不符 → 拒绝（不提前 Complete）。
            if self.buf.len() != self.expected_len as usize {
                let e = ChunkError::LengthMismatch {
                    total: self.expected_len,
                    chunks: self.received,
                };
                self.reset();
                return ChunkedEvent::Malformed(e);
            }
            let complete = core::mem::take(&mut self.buf);
            self.reset();
            return ChunkedEvent::Complete(complete);
        }
        ChunkedEvent::Data { index }
    }

    fn on_final(&mut self, _msg_is_request: bool) -> ChunkedEvent {
        // 69.1 注记（O2）：`SlotTable` 在 `final_frame` 分支提前拦截 Final
        // 帧（control-services `service_slots.rs` 的 final_frame 臂先于
        // rx.on_message 处理），故本函数在 SlotTable 消费路径下**不可达**；
        // 保留为状态机完备性面（host 侧其他 ChunkedReceiver 消费者可能
        // 经 on_message 走到此）。`_msg_is_request` 形参保留签名对称。
        // Final 语义：若块已齐（Complete 已发出），Final 只是取回执的信号；
        // 服务端在收到 Final 的请求上回复结果。此处状态机只负责组装：
        // 未齐就 Final → 拒绝。
        if self.expected_chunks.is_some() {
            let e = ChunkError::LengthMismatch {
                total: self.expected_len,
                chunks: self.received,
            };
            self.reset();
            return ChunkedEvent::Malformed(e);
        }
        ChunkedEvent::Malformed(ChunkError::NoTransfer)
    }

    fn reset(&mut self) {
        self.expected_chunks = None;
        self.expected_len = 0;
        self.received = 0;
        self.buf.clear();
    }
}

/// 客户端底层 IPC 原语（目标：`Sel4IpcClient`；host 测试：内存通道）。
pub trait RawIpc {
    /// 同步请求-响应（对应 `seL4_Call`）。
    fn request(
        &mut self,
        target: &str,
        payload: &[u8],
        timeout_ns: u64,
    ) -> Result<Vec<u8>, AgentError>;
    /// 单向发送（对应 `seL4_Send`）。
    fn send(&mut self, target: &str, payload: &[u8]) -> Result<(), AgentError>;
}

#[cfg(any(test, feature = "sel4"))]
impl RawIpc for crate::ipc_sel4::Sel4IpcClient {
    /// 裸帧请求（无 wire 封装）——目标服务循环（`control_service_main`）
    /// 按裸 MR 字节喂分块状态机；wire 封装（16B 头 + target + CRC）会
    /// 超 MR 112B 预算且两端 magic 不匹配（40 收尾对齐）。
    fn request(
        &mut self,
        _target: &str,
        payload: &[u8],
        timeout_ns: u64,
    ) -> Result<Vec<u8>, AgentError> {
        self.request_raw(payload, timeout_ns)
    }

    fn send(&mut self, _target: &str, payload: &[u8]) -> Result<(), AgentError> {
        self.send_raw(payload)
    }
}

/// `&mut T` 转发（`ChunkedClient` 以 `&mut self.client` 持有传输——
/// 每次 transfer 复用同一客户端实例，不 move 字段）。
/// SPEC-B（2026-08-28）：解除 sel4/test 门控——纯引用转发与 seL4 无关，
/// host 侧 RawIpc 泛型消费（endpoint/守卫测试假件）在任意 host 构建
/// 下亦须成立。
impl<T: RawIpc> RawIpc for &mut T {
    fn request(
        &mut self,
        target: &str,
        payload: &[u8],
        timeout_ns: u64,
    ) -> Result<Vec<u8>, AgentError> {
        (**self).request(target, payload, timeout_ns)
    }

    fn send(&mut self, target: &str, payload: &[u8]) -> Result<(), AgentError> {
        (**self).send(target, payload)
    }
}

/// 客户端分块传输（WP-40.8）。
///
/// 流程：Header(request) → ack；Data(send)×n；Final(request) → 服务端回执。
pub struct ChunkedClient<T: RawIpc> {
    ipc: T,
    /// 发送方 epoch（重启计数；新传输递增）。
    epoch: u64,
    /// 传输超时（ms）。
    timeout_ms: u64,
}

impl<T: RawIpc> ChunkedClient<T> {
    /// 创建。
    pub fn new(ipc: T, epoch: u64, timeout_ms: u64) -> Self {
        Self {
            ipc,
            epoch,
            timeout_ms,
        }
    }

    /// 传输完整载荷并取回执。
    ///
    /// `target` = 服务端点名（EIPC 目标）；`payload` = 待传载荷
    /// （如 ControlEnvelope 270B）；返回服务端完整结构化回执字节
    /// （经分页拉取组装——响应方向同样受 MR 112B 预算约束）。
    pub fn transfer(&mut self, target: &str, payload: &[u8]) -> Result<Vec<u8>, ChunkError> {
        if payload.is_empty() {
            return Err(ChunkError::TooLarge { len: 0 });
        }
        let (header, chunks) = plan_chunks(payload.len(), self.epoch)?;
        self.epoch = self.epoch.saturating_add(1);
        let timeout_ns = self.timeout_ms.saturating_mul(1_000_000);

        // 1. Header（request）→ ack（words 语义下 ack 为 8B 对齐，
        //    前 4B magic 判定）。
        let ack = self
            .ipc
            .request(target, &header.encode(), timeout_ns)
            .map_err(|_| ChunkError::Transport)?;
        if ack.len() < 4 || &ack[..4] != CHUNK_ACK {
            return Err(ChunkError::ServerRejected);
        }

        // 2. Data（send）× n。
        for i in 0..chunks {
            let start = i * CHUNK_DATA_MAX;
            let end = core::cmp::min(start + CHUNK_DATA_MAX, payload.len());
            let frame = encode_data(i as u8, &payload[start..end])?;
            self.ipc
                .send(target, &frame)
                .map_err(|_| ChunkError::Transport)?;
        }

        // 3. Final（request）→ 分页响应头（ENVP）。
        let head = self
            .ipc
            .request(target, CHUNK_MAGIC_FINAL, timeout_ns)
            .map_err(|_| ChunkError::Transport)?;
        let Some((total, count, last_len)) = decode_paged_header(&head) else {
            return Err(ChunkError::ServerRejected);
        };

        // 4. 分页拉取（每页 request → ≤104B 段；段长由头承载——
        //    MR 传输为 word 粒度补零，确切字节长度只在协议头里）。
        let mut out = Vec::with_capacity(total);
        for i in 0..count {
            let r = self
                .ipc
                .request(target, &pull_request(i), timeout_ns)
                .map_err(|_| ChunkError::Transport)?;
            let expected = if i as usize + 1 == count as usize {
                last_len
            } else {
                RESP_PAGE_MAX
            };
            // 69.1 注记（REG-CHUNKED-PULL-NO-DISCRIMINATOR，wire 冻结 Z1/Z11
            // 下不可完全修复）：Pull 回复与拒绝共用同一字节通道且**无判别
            // 位**——`r.len() < expected` 在 expected ≤ 16 时不可判别"ERR
            // 文本"与"末页载荷"（MR word 粒度零填充把 13B ERR 回执填成
            // 16B，service.rs 组帧 + ipc_sel4 组装两端同款）。服务端侧已
            // 消除覆盖性发散源（control-services 69.1 B-1：未拉完的分页
            // 响应拒绝被 Final 覆盖）；残余面（respawn/badge 复用后旧客户
            // 端按旧头续拉）登记为 owner 裁决项——wire 加判别前缀属 Z1
            // 帧格式修订，须 owner 裁决 + ADR。
            if r.len() < expected {
                return Err(ChunkError::ServerRejected);
            }
            out.extend_from_slice(&r[..expected]);
        }
        if out.len() != total {
            return Err(ChunkError::ServerRejected);
        }
        Ok(out)
    }
}

/// 编码分页响应头（服务端 Final 的响应；total → 页数 + 末页长度）。
pub fn encode_paged_header(total_len: usize) -> [u8; PAGED_HEADER_LEN] {
    let chunks = total_len.div_ceil(RESP_PAGE_MAX).max(1);
    let last = total_len - (chunks - 1) * RESP_PAGE_MAX;
    let mut buf = [0u8; PAGED_HEADER_LEN];
    buf[0..4].copy_from_slice(CHUNK_MAGIC_PAGED);
    buf[4..8].copy_from_slice(&(total_len as u32).to_le_bytes());
    buf[8] = chunks as u8;
    buf[9] = last as u8;
    buf
}

/// 解码分页响应头（客户端）。
pub fn decode_paged_header(buf: &[u8]) -> Option<(usize, u8, usize)> {
    if buf.len() < PAGED_HEADER_LEN || &buf[0..4] != CHUNK_MAGIC_PAGED {
        return None;
    }
    let total = u32::from_le_bytes(buf[4..8].try_into().ok()?) as usize;
    let count = buf[8];
    let last = buf[9] as usize;
    if count == 0 || last == 0 || last > RESP_PAGE_MAX {
        return None;
    }
    // total 与 count/last 必须自洽（畸形声明拒绝）。
    if total != (count as usize - 1) * RESP_PAGE_MAX + last {
        return None;
    }
    Some((total, count, last))
}

/// 分页拉取请求（`ENVN ‖ index`）。
pub fn pull_request(index: u8) -> [u8; 5] {
    let mut buf = [0u8; 5];
    buf[0..4].copy_from_slice(PULL_MAGIC);
    buf[4] = index;
    buf
}

/// 判断载荷是否为分页拉取请求（服务端在分块状态机前拦截）。
/// 接受 5B 或 8B（MR words 对齐补零——QEMU 实弹：1 word 传输）。
pub fn is_pull_request(payload: &[u8]) -> bool {
    payload.len() >= 5 && &payload[0..4] == PULL_MAGIC
}

/// 取响应第 `index` 页（服务端；越界返回空段——客户端段长校验拒绝）。
pub fn page_slice(resp: &[u8], index: u8) -> &[u8] {
    let start = index as usize * RESP_PAGE_MAX;
    if start >= resp.len() {
        return &[];
    }
    let end = core::cmp::min(start + RESP_PAGE_MAX, resp.len());
    &resp[start..end]
}

/// 服务端响应辅助：ack 字节。
pub fn ack_bytes() -> [u8; 4] {
    *CHUNK_ACK
}

/// Final 消息字节。
pub fn final_bytes() -> [u8; 4] {
    *CHUNK_MAGIC_FINAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = ChunkHeader {
            total_len: 270,
            chunk_count: 3,
            chunk_size: CHUNK_DATA_MAX as u16,
            epoch: 7,
        };
        let d = ChunkHeader::decode(&h.encode()).expect("decode");
        assert_eq!(d, h);
        assert!(matches!(
            ChunkHeader::decode(&[0u8; CHUNK_HEADER_LEN]),
            Err(ChunkError::BadMagic)
        ));
        assert!(matches!(
            ChunkHeader::decode(&[
                b'E', b'N', b'V', b'1', 0, 0, 0, 0, 99, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]),
            Err(ChunkError::TooManyChunks(99))
        ));
    }

    #[test]
    fn plan_and_data_frames() {
        let payload = alloc::vec![0xABu8; 270];
        let (h, n) = plan_chunks(payload.len(), 1).expect("plan");
        assert_eq!(h.total_len, 270);
        assert_eq!(n, 3);
        assert_eq!(h.chunk_count, 3);
        // 数据帧编码/解码。
        let f0 = encode_data(0, &payload[0..104]).expect("frame");
        let (idx, data) = decode_data(&f0).expect("decode");
        assert_eq!(idx, 0);
        assert_eq!(data, &payload[0..104]);
        // 超长拒绝。
        assert!(matches!(
            encode_data(0, &[0u8; 105]),
            Err(ChunkError::TooLarge { .. })
        ));
    }

    #[test]
    fn receiver_assembles_in_order() {
        let mut rx = ChunkedReceiver::new();
        let payload = alloc::vec![0x42u8; 270];
        let (h, n) = plan_chunks(payload.len(), 5).expect("plan");
        let ev = rx.on_message(&h.encode(), true);
        assert!(matches!(
            ev,
            ChunkedEvent::Header {
                total_len: 270,
                chunk_count: 3
            }
        ));
        for i in 0..n {
            let start = i * CHUNK_DATA_MAX;
            let end = core::cmp::min(start + CHUNK_DATA_MAX, payload.len());
            let f = encode_data(i as u8, &payload[start..end]).expect("frame");
            let ev = rx.on_message(&f, false);
            if i + 1 == n {
                assert!(matches!(ev, ChunkedEvent::Complete(v) if v == payload));
            } else {
                assert!(matches!(ev, ChunkedEvent::Data { index } if index as usize == i));
            }
        }
        assert!(rx.idle(), "完成后必须复位");
    }

    #[test]
    fn receiver_rejects_out_of_order() {
        let mut rx = ChunkedReceiver::new();
        let payload = alloc::vec![0x42u8; 200];
        let (h, _n) = plan_chunks(payload.len(), 1).expect("plan");
        let _ = rx.on_message(&h.encode(), true);
        // 直接发 index 1（跳 0）→ 拒绝并复位。
        let f1 = encode_data(1, &[0u8; 104]).expect("frame");
        let ev = rx.on_message(&f1, false);
        assert!(matches!(
            ev,
            ChunkedEvent::Malformed(ChunkError::BadIndex {
                got: 1,
                expected: 0
            })
        ));
        assert!(rx.idle(), "违例后必须复位（fail-closed）");
    }

    #[test]
    fn receiver_rejects_length_mismatch() {
        let mut rx = ChunkedReceiver::new();
        // 声明 270B/3 块但只发 2 块就 Final → Malformed。
        let payload = alloc::vec![0x42u8; 208];
        let (h, _) = plan_chunks(payload.len(), 1).expect("plan");
        let _ = rx.on_message(&h.encode(), true);
        let ev = rx.on_message(&final_bytes(), true);
        assert!(matches!(
            ev,
            ChunkedEvent::Malformed(ChunkError::LengthMismatch { .. })
        ));
        assert!(rx.idle());
    }

    #[test]
    fn data_without_header_rejected() {
        let mut rx = ChunkedReceiver::new();
        let f = encode_data(0, &[0u8; 10]).expect("frame");
        assert!(matches!(
            rx.on_message(&f, false),
            ChunkedEvent::Malformed(ChunkError::NoTransfer)
        ));
    }

    // ---- 蓝图 68 卷 68.1 WP-68.1.1：服务端交错红测群（接收器面） --------
    // t2/t6 按实现现状判定为**基线即绿**（严格 index 序已存在），本卷
    // 转行为锁 + 变异验证（见执行卡）；代次/迟到丢弃计数为本卷新增观测面。

    /// t2：新 Header 作废旧传输的全部在途 Data——旧传输数据不得进入
    /// 新传输（Header(B) 后到达的旧块 index 必然错位被拒）。
    #[test]
    fn t2_new_header_invalidates_stale_data() {
        let mut rx = ChunkedReceiver::new();
        let _ = rx.on_message(&plan_chunks(270, 1).expect("plan").0.encode(), true);
        let _ = rx.on_message(&encode_data(0, &[0xA1; 104]).expect("frame"), false);
        let _ = rx.on_message(&encode_data(1, &[0xA1; 104]).expect("frame"), false);
        // 新传输 B（1 块 50B）。
        let _ = rx.on_message(&plan_chunks(50, 2).expect("plan").0.encode(), true);
        assert_eq!(rx.generation(), 2, "两次 Header → 代次 2");
        // 旧传输在途块（index=2）迟到 → 拒绝 + 丢弃计数；fail-closed
        // 复位终止当前组装（B 的客户端按传输错误重试——旧数据无从进入）。
        let ev = rx.on_message(&encode_data(2, &[0xA1; 104]).expect("frame"), false);
        assert!(matches!(
            ev,
            ChunkedEvent::Malformed(ChunkError::BadIndex {
                got: 2,
                expected: 0
            })
        ));
        assert!(rx.idle(), "违例后复位（fail-closed）");
        assert_eq!(rx.late_dropped(), 1, "迟到帧必须被计数");
        // B 重试：全新 Header + 首块 → 完成载荷 = B 声明的 50B，无污染。
        let _ = rx.on_message(&plan_chunks(50, 3).expect("plan").0.encode(), true);
        let ev = rx.on_message(&encode_data(0, &[0xBB; 104]).expect("frame"), false);
        assert!(
            matches!(ev, ChunkedEvent::Complete(ref v) if v.len() == 50 && v.iter().all(|&b| b == 0xBB)),
            "B 载荷被旧传输数据污染: {ev:?}"
        );
    }

    /// t6：两传输 Data 交错投递——严格序校验下报明确错误词形
    /// （BadIndex），不得静默拼包。
    #[test]
    fn t6_interleaved_data_strict_index_no_silent_splice() {
        let mut rx = ChunkedReceiver::new();
        // A：2 块 150B。第 0 块到达。
        let _ = rx.on_message(&plan_chunks(150, 1).expect("plan").0.encode(), true);
        let _ = rx.on_message(&encode_data(0, &[0xA1; 104]).expect("frame"), false);
        // B 的 Header 插入（1 块 10B），随后 A 的第 1 块交错到达。
        let _ = rx.on_message(&plan_chunks(10, 2).expect("plan").0.encode(), true);
        let ev = rx.on_message(&encode_data(1, &[0xA1; 104]).expect("frame"), false);
        assert!(
            matches!(
                ev,
                ChunkedEvent::Malformed(ChunkError::BadIndex {
                    got: 1,
                    expected: 0
                })
            ),
            "交错块必须 BadIndex 词形: {ev:?}"
        );
        // fail-closed 复位后 B 重试全新传输——A 的块已被全部拒绝，无静默拼接。
        let _ = rx.on_message(&plan_chunks(10, 3).expect("plan").0.encode(), true);
        let ev = rx.on_message(&encode_data(0, &[0xBB; 104]).expect("frame"), false);
        assert!(
            matches!(ev, ChunkedEvent::Complete(ref v) if v.len() == 10 && v.iter().all(|&b| b == 0xBB)),
            "B 载荷被 A 的块拼接污染: {ev:?}"
        );
        assert_eq!(rx.late_dropped(), 1);
    }

    /// 代次观测面：每 Header +1（wire 不携带，到达序定义）；迟到帧计数
    /// 累计不回零（68.1 观测面定义锁）。
    #[test]
    fn generation_counts_headers_and_late_dropped_accumulates() {
        let mut rx = ChunkedReceiver::new();
        assert_eq!(rx.generation(), 0);
        let _ = rx.on_message(&plan_chunks(10, 1).expect("plan").0.encode(), true);
        assert_eq!(rx.generation(), 1);
        // index 违例（跳 0 发 1）→ 计数。
        let _ = rx.on_message(&encode_data(1, &[0u8; 104]).expect("frame"), false);
        assert_eq!(rx.late_dropped(), 1);
        // 新 Header（代次 +1）后计数不回零；新传输正常序不受既往影响。
        let _ = rx.on_message(&plan_chunks(10, 2).expect("plan").0.encode(), true);
        assert_eq!(rx.generation(), 2);
        assert_eq!(rx.late_dropped(), 1, "迟到丢弃计数累计不回零");
        let ev = rx.on_message(&encode_data(0, &[0xCC; 104]).expect("frame"), false);
        assert!(matches!(ev, ChunkedEvent::Complete(_)));
    }

    /// host 内存通道（模拟 RawIpc 对端；分页响应对齐 target 语义）。
    struct MemPeer {
        rx: ChunkedReceiver,
        last_outcome: Vec<u8>,
    }

    impl MemPeer {
        /// 服务端循环语义（与 control_service_main 一致）：
        /// Final → 缓存完整响应 + 回分页头；ENVN+index → 回对应页。
        fn handle_request(&mut self, payload: &[u8]) -> Result<Vec<u8>, AgentError> {
            if &payload[0..4] == CHUNK_MAGIC_HEADER {
                let _ = self.rx.on_message(payload, true);
                return Ok(ack_bytes().to_vec());
            }
            if &payload[0..4] == CHUNK_MAGIC_FINAL {
                return Ok(encode_paged_header(self.last_outcome.len()).to_vec());
            }
            if is_pull_request(payload) {
                // MR word 粒度：补零到 8B 对齐（客户端按协议头长度截断）。
                let mut padded = page_slice(&self.last_outcome, payload[4]).to_vec();
                let len = padded.len();
                padded.resize(len.div_ceil(8) * 8, 0);
                return Ok(padded);
            }
            Err(AgentError::IpcError(
                heapless::String::try_from("unexpected").expect("static 短串"),
            ))
        }
    }

    impl RawIpc for MemPeer {
        fn request(
            &mut self,
            _target: &str,
            payload: &[u8],
            _timeout_ns: u64,
        ) -> Result<Vec<u8>, AgentError> {
            self.handle_request(payload)
        }
        fn send(&mut self, _target: &str, payload: &[u8]) -> Result<(), AgentError> {
            let _ = self.rx.on_message(payload, false);
            Ok(())
        }
    }

    #[test]
    fn client_transfer_end_to_end() {
        let payload = alloc::vec![0x77u8; 270];
        let peer = MemPeer {
            rx: ChunkedReceiver::new(),
            last_outcome: vec![0, 0],
        };
        let mut client = ChunkedClient::new(peer, 0, 1000);
        let outcome = client
            .transfer("svc-device-agent", &payload)
            .expect("transfer");
        assert_eq!(outcome, vec![0, 0]);
    }

    #[test]
    fn client_transfer_paged_large_response() {
        // 145B 响应（SNAPSHOT 场景：1B 状态 + 144B trusted wire）——超单页
        // 104B 预算，必须经分页拉取组装，不截断。
        let mut resp = alloc::vec![0x30u8; 145];
        resp[0] = 0;
        resp[144] = 0xAB;
        let peer = MemPeer {
            rx: ChunkedReceiver::new(),
            last_outcome: resp.clone(),
        };
        let mut client = ChunkedClient::new(peer, 0, 1000);
        let outcome = client
            .transfer("svc-telemetry", &alloc::vec![2u8; 1])
            .expect("transfer");
        assert_eq!(outcome.len(), 145, "分页组装必须还原完整长度");
        assert_eq!(outcome, resp);
    }

    #[test]
    fn paged_header_roundtrip_and_validation() {
        let h = encode_paged_header(145);
        assert_eq!(decode_paged_header(&h), Some((145, 2, 41)));
        let h1 = encode_paged_header(2);
        assert_eq!(decode_paged_header(&h1), Some((2, 1, 2)));
        // 畸形：非 ENVP magic / 长度不符 / count=0。
        assert_eq!(decode_paged_header(&[0u8; 10]), None);
        assert_eq!(decode_paged_header(&h[..9]), None);
        let mut zero_count = h;
        zero_count[8] = 0;
        assert_eq!(decode_paged_header(&zero_count), None);
        // pull 请求判定 + 页切片。
        assert!(is_pull_request(&pull_request(1)));
        let resp: Vec<u8> = (0..200u8).collect();
        assert_eq!(page_slice(&resp, 0).len(), 104);
        assert_eq!(page_slice(&resp, 1).len(), 96);
        assert!(page_slice(&resp, 2).is_empty());
    }

    #[test]
    fn client_rejects_oversized_and_empty() {
        let peer = MemPeer {
            rx: ChunkedReceiver::new(),
            last_outcome: Vec::new(),
        };
        let mut client = ChunkedClient::new(peer, 0, 1000);
        assert!(matches!(
            client.transfer("t", &[]),
            Err(ChunkError::TooLarge { len: 0 })
        ));
        // 4096B 载荷（wire 上限）→ 40 块 > 16 上限 → 拒绝。
        let big = alloc::vec![0u8; 4096];
        assert!(matches!(
            client.transfer("t", &big),
            Err(ChunkError::TooManyChunks(40))
        ));
    }
}
