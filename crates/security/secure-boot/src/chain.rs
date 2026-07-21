//! 信任链与启动阶段（v0.113.0，D5/D6）.
//!
//! D5：蓝图密钥 `[u8; 64]`（rom_root_key/bl_pubkey）与 SM2 未压缩公钥 65B
//! （0x04‖x‖y）格式不符，改用 eneros-crypto 的 [`Sm2PublicKey`] 强类型。
//! D6：删除蓝图 `ChainOfTrust.kernel_sig/runtime_sig` 死字段与
//! `bl_pubkey: [0u8;64]` 永不更新的零密钥 bug——下级验签密钥由
//! `BootVerifier::advance_stage` 显式传递并写入 [`ChainOfTrust::set_stage_key`]。

use eneros_crypto::Sm2PublicKey;

/// 启动阶段（四级信任链 + 完成态）.
///
/// 严格逐级推进：Rom → Bootloader → Kernel → Runtime → Complete。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    /// ROM 级（已由硬件根信任验证，`verify_stage` 直通，蓝图 §4.5 语义）.
    Rom,
    /// Bootloader 级（使用信任根公钥验签）.
    Bootloader,
    /// 内核级（使用 Bootloader 阶段安装的 stage_key 验签）.
    Kernel,
    /// Runtime 级（沿用/轮换 stage_key 验签）.
    Runtime,
    /// 信任链完成（`verify_stage` 直通，`advance_stage` 拒绝）.
    Complete,
}

/// 信任链状态（字段私有，经访问器读取）.
///
/// - `root_key`：信任根公钥，构造注入，用于 Bootloader 级验签
/// - `stage_key`：下级验签公钥（BL 公钥随已验签镜像体传递，其完整性由
///   哈希 + 签名覆盖传递可信），用于 Kernel/Runtime 级验签
/// - `current_stage`：当前启动阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainOfTrust {
    /// 信任根公钥（构造注入，集成层由熔丝/安全存储烧录）.
    root_key: Sm2PublicKey,
    /// 下级验签公钥（Bootloader→Kernel 推进时安装）.
    stage_key: Option<Sm2PublicKey>,
    /// 当前启动阶段.
    current_stage: BootStage,
}

impl ChainOfTrust {
    /// 以信任根公钥构造信任链（初始 stage = Rom，stage_key = None）.
    pub fn new(root_key: Sm2PublicKey) -> Self {
        Self {
            root_key,
            stage_key: None,
            current_stage: BootStage::Rom,
        }
    }

    /// 信任根公钥.
    pub fn root_key(&self) -> &Sm2PublicKey {
        &self.root_key
    }

    /// 下级验签公钥（未安装时为 None）.
    pub fn stage_key(&self) -> Option<&Sm2PublicKey> {
        self.stage_key.as_ref()
    }

    /// 当前启动阶段.
    pub fn current_stage(&self) -> BootStage {
        self.current_stage
    }

    /// 推进当前阶段（crate 内可见，仅 [`crate::BootVerifier`] 调用）.
    pub(crate) fn set_stage(&mut self, stage: BootStage) {
        self.current_stage = stage;
    }

    /// 安装/轮换下级验签公钥（crate 内可见，仅 [`crate::BootVerifier`] 调用）.
    pub(crate) fn set_stage_key(&mut self, key: Sm2PublicKey) {
        self.stage_key = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{CsRng, Sm2KeyPair};

    use super::*;

    /// 初始状态：current_stage == Rom 且 stage_key == None.
    #[test]
    fn chain_initial_state() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let chain = ChainOfTrust::new(kp.public_key);
        assert_eq!(chain.current_stage(), BootStage::Rom);
        assert!(chain.stage_key().is_none());
        assert_eq!(chain.root_key(), &kp.public_key);
    }
}
