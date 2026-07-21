//! EnerOS v0.108.0 IEC 61850-9-2 SV 采样值接收协议栈（P2-G 第 4 版）.
//!
//! 在 v0.105.0 信息模型（DaValue）与 v0.107.0 GOOSE 事件通道基座上，
//! 实现 SV 二层采样值接收（EtherType 0x88BA）+ BER PDU 解码 + smpCnt 连续性检测，
//! 打通联邦保护协同的高速采样数据通道，为 v0.109.0 故障录波提供安全采样数据源。
//!
//! # 核心类型
//!
//! - [`sv_rx::SvSubscriber`] — SV 订阅者（泛型 L2 传输，receive / take_samples / set_callback）
//! - [`sv_rx::SvSample`] — 解码后的 SV 采样（smp_cnt + timestamp + channels + status）
//! - [`sv_rx::SampleStatus`] — 采样状态（New / Duplicate / SmpJump，D12）
//! - [`sv_buffer::RingBuffer`] — 固定容量环形缓冲（溢出覆盖最旧，D6）
//! - [`SvError`] — 错误枚举（TransportError / BerDecodeError / InvalidConfig / BufferOverflow，D10）
//! - [`L2Transport`] — 二层传输抽象 trait（send / recv，D4）
//! - [`MockL2`] — 脚本化 mock 传输（帧队列 + 发送记录 + 错误注入 + loopback，D4/D11）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/iec61850_sv/` → `crates/protocols/iec61850-sv/`（eneros-iec61850-sv）；蓝图 `crates/iec62351/` → `crates/security/iec62351/`（eneros-iec62351） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；SV 属 protocols，IEC 62351 属 security |
//! | **D2** | 蓝图 `docs/phase2/sv_security.md` → `docs/protocols/iec61850-sv-design.md` + `docs/protocols/iec62351-design.md` | 记忆 §2.3.3 强制：文档按方向分类；两个 crate 独立文档 |
//! | **D3** | 蓝图 `tests/sv_secure.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.107.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 删除蓝图 §4.5 `extern "C"` raw socket FFI + unsafe；SV 侧复用 GOOSE 的 `L2Transport` trait + `MockL2`（置于 lib.rs）；真实 raw socket 接线在集成层 | aarch64-unknown-none 无 libc 可链接 extern "C"；项目零 unsafe/零 C FFI 惯例；与 v0.107.0 D4 同先例 |
//! | **D5** | `SvSubscriber<T: L2Transport>` 泛型化，transport 由 `new` 注入（蓝图内部建 socket 写死） | 可测试性 + 网卡选择属集成层决策（Karpathy Simplicity First） |
//! | **D6** | 蓝图 §4.1 `RingBuffer { buf: Box<[T]> }` → `Vec<T>` 固定容量（heapless 风格）；`Box` 在 no_std 需全局分配器，Vec 更通用 | no_std 下 `Box<[T]>` 需 `alloc::boxed::Box` 且初始化冗长；`Vec::with_capacity` 更直观（v0.107.0 MockL2 用 Vec 先例） |
//! | **D7** | 蓝图 §4.5 `Sm4Cipher`/`Sm3Hmac` 自封装 FFI → 直接复用 eneros-crypto 的 `Sm4Gcm`/`Sm3Hmac`（纯 Rust，零 unsafe） | v0.31.0 已落地纯 Rust 实现；蓝图 FFI 代码在 aarch64-unknown-none 无法链接（无 libc）；避免重复造轮子（记忆 §5.5） |
//! | **D8** | 蓝图 §4.1 `SecureGoose` 单类型 → `SecureGoose` + `SecureSv` 同构双类型（内部均委托公共 `SecureChannel` 私有结构） | GOOSE 与 SV 语义独立（事件 vs 采样），调用方不应混用；公共逻辑抽取私有结构避免重复（Simplicity First） |
//! | **D9** | 蓝图 §4.1 `KeyMgmt.rotate_keys()` 内部生成密钥 → `rotate_keys(now, new_key_data, new_mac_key)` 由调用方注入密钥材料 | no_std 无系统熵源（CsRng 固定种子仅测试用）；生产环境密钥应由硬件 TRNG/密钥管理系统注入；与 v0.31.0 CaIssuer 外部注入 rng 先例一致 |
//! | **D10** | 错误模型统一：`SvError` = TransportError / BerDecodeError / InvalidConfig / BufferOverflow（4 变体）；`SecError` = KeyExpired / HmacMismatch / DecryptFailed / EncryptFailed / InvalidKeyId（5 变体） | 蓝图 SocketCreateFailed/SendFailed 随 FFI 删除合并为 TransportError；变体覆盖各失败面（对齐 v0.107.0 D10 精简风格） |
//! | **D11** | 性能 < 0.5ms（加密延迟）落地为 cfg(test) Instant 断言（MockL2 回路，加密+解密口径，文档声明）；§6.2 真实 GOOSE 端到端加密为实验室硬件项，以 mock 替代 | 无真实网卡硬件（与 v0.107.0 D11 同口径） |
//! | **D12** | 接收侧 smpCnt 跳变检测以 `SampleStatus`（New/Duplicate/SmpJump）随样本返回；蓝图 §4.4 要求检测跳变但 §4.2 `receive -> Result<(), SvError>` 无承载 → `SvSample.status: SampleStatus` 字段 + `receive -> Result<bool, SvError>` | 蓝图自相矛盾（要求检测但接口无处上报）；接收方必须能区分新采样/重复/丢样 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，第三方依赖仅 `eneros-iec61850-model`（path），
//! 零 unsafe，不调用 `panic!` / `todo!` / `unimplemented!`，
//! 可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod sv_buffer;
pub mod sv_rx;

