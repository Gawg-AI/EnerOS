//! Agent 热更新状态机：数据模型 + `AgentHotUpdater` trait + 实现.
//!
//! 流程（蓝图 §4.3）：`register_agent` 登记当前版本（D6）→ `prepare`（ABI 真实比对 +
//! 模块字节登记，建上下文 Downloading）→ `request_started`/`request_finished` 追踪活跃
//! 请求（D7）→ `drain` 单步排空评估（D2）→ `swap` 经 [`ArcHotSwap`] 原子切换 →
//! `verify`（SM3 模块哈希常时比较 + 健康探针，D4）→ 失败 `rollback` 切回旧版本。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use eneros_crypto::{ct_eq, sm3_hash};
use heapless::String as HString;

use crate::hot_swap::ArcHotSwap;

/// 热更新状态（七态）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotUpdateState {
    /// 空闲
    Idle,
    /// 下载新版本
    Downloading,
    /// 排空旧版本请求
    Draining,
    /// 切换中
    Swapping,
    /// 验证新版本
    Verifying,
    /// 完成
    Completed,
    /// 失败
    Failed,
}

/// Agent 版本信息.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVersion {
    /// Agent 标识
    pub agent_id: u32,
    /// 版本号（≤ 32 字节）
    pub version: HString<32>,
    /// 模块 SM3 哈希（verify 校验目标）
    pub module_hash: [u8; 32],
    /// 入口符号（≤ 64 字节）
    pub entry_point: HString<64>,
    /// ABI 版本（prepare 时与登记版本真实比对，D6）
    pub abi_version: u16,
}

/// 热更新上下文（prepare 时创建，Completed/Failed 后可开启新一轮）.
#[derive(Debug, Clone)]
pub struct HotUpdateContext {
    /// Agent 标识
    pub agent_id: u32,
    /// 旧版本（登记当前版本快照，rollback 目标）
    pub old_version: AgentVersion,
    /// 新版本（swap 切换目标）
    pub new_version: AgentVersion,
    /// 当前状态
    pub state: HotUpdateState,
    /// 活跃请求数（drain/swap 时同步自计数器）
    pub active_requests: u32,
    /// 排空超时（毫秒）
    pub drain_timeout_ms: u64,
    /// 更新起始时间戳（D1：经 prepare 的 now_ms 参数注入）
    pub start_ms: u64,
}

/// 热更新错误.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotUpdateError {
    /// Agent 未注册 / 无更新上下文
    AgentNotFound,
    /// ABI 不兼容（expect 登记版本，got 新版本）
    AbiIncompatible {
        /// 期望的 ABI 版本
        expect: u16,
        /// 实际收到的 ABI 版本
        got: u16,
    },
    /// 排空超时（active 为超时时刻的活跃请求数）
    DrainTimeout {
        /// 超时时刻仍活跃的请求数
        active: u32,
    },
    /// 模块加载失败
    ModuleLoadFailed,
    /// 验证失败（模块哈希不匹配或健康探针未通过）
    VerificationFailed,
    /// 已有更新在进行 / 状态不允许该操作
    AlreadyInProgress,
    /// 无旧版本可回滚
    NoOldVersionToRollback,
}

/// Agent 热更新器 trait.
///
/// 时间戳一律经 `now_ms` 参数传入（D1）；`drain` 为单步评估语义（D2），
/// 返回 `Ok(())` 且活跃请求未归零时由调用方稍后重试。
pub trait AgentHotUpdater {
    /// 准备更新：ABI 真实比对登记版本（D6），登记模块字节供 verify 校验.
    fn prepare(
        &mut self,
        agent_id: u32,
        new_version: AgentVersion,
        module_bytes: &[u8],
        now_ms: u64,
    ) -> Result<(), HotUpdateError>;

    /// 排空评估（D2 单步）：活跃=0 → Ok 可 swap；超时 → DrainTimeout；否则保持
    /// Draining 由调用方重试.
    fn drain(&mut self, agent_id: u32, timeout_ms: u64, now_ms: u64) -> Result<(), HotUpdateError>;

    /// 原子切换到新版本（仅 Draining 态可执行），切换后进入 Verifying.
    fn swap(&mut self, agent_id: u32) -> Result<(), HotUpdateError>;

