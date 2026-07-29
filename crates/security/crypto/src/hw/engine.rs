//! 硬件加密引擎抽象层 (v0.158.0).
//!
//! 定义统一的 `CryptoEngine` trait，屏蔽不同硬件平台（飞腾 D2000、鲲鹏 920 KAE）
//! 的寄存器差异，并提供 `CryptoSelector` 实现硬件优先 + 软件 fallback 策略。
//!
//! # 设计原则
//! - 硬件优先：启动时探测硬件引擎，可用则使用硬件加速
//! - 自动降级：硬件故障/超时自动 fallback 到纯 Rust 软件实现
//! - 零拷贝：大数据通过 DMA 传输，避免 CPU 拷贝开销
//! - 可扩展：trait 抽象支持未来新硬件引擎接入
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::error::CryptoError;

// ============================================================
// Algorithm & Mode Definitions
// ============================================================

/// 国密算法类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// SM2 椭圆曲线公钥算法
    Sm2,
    /// SM3 密码杂凑算法
    Sm3,
    /// SM4 分组密码算法
    Sm4,
}

/// SM4 工作模式.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Sm4Mode {
    /// ECB 模式（电子密码本）
    Ecb = 0,
    /// CBC 模式（密码分组链接）
    Cbc = 1,
    /// CTR 模式（计数器）
    Ctr = 2,
    /// GCM 模式（伽罗瓦/计数器）
    Gcm = 3,
}

// ============================================================
// Engine Statistics
// ============================================================

/// 引擎性能统计.
#[derive(Debug, Clone, Default)]
pub struct EngineStats {
    /// 总操作次数
    pub total_ops: u64,
    /// 成功操作次数
    pub success_ops: u64,
    /// 失败操作次数
    pub failed_ops: u64,
    /// Fallback 到软件的次数
    pub fallback_count: u64,
    /// 累计处理字节数
    pub bytes_processed: u64,
}

impl EngineStats {
    /// 创建空统计.
    pub const fn new() -> Self {
        Self {
            total_ops: 0,
            success_ops: 0,
            failed_ops: 0,
            fallback_count: 0,
            bytes_processed: 0,
        }
    }

    /// 记录一次成功操作.
    pub fn record_success(&mut self, bytes: usize) {
        self.total_ops += 1;
        self.success_ops += 1;
        self.bytes_processed += bytes as u64;
    }

    /// 记录一次失败操作.
    pub fn record_failure(&mut self) {
        self.total_ops += 1;
        self.failed_ops += 1;
    }

    /// 记录一次 fallback.
    pub fn record_fallback(&mut self) {
        self.fallback_count += 1;
    }

    /// 成功率（百分比，0~100）.
    pub fn success_rate(&self) -> u8 {
        if self.total_ops == 0 {
            return 100;
        }
        ((self.success_ops * 100) / self.total_ops) as u8
    }
}

// ============================================================
// CryptoEngine Trait
// ============================================================

/// 硬件加密引擎统一 trait.
///
/// 所有加密引擎（硬件/软件）必须实现此 trait。
/// 硬件引擎通过 MMIO 寄存器操作实现加速，软件引擎使用纯 Rust 算法。
///
/// # 安全要求
/// - 密钥在引擎内部处理，不应在内存中残留
/// - 操作完成后应清零临时缓冲区
pub trait CryptoEngine {
    /// SM2 签名（硬件加速）.
    ///
    /// # 参数
    /// - `private_key`: 32 字节 SM2 私钥（大端）
    /// - `msg`: 待签名消息
    ///
    /// # 返回
    /// 64 字节签名 (r ‖ s)，各 32 字节大端
    fn sm2_sign(&mut self, private_key: &[u8], msg: &[u8]) -> Result<[u8; 64], CryptoError>;