pub use sv_buffer::RingBuffer;
pub use sv_rx::{SampleStatus, SvSample, SvSubscriber};

/// SV 协议错误（D10：统一错误模型，4 变体覆盖解码/传输/配置/缓冲）。
#[derive(Debug, Clone, PartialEq)]
pub enum SvError {
    /// 传输层错误（发送/接收失败）。
    TransportError,
    /// BER 解码失败（报文畸形/截断/未知长度格式）。
    BerDecodeError,
    /// 配置无效（如 app_id == 0）。
    InvalidConfig,
    /// 缓冲溢出（环形缓冲写满后仍写入）。
    BufferOverflow,
}

/// 二层传输抽象（D4：v0.27.0 网卡真实接线在集成层）。
pub trait L2Transport {
    /// 发送一帧二层报文。
    fn send(&mut self, frame: &[u8]) -> Result<(), SvError>;
    /// 接收一帧二层报文到 `buf`，返回实际字节数。
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, SvError>;
}

/// 脚本化 Mock 二层传输（测试/集成占位，D4/D11）。
///
/// - `send`：记录已发帧（供时序/内容断言）；loopback 模式时自动压入 rx 队列
/// - `recv`：依次弹出预置帧；无帧 → 返回 0（空接收）
/// - 可注入一次性 send/recv 错误
/// - 可启用 loopback：publish 的帧立即可被 poll 收到
#[derive(Debug, Clone, PartialEq)]
pub struct MockL2 {
    rx_frames: alloc::collections::VecDeque<alloc::vec::Vec<u8>>,
    tx_frames: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    inject_send_error: Option<SvError>,
    inject_recv_error: Option<SvError>,
    loopback: bool,
}

impl MockL2 {
    /// 创建空 mock（默认 loopback 关闭）。
    pub fn new() -> Self {
        Self {
            rx_frames: alloc::collections::VecDeque::new(),
            tx_frames: alloc::vec::Vec::new(),
            inject_send_error: None,
            inject_recv_error: None,
            loopback: false,
        }
    }

    /// 预置一段 recv 帧（按弹出顺序消费）。
    pub fn push_rx_frame(&mut self, bytes: &[u8]) {
        self.rx_frames.push_back(alloc::vec::Vec::from(bytes));
    }