    /// 验证新版本：SM3 模块哈希 + 健康探针（D4）；双过 → Completed.
    fn verify(&mut self, agent_id: u32) -> Result<bool, HotUpdateError>;

    /// 回滚：ArcHotSwap 切回旧版本，状态置 Failed.
    fn rollback(&mut self, agent_id: u32) -> Result<(), HotUpdateError>;

    /// 查询更新上下文.
    fn get_state(&self, agent_id: u32) -> Option<&HotUpdateContext>;
}

/// Agent 热更新管理器实现.
///
/// 容量口径：最多并发管理 16 个 Agent（heapless 定容，无堆增长）。
pub struct AgentHotUpdaterImpl {
    /// 更新上下文表（agent_id → ctx，prepare 时登记）
    contexts: heapless::Vec<(u32, HotUpdateContext), 16>,
    /// 活跃请求计数表（D7：路由层经 request_started/finished 维护）
    active_counts: heapless::Vec<(u32, AtomicU32), 16>,
    /// 版本槽（agent_id → 当前版本原子指针，register_agent 初始化）
    slots: heapless::Vec<(u32, ArcHotSwap<AgentVersion>), 16>,
    /// 新模块镜像（prepare 登记，verify 做 SM3 校验）
    module_images: heapless::Vec<(u32, Vec<u8>), 16>,
    /// 健康探针（D4：with_probe 注册，None 视为恒 true）
    probe: Option<fn(u32) -> bool>,
}

impl AgentHotUpdaterImpl {
    /// 创建空管理器（无健康探针，verify 仅做 SM3 校验）.
    pub fn new() -> Self {
        Self {
            contexts: heapless::Vec::new(),
            active_counts: heapless::Vec::new(),
            slots: heapless::Vec::new(),
            module_images: heapless::Vec::new(),
            probe: None,
        }
    }

    /// 创建带健康探针的管理器（D4：`probe(agent_id)` 返回 false 视为不健康）.
    pub fn with_probe(probe: fn(u32) -> bool) -> Self {
        Self {
            probe: Some(probe),
            ..Self::new()
        }
    }

    /// 登记 Agent 当前版本（D6）：初始化版本槽与活跃计数器.
    ///
    /// 重复注册语义：**更新**登记版本（经 ArcHotSwap 原子换入），活跃计数保持不变；
    /// 容量满时静默忽略（heapless 定容口径）。
    pub fn register_agent(&mut self, agent_id: u32, current: AgentVersion) {
        if let Some((_, slot)) = self.slots.iter().find(|(id, _)| *id == agent_id) {
            // 重复注册：更新登记版本（旧 Arc 随返回值 drop 释放）
            let _old = slot.swap(Arc::new(current));
            return;
        }
        self.slots
            .push((agent_id, ArcHotSwap::new(Arc::new(current))))
            .ok();
        self.active_counts.push((agent_id, AtomicU32::new(0))).ok();
    }