    /// SM2 验签.
    ///
    /// # 参数
    /// - `public_key`: 64 字节 SM2 公钥 (x ‖ y)，大端
    /// - `msg`: 原始消息
    /// - `sig`: 64 字节签名 (r ‖ s)
    ///
    /// # 返回
    /// `true` 表示签名有效
    fn sm2_verify(
        &mut self,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<bool, CryptoError>;

    /// SM3 哈希.
    ///
    /// # 参数
    /// - `data`: 任意长度输入数据
    ///
    /// # 返回
    /// 32 字节（256 位）杂凑值
    fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError>;

    /// SM4 对称加密.
    ///
    /// # 参数
    /// - `key`: 16 字节密钥
    /// - `iv`: 可选 IV（CBC/CTR/GCM 模式需要）
    /// - `mode`: SM4 工作模式
    /// - `data`: 明文数据
    ///
    /// # 返回
    /// 密文（长度取决于模式和填充）
    fn sm4_encrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError>;

    /// SM4 对称解密.
    ///
    /// # 参数
    /// - `key`: 16 字节密钥
    /// - `iv`: 可选 IV（CBC/CTR/GCM 模式需要）
    /// - `mode`: SM4 工作模式
    /// - `data`: 密文数据
    ///
    /// # 返回
    /// 明文
    fn sm4_decrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError>;

    /// 引擎名称.
    fn name(&self) -> &'static str;

    /// 是否为硬件加速引擎.
    fn is_hardware(&self) -> bool;

    /// 引擎是否可用（硬件探测结果）.
    ///
    /// 默认返回 `true`（软件引擎始终可用）。
    /// 硬件引擎应重写此方法以检测实际硬件存在。
    fn is_available(&self) -> bool {
        true
    }

    /// 获取引擎统计信息.
    ///
    /// 默认返回空统计。硬件引擎应重写以提供实际统计。
    fn stats(&self) -> EngineStats {
        EngineStats::new()
    }
}

// ============================================================
// CryptoSelector
// ============================================================

/// 加密引擎选择器.
///
/// 管理多个加密引擎，按优先级选择：硬件引擎优先，软件引擎作为 fallback。
/// 当硬件引擎操作失败时，自动降级到软件实现。
///
/// # 示例
/// ```ignore
/// use eneros_crypto::hw::{CryptoSelector, SoftwareCryptoEngine};
///
/// let mut selector = CryptoSelector::new();
/// selector.register(Box::new(SoftwareCryptoEngine::new()));
/// let hash = selector.sm3_hash(b"hello").unwrap();
/// ```
pub struct CryptoSelector {
    /// 已注册的引擎列表（按优先级排序，索引 0 最高）
    engines: Vec<Box<dyn CryptoEngine>>,
    /// 当前活跃引擎索引
    active_idx: usize,
    /// 选择器级别统计
    stats: EngineStats,
}

impl CryptoSelector {
    /// 创建空选择器.
    pub fn new() -> Self {
        Self {
            engines: Vec::new(),
            active_idx: 0,
            stats: EngineStats::new(),
        }
    }

    /// 注册加密引擎.
    ///
    /// 硬件引擎应优先注册（先注册 = 高优先级）。
    /// 软件 fallback 引擎应最后注册。
    pub fn register(&mut self, engine: Box<dyn CryptoEngine>) {
        self.engines.push(engine);
    }

    /// 获取当前活跃引擎名称.
    pub fn active_engine_name(&self) -> &'static str {
        if let Some(engine) = self.engines.get(self.active_idx) {
            engine.name()
        } else {
            "none"
        }
    }

    /// 获取当前活跃引擎是否为硬件.
    pub fn is_active_hardware(&self) -> bool {
        self.engines
            .get(self.active_idx)
            .map(|e| e.is_hardware())
            .unwrap_or(false)
    }

    /// 已注册引擎数量.
    pub fn engine_count(&self) -> usize {
        self.engines.len()
    }

    /// 选择器统计.
    pub fn stats(&self) -> &EngineStats {
        &self.stats
    }

    /// 列出所有已注册引擎信息.
    pub fn list_engines(&self) -> Vec<(&'static str, bool, bool)> {
        self.engines
            .iter()
            .map(|e| (e.name(), e.is_hardware(), e.is_available()))
            .collect()
    }

    // ── 内部：带 fallback 的操作执行 ──

    /// 执行 SM3 哈希（硬件优先 + 自动 fallback）.
    pub fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError> {
        self.exec_with_fallback(|engine| engine.sm3_hash(data))
    }