    /// 已发送帧记录（供内容/时序断言）。
    pub fn tx_frames(&self) -> &[alloc::vec::Vec<u8>] {
        &self.tx_frames
    }

    /// 清空已发送记录。
    pub fn clear_tx(&mut self) {
        self.tx_frames.clear();
    }

    /// 注入一次性 send 错误。
    pub fn inject_send_error_once(&mut self, e: SvError) {
        self.inject_send_error = Some(e);
    }

    /// 注入一次性 recv 错误。
    pub fn inject_recv_error_once(&mut self, e: SvError) {
        self.inject_recv_error = Some(e);
    }

    /// 启用/禁用 loopback：send 的帧自动进入 rx 队列尾部。
    pub fn set_loopback(&mut self, enabled: bool) {
        self.loopback = enabled;
    }

    /// 查询 loopback 状态。
    pub fn loopback_enabled(&self) -> bool {
        self.loopback
    }
}

impl Default for MockL2 {
    fn default() -> Self {
        Self::new()
    }
}

impl L2Transport for MockL2 {
    fn send(&mut self, frame: &[u8]) -> Result<(), SvError> {
        if let Some(e) = self.inject_send_error.take() {
            return Err(e);
        }
        let cloned = alloc::vec::Vec::from(frame);
        if self.loopback {
            self.rx_frames.push_back(cloned.clone());
        }
        self.tx_frames.push(cloned);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, SvError> {
        if let Some(e) = self.inject_recv_error.take() {
            return Err(e);
        }
        let Some(frame) = self.rx_frames.pop_front() else {
            return Ok(0);
        };
        let n = frame.len().min(buf.len());
        buf[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_l2_send_records_tx() {
        let mut mock = MockL2::new();
        mock.send(&[0x01, 0x02]).unwrap();
        assert_eq!(mock.tx_frames().len(), 1);
        assert_eq!(mock.tx_frames()[0], alloc::vec![0x01, 0x02]);
    }

    #[test]
    fn mock_l2_recv_pops_preloaded_frame() {
        let mut mock = MockL2::new();
        mock.push_rx_frame(&[0xAA, 0xBB, 0xCC]);
        let mut buf = [0u8; 8];
        let n = mock.recv(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf[..n], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn mock_l2_recv_empty_returns_zero() {
        let mut mock = MockL2::new();
        let mut buf = [0u8; 4];
        let n = mock.recv(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn mock_l2_inject_send_error_once() {
        let mut mock = MockL2::new();
        mock.inject_send_error_once(SvError::TransportError);
        assert_eq!(mock.send(&[0x01]), Err(SvError::TransportError));
        // 第二次 send 应成功
        assert!(mock.send(&[0x01]).is_ok());
    }

    #[test]
    fn mock_l2_inject_recv_error_once() {
        let mut mock = MockL2::new();
        mock.inject_recv_error_once(SvError::TransportError);
        let mut buf = [0u8; 4];
        assert_eq!(mock.recv(&mut buf), Err(SvError::TransportError));
        // 第二次 recv 应成功（空队列返回 0）
        assert_eq!(mock.recv(&mut buf).unwrap(), 0);
    }

    #[test]
    fn mock_l2_loopback_delivers_sent_frame() {
        let mut mock = MockL2::new();
        mock.set_loopback(true);
        mock.send(&[0x11, 0x22]).unwrap();
        let mut buf = [0u8; 4];
        let n = mock.recv(&mut buf).unwrap();
        assert_eq!(n, 2);
        assert_eq!(&buf[..n], &[0x11, 0x22]);
    }

    #[test]
    fn mock_l2_clear_tx_empties_record() {
        let mut mock = MockL2::new();
        mock.send(&[0x01]).unwrap();
        assert_eq!(mock.tx_frames().len(), 1);
        mock.clear_tx();
        assert!(mock.tx_frames().is_empty());
    }
}