    /// 请求进入（D7：路由层调用），活跃计数 +1；未注册 Agent 忽略.
    pub fn request_started(&self, agent_id: u32) {
        if let Some((_, c)) = self.active_counts.iter().find(|(id, _)| *id == agent_id) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 请求完成（D7：路由层调用），活跃计数 -1；未注册 Agent 忽略.
    pub fn request_finished(&self, agent_id: u32) {
        if let Some((_, c)) = self.active_counts.iter().find(|(id, _)| *id == agent_id) {
            c.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// 读取登记版本槽当前版本（D8：公开——路由层经此把新请求路由到当前版本，
    /// 集成测试经此断言 swap 前后路由目标；未注册返回 None）.
    pub fn slot_load(&self, agent_id: u32) -> Option<Arc<AgentVersion>> {
        self.slots
            .iter()
            .find(|(id, _)| *id == agent_id)
            .map(|(_, s)| s.load())
    }

    /// 读取活跃请求计数（未注册视为 0）.
    fn active_count(&self, agent_id: u32) -> u32 {
        self.active_counts
            .iter()
            .find(|(id, _)| *id == agent_id)
            .map_or(0, |(_, c)| c.load(Ordering::Relaxed))
    }

    /// 上下文索引（无则 None）.
    fn context_index(&self, agent_id: u32) -> Option<usize> {
        self.contexts.iter().position(|(id, _)| *id == agent_id)
    }
}

impl Default for AgentHotUpdaterImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentHotUpdater for AgentHotUpdaterImpl {
    fn prepare(
        &mut self,
        agent_id: u32,
        new_version: AgentVersion,
        module_bytes: &[u8],
        now_ms: u64,
    ) -> Result<(), HotUpdateError> {
        // D6：未登记 → AgentNotFound；登记版本快照即 old_version
        let registered = self
            .slot_load(agent_id)
            .ok_or(HotUpdateError::AgentNotFound)?;
        // 进行中判定：仅 Idle/Completed/Failed 可开启新一轮
        if let Some(idx) = self.context_index(agent_id) {
            match self.contexts[idx].1.state {
                HotUpdateState::Idle | HotUpdateState::Completed | HotUpdateState::Failed => {}
                _ => return Err(HotUpdateError::AlreadyInProgress),
            }
        }
        // D6：ABI 与登记当前版本真实比对
        if registered.abi_version != new_version.abi_version {
            return Err(HotUpdateError::AbiIncompatible {
                expect: registered.abi_version,
                got: new_version.abi_version,
            });
        }
        // 登记新模块字节（重复 prepare 替换，供 verify 做 SM3 校验）
        let image = Vec::from(module_bytes);
        if let Some((_, img)) = self
            .module_images
            .iter_mut()
            .find(|(id, _)| *id == agent_id)
        {
            *img = image;
        } else {
            self.module_images.push((agent_id, image)).ok();
        }
        // 建/换上下文（old_version = 登记当前版本，D1：start_ms 经参数注入）
        let ctx = HotUpdateContext {
            agent_id,
            old_version: (*registered).clone(),
            new_version,
            state: HotUpdateState::Downloading,
            active_requests: self.active_count(agent_id),
            drain_timeout_ms: 30_000,
            start_ms: now_ms,
        };
        if let Some(idx) = self.context_index(agent_id) {
            self.contexts[idx].1 = ctx;
        } else {
            self.contexts.push((agent_id, ctx)).ok();
        }
        Ok(())
    }

    fn drain(&mut self, agent_id: u32, timeout_ms: u64, now_ms: u64) -> Result<(), HotUpdateError> {
        let active = self.active_count(agent_id);
        let idx = self
            .context_index(agent_id)
            .ok_or(HotUpdateError::AgentNotFound)?;
        let ctx = &mut self.contexts[idx].1;
        ctx.state = HotUpdateState::Draining;
        ctx.drain_timeout_ms = timeout_ms;
        ctx.active_requests = active;
        // D2 单步评估：活跃=0 → 可 swap；超时 → DrainTimeout；否则保持 Draining
        if active == 0 {
            return Ok(());
        }
        if now_ms.saturating_sub(ctx.start_ms) > timeout_ms {
            return Err(HotUpdateError::DrainTimeout { active });
        }
        Ok(())
    }

    fn swap(&mut self, agent_id: u32) -> Result<(), HotUpdateError> {
        let idx = self
            .context_index(agent_id)
            .ok_or(HotUpdateError::AgentNotFound)?;
        // 仅 Draining 态可执行
        if self.contexts[idx].1.state != HotUpdateState::Draining {
            return Err(HotUpdateError::AlreadyInProgress);
        }
        // 经 ArcHotSwap 原子切换到新版本；返回的旧 Arc 随作用域 drop，
        // 管理器释放旧版本引用，在途 load 持有者不受影响（零中断）
        let new_v = self.contexts[idx].1.new_version.clone();
        if let Some((_, slot)) = self.slots.iter().find(|(id, _)| *id == agent_id) {
            let _old = slot.swap(Arc::new(new_v));
        }
        let active = self.active_count(agent_id);
        let ctx = &mut self.contexts[idx].1;
        ctx.state = HotUpdateState::Verifying;
        ctx.active_requests = active;
        Ok(())
    }

    fn verify(&mut self, agent_id: u32) -> Result<bool, HotUpdateError> {
        let idx = self
            .context_index(agent_id)
            .ok_or(HotUpdateError::AgentNotFound)?;
        // D4：SM3 模块哈希常时比较（复用 eneros-crypto）
        let hash_ok = match self.module_images.iter().find(|(id, _)| *id == agent_id) {
            Some((_, img)) => {
                let digest = sm3_hash(img);
                ct_eq(&digest, &self.contexts[idx].1.new_version.module_hash)
            }
            None => false,
        };
        // D4：健康探针（未注册视为通过）
        let probe_ok = self.probe.map_or(true, |p| p(agent_id));
        let ctx = &mut self.contexts[idx].1;
        if hash_ok && probe_ok {
            ctx.state = HotUpdateState::Completed;
            Ok(true)
        } else {
            ctx.state = HotUpdateState::Failed;
            Err(HotUpdateError::VerificationFailed)
        }
    }

    fn rollback(&mut self, agent_id: u32) -> Result<(), HotUpdateError> {
        let idx = self
            .context_index(agent_id)
            .ok_or(HotUpdateError::AgentNotFound)?;
        // 切回旧版本（旧槽位实例随返回 Arc drop 释放）
        let old_v = self.contexts[idx].1.old_version.clone();
        if let Some((_, slot)) = self.slots.iter().find(|(id, _)| *id == agent_id) {
            let _cur = slot.swap(Arc::new(old_v));
        }
        self.contexts[idx].1.state = HotUpdateState::Failed;
        Ok(())
    }

    fn get_state(&self, agent_id: u32) -> Option<&HotUpdateContext> {
        self.contexts
            .iter()
            .find(|(id, _)| *id == agent_id)
            .map(|(_, ctx)| ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试版本（module_hash 默认全 0，需要 SM3 校验的用例单独填）
    fn ver(agent_id: u32, version: &str, abi: u16) -> AgentVersion {
        AgentVersion {
            agent_id,
            version: HString::try_from(version).unwrap(),
            module_hash: [0u8; 32],
            entry_point: HString::try_from("main").unwrap(),
            abi_version: abi,
        }
    }

    /// 读取槽位当前版本（测试便捷函数）
    fn current_version(u: &AgentHotUpdaterImpl, agent_id: u32) -> AgentVersion {
        (*u.slot_load(agent_id).unwrap()).clone()
    }

    // HOT1：初始 get_state 为 None
    #[test]
    fn hot1_initial_state_is_none() {
        let u = AgentHotUpdaterImpl::new();
        assert!(u.get_state(1).is_none());
    }

    // HOT2：register 后可查（prepare 后上下文字段正确）
    #[test]
    fn hot2_register_then_prepare_context_fields() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        // 登记版本可经槽位读取
        assert_eq!(current_version(&u, 1).version.as_str(), "1.0.0");
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 1000).unwrap();
        let ctx = u.get_state(1).unwrap();
        assert_eq!(ctx.agent_id, 1);
        assert_eq!(ctx.old_version.version.as_str(), "1.0.0");
        assert_eq!(ctx.new_version.version.as_str(), "2.0.0");
        assert_eq!(ctx.start_ms, 1000);
        assert_eq!(ctx.drain_timeout_ms, 30_000);
    }

    // HOT3：prepare 建上下文 Downloading
    #[test]
    fn hot3_prepare_creates_downloading_context() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 500).unwrap();
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Downloading);
    }