    /// 执行 SM2 签名（硬件优先 + 自动 fallback）.
    pub fn sm2_sign(&mut self, private_key: &[u8], msg: &[u8]) -> Result<[u8; 64], CryptoError> {
        self.exec_with_fallback(|engine| engine.sm2_sign(private_key, msg))
    }

    /// 执行 SM2 验签（硬件优先 + 自动 fallback）.
    pub fn sm2_verify(
        &mut self,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<bool, CryptoError> {
        self.exec_with_fallback(|engine| engine.sm2_verify(public_key, msg, sig))
    }

    /// 执行 SM4 加密（硬件优先 + 自动 fallback）.
    pub fn sm4_encrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.exec_with_fallback(|engine| engine.sm4_encrypt(key, iv, mode, data))
    }

    /// 执行 SM4 解密（硬件优先 + 自动 fallback）.
    pub fn sm4_decrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.exec_with_fallback(|engine| engine.sm4_decrypt(key, iv, mode, data))
    }

    /// 带 fallback 的通用执行逻辑.
    ///
    /// 从当前活跃引擎开始尝试，若遇到硬件错误则依次降级到下一个引擎。
    fn exec_with_fallback<T, F>(&mut self, op: F) -> Result<T, CryptoError>
    where
        F: Fn(&mut dyn CryptoEngine) -> Result<T, CryptoError>,
    {
        if self.engines.is_empty() {
            return Err(CryptoError::EngineUnavailable);
        }

        let start_idx = self.active_idx;
        let count = self.engines.len();

        for offset in 0..count {
            let idx = (start_idx + offset) % count;

            // 跳过不可用的引擎
            if !self.engines[idx].is_available() {
                continue;
            }

            match op(self.engines[idx].as_mut()) {
                Ok(result) => {
                    self.active_idx = idx;
                    self.stats.record_success(0);
                    return Ok(result);
                }
                Err(e) if e.is_hardware_error() => {
                    // 硬件错误：记录并尝试下一个引擎
                    self.stats.record_failure();
                    self.stats.record_fallback();
                    continue;
                }
                Err(e) => {
                    // 非硬件错误（如参数错误）：直接返回
                    self.stats.record_failure();
                    return Err(e);
                }
            }
        }

        Err(CryptoError::EngineUnavailable)
    }
}

impl Default for CryptoSelector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 mock 引擎（总是成功）.
    struct MockEngine {
        name: &'static str,
        hw: bool,
        available: bool,
    }

    impl CryptoEngine for MockEngine {
        fn sm2_sign(&mut self, _key: &[u8], _msg: &[u8]) -> Result<[u8; 64], CryptoError> {
            Ok([0xAA; 64])
        }
        fn sm2_verify(
            &mut self,
            _pk: &[u8],
            _msg: &[u8],
            _sig: &[u8; 64],
        ) -> Result<bool, CryptoError> {
            Ok(true)
        }
        fn sm3_hash(&mut self, _data: &[u8]) -> Result<[u8; 32], CryptoError> {
            Ok([0xBB; 32])
        }
        fn sm4_encrypt(
            &mut self,
            _key: &[u8; 16],
            _iv: Option<&[u8]>,
            _mode: Sm4Mode,
            data: &[u8],
        ) -> Result<Vec<u8>, CryptoError> {
            Ok(data.to_vec())
        }
        fn sm4_decrypt(
            &mut self,
            _key: &[u8; 16],
            _iv: Option<&[u8]>,
            _mode: Sm4Mode,
            data: &[u8],
        ) -> Result<Vec<u8>, CryptoError> {
            Ok(data.to_vec())
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn is_hardware(&self) -> bool {
            self.hw
        }
        fn is_available(&self) -> bool {
            self.available
        }
    }

    /// 测试用 mock 引擎（总是硬件错误）.
    struct FailingHwEngine;

    impl CryptoEngine for FailingHwEngine {
        fn sm2_sign(&mut self, _key: &[u8], _msg: &[u8]) -> Result<[u8; 64], CryptoError> {
            Err(CryptoError::EngineTimeout)
        }
        fn sm2_verify(
            &mut self,
            _pk: &[u8],
            _msg: &[u8],
            _sig: &[u8; 64],
        ) -> Result<bool, CryptoError> {
            Err(CryptoError::EngineTimeout)
        }
        fn sm3_hash(&mut self, _data: &[u8]) -> Result<[u8; 32], CryptoError> {
            Err(CryptoError::EngineError)
        }
        fn sm4_encrypt(
            &mut self,
            _key: &[u8; 16],
            _iv: Option<&[u8]>,
            _mode: Sm4Mode,
            _data: &[u8],
        ) -> Result<Vec<u8>, CryptoError> {
            Err(CryptoError::DmaError)
        }
        fn sm4_decrypt(
            &mut self,
            _key: &[u8; 16],
            _iv: Option<&[u8]>,
            _mode: Sm4Mode,
            _data: &[u8],
        ) -> Result<Vec<u8>, CryptoError> {
            Err(CryptoError::DmaError)
        }
        fn name(&self) -> &'static str {
            "failing-hw"
        }
        fn is_hardware(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_selector_empty() {
        let mut sel = CryptoSelector::new();
        assert_eq!(sel.engine_count(), 0);
        assert!(sel.sm3_hash(b"test").is_err());
    }

    #[test]
    fn test_selector_single_engine() {
        let mut sel = CryptoSelector::new();
        sel.register(Box::new(MockEngine {
            name: "mock",
            hw: false,
            available: true,
        }));
        assert_eq!(sel.engine_count(), 1);
        assert_eq!(sel.active_engine_name(), "mock");

        let hash = sel.sm3_hash(b"hello").unwrap();
        assert_eq!(hash, [0xBB; 32]);
    }

    #[test]
    fn test_selector_fallback() {
        let mut sel = CryptoSelector::new();
        // 先注册故障硬件引擎
        sel.register(Box::new(FailingHwEngine));
        // 再注册正常软件引擎
        sel.register(Box::new(MockEngine {
            name: "software",
            hw: false,
            available: true,
        }));

        // 应自动 fallback 到软件引擎
        let hash = sel.sm3_hash(b"test").unwrap();
        assert_eq!(hash, [0xBB; 32]);
        assert_eq!(sel.active_engine_name(), "software");
        assert!(sel.stats().fallback_count > 0);
    }

    #[test]
    fn test_selector_unavailable_skip() {
        let mut sel = CryptoSelector::new();
        // 不可用引擎
        sel.register(Box::new(MockEngine {
            name: "unavailable",
            hw: true,
            available: false,
        }));
        // 可用引擎
        sel.register(Box::new(MockEngine {
            name: "available",
            hw: false,
            available: true,
        }));

        let hash = sel.sm3_hash(b"data").unwrap();
        assert_eq!(hash, [0xBB; 32]);
        assert_eq!(sel.active_engine_name(), "available");
    }

    #[test]
    fn test_engine_stats() {
        let mut stats = EngineStats::new();
        assert_eq!(stats.success_rate(), 100);

        stats.record_success(100);
        stats.record_success(200);
        stats.record_failure();
        assert_eq!(stats.total_ops, 3);
        assert_eq!(stats.success_ops, 2);
        assert_eq!(stats.failed_ops, 1);
        assert_eq!(stats.bytes_processed, 300);
        assert_eq!(stats.success_rate(), 66);
    }

    #[test]
    fn test_sm4_mode_values() {
        assert_eq!(Sm4Mode::Ecb as u8, 0);
        assert_eq!(Sm4Mode::Cbc as u8, 1);
        assert_eq!(Sm4Mode::Ctr as u8, 2);
        assert_eq!(Sm4Mode::Gcm as u8, 3);
    }

    #[test]
    fn test_algorithm_enum() {
        assert_ne!(Algorithm::Sm2, Algorithm::Sm3);
        assert_ne!(Algorithm::Sm3, Algorithm::Sm4);
        assert_eq!(Algorithm::Sm2, Algorithm::Sm2);
    }

    #[test]
    fn test_list_engines() {
        let mut sel = CryptoSelector::new();
        sel.register(Box::new(MockEngine {
            name: "hw-engine",
            hw: true,
            available: true,
        }));
        sel.register(Box::new(MockEngine {
            name: "sw-engine",
            hw: false,
            available: true,
        }));

        let list = sel.list_engines();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], ("hw-engine", true, true));
        assert_eq!(list[1], ("sw-engine", false, true));
    }
}