    // HOT4：重复 prepare → AlreadyInProgress
    #[test]
    fn hot4_duplicate_prepare_rejected() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 0).unwrap();
        let err = u.prepare(1, ver(1, "2.0.1", 1), b"mod2", 1).unwrap_err();
        assert_eq!(err, HotUpdateError::AlreadyInProgress);
    }

    // HOT5：ABI 不等 → AbiIncompatible 字段正确（D6 真实比对）
    #[test]
    fn hot5_abi_incompatible_rejected() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        let err = u.prepare(1, ver(1, "2.0.0", 2), b"mod", 0).unwrap_err();
        assert_eq!(err, HotUpdateError::AbiIncompatible { expect: 1, got: 2 });
    }

    // HOT6：drain 活跃=0 → Ok 且 Draining
    #[test]
    fn hot6_drain_passes_when_no_active_requests() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 0).unwrap();
        u.drain(1, 30_000, 0).unwrap();
        let ctx = u.get_state(1).unwrap();
        assert_eq!(ctx.state, HotUpdateState::Draining);
        assert_eq!(ctx.active_requests, 0);
    }

    // HOT7：drain 超时 → DrainTimeout{active}
    #[test]
    fn hot7_drain_timeout_reports_active() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 1000).unwrap();
        u.request_started(1);
        // 活跃=1 未超时：保持 Draining，调用方重试
        u.drain(1, 100, 1050).unwrap();
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Draining);
        // 超时：DrainTimeout{active: 1}
        let err = u.drain(1, 100, 1201).unwrap_err();
        assert_eq!(err, HotUpdateError::DrainTimeout { active: 1 });
    }

    // HOT8：swap 非 Draining 拒绝；Draining 后成功 → Verifying 且槽位为新版本
    #[test]
    fn hot8_swap_state_check_and_reroute() {
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        u.prepare(1, ver(1, "2.0.0", 1), b"mod", 0).unwrap();
        // Downloading 态拒绝
        let err = u.swap(1).unwrap_err();
        assert_eq!(err, HotUpdateError::AlreadyInProgress);
        assert_eq!(current_version(&u, 1).version.as_str(), "1.0.0");
        // Draining 后 swap 成功
        u.drain(1, 30_000, 0).unwrap();
        u.swap(1).unwrap();
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Verifying);
        assert_eq!(current_version(&u, 1).version.as_str(), "2.0.0");
    }

    // HOT9：verify 双过（SM3 哈希匹配 + 默认探针）→ Completed
    #[test]
    fn hot9_verify_success_completes() {
        let module = b"agent-module-v2-bytes";
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        let mut v2 = ver(1, "2.0.0", 1);
        v2.module_hash = sm3_hash(module);
        u.prepare(1, v2, module, 0).unwrap();
        u.drain(1, 30_000, 0).unwrap();
        u.swap(1).unwrap();
        assert!(u.verify(1).unwrap());
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Completed);
    }

    // HOT10：探针 false → Failed + VerificationFailed
    #[test]
    fn hot10_verify_probe_failure_fails() {
        fn always_unhealthy(_id: u32) -> bool {
            false
        }
        let module = b"agent-module-v2-bytes";
        let mut u = AgentHotUpdaterImpl::with_probe(always_unhealthy);
        u.register_agent(1, ver(1, "1.0.0", 1));
        let mut v2 = ver(1, "2.0.0", 1);
        v2.module_hash = sm3_hash(module);
        u.prepare(1, v2, module, 0).unwrap();
        u.drain(1, 30_000, 0).unwrap();
        u.swap(1).unwrap();
        let err = u.verify(1).unwrap_err();
        assert_eq!(err, HotUpdateError::VerificationFailed);
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Failed);
    }

    // HOT11：rollback → 槽位恢复旧版本 + state=Failed
    #[test]
    fn hot11_rollback_restores_old_version() {
        let module = b"agent-module-v2-bytes";
        let mut u = AgentHotUpdaterImpl::new();
        u.register_agent(1, ver(1, "1.0.0", 1));
        let mut v2 = ver(1, "2.0.0", 1);
        v2.module_hash = sm3_hash(module);
        u.prepare(1, v2, module, 0).unwrap();
        u.drain(1, 30_000, 0).unwrap();
        u.swap(1).unwrap();
        assert_eq!(current_version(&u, 1).version.as_str(), "2.0.0");
        u.rollback(1).unwrap();
        assert_eq!(current_version(&u, 1).version.as_str(), "1.0.0");
        assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Failed);
    }

    // HOT12：未注册 Agent 各方法 → AgentNotFound / None；计数调用静默忽略
    #[test]
    fn hot12_unregistered_agent_not_found() {
        let mut u = AgentHotUpdaterImpl::new();
        assert_eq!(
            u.prepare(9, ver(9, "2.0.0", 1), b"mod", 0).unwrap_err(),
            HotUpdateError::AgentNotFound
        );
        assert_eq!(
            u.drain(9, 100, 0).unwrap_err(),
            HotUpdateError::AgentNotFound
        );
        assert_eq!(u.swap(9).unwrap_err(), HotUpdateError::AgentNotFound);
        assert_eq!(u.verify(9).unwrap_err(), HotUpdateError::AgentNotFound);
        assert_eq!(u.rollback(9).unwrap_err(), HotUpdateError::AgentNotFound);
        assert!(u.get_state(9).is_none());
        // 未注册计数调用不 panic
        u.request_started(9);
        u.request_finished(9);
        assert_eq!(u.active_count(9), 0);
    }
}
