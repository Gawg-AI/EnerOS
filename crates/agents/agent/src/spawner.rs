//! Agent 启动器 — AgentSpawner / StaticAgentRegistry (AGT-003 real ELF spawn).
//!
//! # 设计
//! - `StaticAgentRegistry` 是编译时静态链接的 `AgentType → ELF bytes` 映射（第五册 §1）。
//!   所有 Agent 以签名的 ELF64 PIE 二进制形式静态链接进系统映像，无动态装载、
//!   无插件 ABI、无跨二进制 trait-object（MASTER §G6 禁止捷径）。
//! - `AgentSpawner` 持有共享 registry / lifecycle / `&'static StaticAgentRegistry`，
//!   执行真实 spawn 流程：解析 ELF → 创建 `ProcessHandle` → 生命周期转换。
//! - spawn 流程不再调用 `AgentEntry::on_init/on_start`（这是 MASTER §G6 明确禁止的
//!   "Box<dyn AgentEntry> 的 init/start 返回即算 spawn" 捷径）。取而代之的是
//!   创建代表真实隔离 EL0 进程的 `ProcessHandle`，进程内部初始化由进程自身完成。
//!
//! # 偏差声明
//! - D1: `lifecycle: Rc<RefCell<LifecycleManager>>`（非蓝图的 `Rc<LifecycleManager>`），因 `force_state` 需要 `&mut self`
//! - D2(修订): 原 `AgentFactory` trait 已删除（第五册 §1 禁止动态插件路径），改为 `StaticAgentRegistry` 静态注册表
//! - D3: `spawn_blocking` 委托 `spawn`（Phase 1 单线程无异步运行时）
//! - D4: `spawn` 签名追加 `now: u64`（no_std 时间戳外部提供）
//! - D5: 错误清理用 `force_state`（Ready→Error 不在 TRANSITIONS 表中，`transition` 会拒绝）
//! - D6(AGT-003): spawn 创建 `ProcessHandle`（真实进程抽象），不创建 `Box<dyn AgentEntry>`
//! - D7(AGT-003): `StaticAgentRegistry` 存储 `&'static [u8]` ELF 字节，不存储 spawn 函数
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::{
    elf::LoadedElf, AgentConfig, AgentDescriptor, AgentError, AgentId, AgentPackage, AgentRegistry,
    AgentPackageManifest, AgentState, AgentType, LifecycleManager, ProcessHandle,
    ResourceBudget,
};

// ============================================================================
// StaticAgentRegistry — AgentType → ELF bytes (static, compile-time known)
// ============================================================================

/// 静态链接应用注册表（第五册 §1, AGT-003）.
///
/// 编译时已知的 `AgentType → ELF bytes` 映射。所有 Agent 以签名的 ELF64 PIE
/// 二进制形式静态链接进系统映像。无动态装载、无插件 ABI、无 trait-object 跨二进制。
///
/// ELF 字节在 spawn 时由 `LoadedElf::parse` 解析和验证（ELF64 + AArch64 + PIE +
/// W^X + 有界检查），然后创建 `ProcessHandle` 代表真实隔离进程。
pub struct AgentBinary {
    /// Agent ELF64 PIE 字节。
    pub elf: &'static [u8],
    /// 签名清单（SEC-002）。`None` 表示该条目未提供签名材料，
    /// spawn 时一律拒绝加载（fail-closed）。
    pub manifest: Option<AgentPackageManifest>,
}

pub struct StaticAgentRegistry {
    entries: &'static [(AgentType, AgentBinary)],
}

impl StaticAgentRegistry {
    /// 创建静态注册表。`entries` 是 `&'static [(AgentType, AgentBinary)]` 数组字面量。
    ///
    /// 注意：SEC-002 后 spawn 强制验签；条目必须携带 `Some(manifest)`（签名清单），
    /// `None` 条目会在 spawn 时被拒绝（未签名不加载）。
    pub const fn new(entries: &'static [(AgentType, AgentBinary)]) -> Self {
        Self { entries }
    }

    /// 查找 AgentType 对应的二进制条目（ELF + 签名清单）。
    pub fn get_binary(&self, agent_type: AgentType) -> Option<&'static AgentBinary> {
        for (ty, binary) in self.entries {
            if *ty == agent_type {
                return Some(binary);
            }
        }
        None
    }

    /// 返回已注册的 Agent 类型数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ============================================================================
// AgentSpawner — real ELF spawn → ProcessHandle
// ============================================================================

/// Agent 启动器.
///
/// 持有共享注册表、生命周期管理器与静态注册表引用，执行 Agent 启动流程。
///
/// spawn 流程创建 `ProcessHandle`（真实进程抽象），不创建 `Box<dyn AgentEntry>`
/// （MASTER §G6 禁止捷径 D6）。
pub struct AgentSpawner {
    registry: Rc<RefCell<AgentRegistry>>,
    lifecycle: Rc<RefCell<LifecycleManager>>,
    agents: &'static StaticAgentRegistry,
    /// 已 spawn 的进程句柄（按 spawn 顺序存储）.
    processes: RefCell<Vec<ProcessHandle>>,
}

impl AgentSpawner {
    /// 创建 AgentSpawner.
    pub fn new(
        registry: Rc<RefCell<AgentRegistry>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        agents: &'static StaticAgentRegistry,
    ) -> Self {
        AgentSpawner {
            registry,
            lifecycle,
            agents,
            processes: RefCell::new(Vec::new()),
        }
    }

    /// 启动 Agent（D4 偏差：追加 `now: u64` 参数）.
    ///
    /// 执行真实 ELF spawn 流程（AGT-003 D6）：
    /// 1. 创建 `AgentDescriptor::new(config.agent_type, &config.name, now)`
    /// 2. 应用 `priority_override` / `mem_override`
    /// 3. 注册到 registry
    /// 4. `Created→Ready` 转换
    /// 5. 从静态注册表查找 ELF 字节 — 失败时 `force_state(id, Error)` 并返回 `CodeLoadFailed`（D5）
    /// 6. 解析 ELF（`LoadedElf::parse`）— 失败时 `force_state(id, Error)` 并返回 `CodeLoadFailed`（D5）
    /// 7. 创建 `ProcessHandle`（真实进程抽象）
    /// 8. 检查内存预算 — 超限时 `force_state(id, Error)` 并返回错误（D5）
    /// 9. `ProcessHandle::start()` — 失败时 `force_state(id, Error)` 并返回错误（D5）
    /// 10. `Ready→Running` 转换
    /// 11. 存储 `ProcessHandle`
    /// 12. 返回 `Ok(id)`
    pub fn spawn(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError> {
        // Step 1: 创建描述符
        let mut desc = AgentDescriptor::new(config.agent_type, &config.name, now);

        // Step 2: 应用覆盖
        if let Some(p) = config.priority_override {
            desc.priority = p;
        }
        if let Some(m) = config.mem_override {
            desc.mem_quota = m;
        }

        // Step 3: 注册到 registry
        let id = self.registry.borrow_mut().register(desc)?;

        // Step 4: Created → Ready
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Ready)
            .map_err(|e| {
                let _ = self
                    .lifecycle
                    .borrow_mut()
                    .force_state(id, AgentState::Error);
                e
            })?;

        // Step 5: 查找 ELF 字节 + 签名清单（SEC-002）
        let binary = self.agents.get_binary(config.agent_type).ok_or_else(|| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            AgentError::CodeLoadFailed(String::from("no agent ELF registered for type"))
        })?;
        let elf_bytes = binary.elf;

        // Step 5.5: 签名验证（SEC-002）— 未签名/错签/哈希不符一律拒绝加载
        let manifest = binary.manifest.clone().ok_or_else(|| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            AgentError::CodeLoadFailed(String::from(
                "agent package has no signature manifest (unsigned; SEC-002)",
            ))
        })?;
        let pkg = AgentPackage {
            manifest,
            elf_bytes: elf_bytes.to_vec(),
        };
        pkg.verify().map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            AgentError::CodeLoadFailed(alloc::format!("agent package verification failed: {e}"))
        })?;

        // Step 6: 解析 ELF
        let loaded_elf = LoadedElf::parse(elf_bytes).map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            AgentError::CodeLoadFailed(alloc::format!("ELF parse failed: {}", e))
        })?;

        // Step 7: 创建 ProcessHandle
        let budget = ResourceBudget {
            priority: config.priority_override.unwrap_or(128),
            max_memory_bytes: config.mem_override.unwrap_or(64 * 1024 * 1024) as u64,
            max_cpu_us_per_tick: 10_000,
            max_ipc_per_sec: 1000,
        };
        let mut process = ProcessHandle::new(
            id,
            config.agent_type,
            &config.name,
            loaded_elf.entry(),
            loaded_elf.segments(),
            budget,
        );

        // Step 8: 检查内存预算
        if process.exceeds_memory_budget() {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            return Err(AgentError::CodeLoadFailed(String::from(
                "agent ELF memory exceeds budget",
            )));
        }

        // Step 9: 启动进程
        process.start().map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            AgentError::InitFailed(e)
        })?;

        // Step 10: Ready → Running
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Running)
            .map_err(|e| {
                let _ = self
                    .lifecycle
                    .borrow_mut()
                    .force_state(id, AgentState::Error);
                e
            })?;

        // Step 11: 存储 ProcessHandle
        self.processes.borrow_mut().push(process);

        // Step 12: 返回 Ok(id)
        Ok(id)
    }

    /// 阻塞式启动 Agent（D3 偏差：Phase 1 单线程下与 `spawn` 等价）.
    pub fn spawn_blocking(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError> {
        self.spawn(config, now)
    }

    /// 获取已 spawn 的进程句柄引用（按 AgentId 查找）.
    pub fn get_process(&self, id: AgentId) -> Option<ProcessHandle> {
        self.processes
            .borrow()
            .iter()
            .find(|p| p.agent_id == id)
            .cloned()
    }

    /// 已 spawn 的进程数.
    pub fn process_count(&self) -> usize {
        self.processes.borrow().len()
    }
}

// ============================================================================
// Tests
// ============================================================================

/// 构建最小有效 ELF64 PIE AArch64 二进制（128 字节）.
///
/// 用于测试 spawner 的真实 ELF 解析路径。这是一个 `const fn`，可在 `static` 上下文中使用。
#[cfg(test)]
const fn build_minimal_test_elf() -> [u8; 128] {
    let mut buf = [0u8; 128];

    // --- e_ident (16 bytes) ---
    buf[0] = 0x7f; // ELF magic
    buf[1] = 0x45; // 'E'
    buf[2] = 0x4c; // 'L'
    buf[3] = 0x46; // 'F'
    buf[4] = 2; // ELFCLASS64
    buf[5] = 1; // ELFDATA2LSB
    buf[6] = 1; // EV_CURRENT

    // --- e_type = ET_DYN (3) at offset 16 ---
    buf[16] = 3;
    buf[17] = 0;

    // --- e_machine = EM_AARCH64 (183) at offset 18 ---
    buf[18] = 0xB7;
    buf[19] = 0;

    // --- e_entry = 0x1000 at offset 24 (8 bytes LE) ---
    buf[24] = 0x00;
    buf[25] = 0x10;

    // --- e_phoff = 64 at offset 32 (8 bytes LE) ---
    buf[32] = 64;

    // --- e_phentsize = 56 at offset 54 (2 bytes LE) ---
    buf[54] = 56;

    // --- e_phnum = 1 at offset 56 (2 bytes LE) ---
    buf[56] = 1;

    // --- Program header at offset 64 (56 bytes) ---
    let ph = 64;

    // p_type = PT_LOAD (1)
    buf[ph] = 1;

    // p_flags = PF_R | PF_X (5) — read + execute, no write (W^X OK)
    buf[ph + 4] = 5;

    // p_offset = 120 (code starts after ehdr + phdr = 64 + 56 = 120)
    buf[ph + 8] = 120;

    // p_vaddr = 0x1000 (matches entry point)
    buf[ph + 16] = 0x00;
    buf[ph + 17] = 0x10;

    // p_filesz = 8
    buf[ph + 32] = 8;

    // p_memsz = 8
    buf[ph + 40] = 8;

    // p_align = 0x1000
    buf[ph + 48] = 0x00;
    buf[ph + 49] = 0x10;

    // --- Code at offset 120 (8 bytes): NOP + RET ---
    buf[120] = 0x1f; // NOP
    buf[121] = 0x20;
    buf[122] = 0x03;
    buf[123] = 0xd5;
    buf[124] = 0xc0; // RET
    buf[125] = 0x03;
    buf[126] = 0x5f;
    buf[127] = 0xd6;

    buf
}

/// 最小有效 ELF 字节（static, 用于 StaticAgentRegistry）.
#[cfg(test)]
static MINIMAL_ELF: &[u8] = &build_minimal_test_elf();

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::string::String;
    use core::cell::RefCell;

    use super::*;
    use crate::{AgentRegistry, LifecycleManager, ProcessState};

    // ---- Static registries (compile-time known ELF bytes) ----
    //
    // SEC-002：spawn 强制验签。`SUCCESS_REGISTRY` 条目 manifest: None（未签名），
    // 用于验证"未签名拒绝加载"；签名正路径使用 `signed_registry()`。

    /// SEC-002 测试辅助：用固定私钥（d=1）对 ELF 生成确定性 SM2 签名清单。
    ///
    /// 确定性 rng（`CsRng::for_deterministic_signature`）保证同一 ELF 每次
    /// 签名一致；私钥 d=1 是合法标量（d ∈ [1, n-1]），验签侧用同一公钥。
    fn signed_manifest(agent_type: AgentType) -> AgentPackageManifest {
        use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair, Sm2PrivateKey};
        use eneros_crypto::CsRng;

        let name = match agent_type {
            AgentType::Energy => "energy",
            AgentType::Device => "device",
            AgentType::Market => "market",
            AgentType::Grid => "grid",
            _ => "agent",
        };
        let version = "1.0.0";
        let hash = crate::process::sm3_hash(MINIMAL_ELF);

        let mut d = [0u8; 32];
        d[31] = 1; // 固定私钥 d = 1
        let sk = Sm2PrivateKey::from_bytes(&d).expect("fixed sk");
        let keypair = Sm2KeyPair::from_private_key(&sk).expect("keypair");

        let mut msg = alloc::vec::Vec::new();
        msg.extend_from_slice(name.as_bytes());
        msg.extend_from_slice(version.as_bytes());
        msg.extend_from_slice(&hash);
        let mut rng = CsRng::for_deterministic_signature(b"sec002-test", &msg, b"spawner");
        let sig = sm2_sign(&msg, &keypair.private_key, &keypair.public_key, &mut rng)
            .expect("deterministic sign");
        let sig_bytes = sig.to_bytes();
        let uncompressed = keypair.public_key.to_bytes_uncompressed();
        let mut pk_bytes = [0u8; 64];
        pk_bytes.copy_from_slice(&uncompressed[1..65]);

        AgentPackageManifest {
            name: String::from(name),
            version: String::from(version),
            agent_type,
            budget: ResourceBudget::default(),
            elf_hash: hash,
            signature: sig_bytes,
            signer_pubkey: pk_bytes,
        }
    }

    fn signed_binary(agent_type: AgentType) -> AgentBinary {
        AgentBinary {
            elf: MINIMAL_ELF,
            manifest: Some(signed_manifest(agent_type)),
        }
    }

    static SUCCESS_REGISTRY: StaticAgentRegistry = StaticAgentRegistry::new(&[
        (AgentType::Energy, AgentBinary { elf: MINIMAL_ELF, manifest: None }),
        (AgentType::Device, AgentBinary { elf: MINIMAL_ELF, manifest: None }),
        (AgentType::Market, AgentBinary { elf: MINIMAL_ELF, manifest: None }),
        (AgentType::Grid, AgentBinary { elf: MINIMAL_ELF, manifest: None }),
    ]);

    /// 签名注册表（SEC-002 正路径）：所有条目携带真实 SM2 签名清单。
    fn signed_registry() -> &'static StaticAgentRegistry {
        use std::sync::OnceLock;
        static REGS: OnceLock<StaticAgentRegistry> = OnceLock::new();
        REGS.get_or_init(|| {
            let entries: &'static [(AgentType, AgentBinary)] = Box::leak(Box::new([
                (AgentType::Energy, signed_binary(AgentType::Energy)),
                (AgentType::Device, signed_binary(AgentType::Device)),
                (AgentType::Market, signed_binary(AgentType::Market)),
                (AgentType::Grid, signed_binary(AgentType::Grid)),
            ]));
            StaticAgentRegistry::new(entries)
        })
    }

    static FAIL_REGISTRY: StaticAgentRegistry = StaticAgentRegistry::new(&[]);

    // ---- Helper functions ----

    /// Create an AgentSpawner with shared registry and lifecycle.
    fn make_spawner(
        agents: &'static StaticAgentRegistry,
    ) -> (
        AgentSpawner,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<LifecycleManager>>,
    ) {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
        let spawner = AgentSpawner::new(reg.clone(), lifecycle.clone(), agents);
        (spawner, reg, lifecycle)
    }

    /// Create a minimal AgentConfig.
    fn make_config(agent_type: AgentType, name: &str) -> AgentConfig {
        AgentConfig {
            agent_type,
            name: String::from(name),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        }
    }

    // ---- Tests ----

    #[test]
    fn test_spawn_success() {
        let (spawner, _reg, lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        let result = spawner.spawn(config, 1000);
        assert!(result.is_ok());
        let id = result.unwrap();
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_creates_process_handle() {
        let (spawner, _reg, _lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        let id = spawner.spawn(config, 1000).unwrap();

        // ProcessHandle should be stored.
        assert_eq!(spawner.process_count(), 1);

        // Verify the process handle.
        let process = spawner
            .get_process(id)
            .expect("process handle should exist");
        assert_eq!(process.agent_id, id);
        assert_eq!(process.agent_type, AgentType::Energy);
        assert_eq!(process.name, "e1");
        assert_eq!(process.state, ProcessState::Running);
        assert!(process.is_alive());
        assert!(process.has_executable_segment());
        assert_eq!(process.entry_point, 0x1000);
    }

    #[test]
    fn test_spawn_returns_agent_id() {
        let (spawner, reg, _lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        let id = spawner.spawn(config, 1000).unwrap();
        assert!(reg.borrow().exists(id));
    }

    #[test]
    fn test_spawn_blocking_equivalent_to_spawn() {
        let (spawner, _reg, lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        let result = spawner.spawn_blocking(config, 1000);
        assert!(result.is_ok());
        let id = result.unwrap();
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_no_elf_registered_goes_to_error() {
        let (spawner, reg, lifecycle) = make_spawner(&FAIL_REGISTRY);
        let config = make_config(AgentType::Energy, "e1");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert!(matches!(err, AgentError::CodeLoadFailed(_)));
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
        // No process handle created.
        assert_eq!(spawner.process_count(), 0);
    }

    #[test]
    fn test_spawn_priority_override_applied() {
        let (spawner, reg, _lifecycle) = make_spawner(signed_registry());
        let mut config = make_config(AgentType::Device, "d1");
        config.priority_override = Some(200);
        let id = spawner.spawn(config, 1000).unwrap();
        let priority = reg.borrow().get(id).unwrap().priority;
        assert_eq!(priority, 200);

        // Also check the ProcessHandle budget.
        let process = spawner.get_process(id).unwrap();
        assert_eq!(process.budget.priority, 200);
    }

    #[test]
    fn test_spawn_mem_override_applied() {
        let (spawner, reg, _lifecycle) = make_spawner(signed_registry());
        let mut config = make_config(AgentType::Energy, "e1");
        config.mem_override = Some(1024);
        let id = spawner.spawn(config, 1000).unwrap();
        let mem_quota = reg.borrow().get(id).unwrap().mem_quota;
        assert_eq!(mem_quota, 1024);

        // ProcessHandle budget should reflect the override.
        let process = spawner.get_process(id).unwrap();
        assert_eq!(process.budget.max_memory_bytes, 1024);
    }

    #[test]
    fn test_spawn_multiple_agents_independent() {
        let (spawner, reg, lifecycle) = make_spawner(signed_registry());
        let id1 = spawner
            .spawn(make_config(AgentType::Energy, "e1"), 1000)
            .unwrap();
        let id2 = spawner
            .spawn(make_config(AgentType::Market, "m1"), 1000)
            .unwrap();
        let id3 = spawner
            .spawn(make_config(AgentType::Grid, "g1"), 1000)
            .unwrap();
        assert_eq!(reg.borrow().count(), 3);
        assert_eq!(spawner.process_count(), 3);
        assert_eq!(
            lifecycle.borrow().current_state(id1),
            Ok(AgentState::Running)
        );
        assert_eq!(
            lifecycle.borrow().current_state(id2),
            Ok(AgentState::Running)
        );
        assert_eq!(
            lifecycle.borrow().current_state(id3),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_registers_in_registry() {
        let (spawner, reg, _lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        spawner.spawn(config, 1000).unwrap();
        assert_eq!(reg.borrow().count(), 1);
    }

    #[test]
    fn test_spawn_created_to_ready_to_running() {
        let (spawner, _reg, lifecycle) = make_spawner(signed_registry());
        let config = make_config(AgentType::Energy, "e1");
        let id = spawner.spawn(config, 1000).unwrap();
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_memory_budget_exceeded() {
        // Set mem_override to 1 byte — the ELF (8 bytes of code + headers) exceeds this.
        let (spawner, reg, lifecycle) = make_spawner(signed_registry());
        let mut config = make_config(AgentType::Energy, "e1");
        config.mem_override = Some(1);
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert!(matches!(err, AgentError::CodeLoadFailed(_)));
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
        assert_eq!(spawner.process_count(), 0);
    }

    #[test]
    fn test_get_process_returns_none_for_unknown_id() {
        let (spawner, _reg, _lifecycle) = make_spawner(signed_registry());
        let fake_id = AgentId::generate();
        assert!(spawner.get_process(fake_id).is_none());
    }

    #[test]
    fn test_static_registry_get_binary() {
        let binary = SUCCESS_REGISTRY.get_binary(AgentType::Energy);
        assert!(binary.is_some());
        assert_eq!(binary.unwrap().elf.len(), 128);

        let none = FAIL_REGISTRY.get_binary(AgentType::Energy);
        assert!(none.is_none());
    }

    /// SEC-002：无签名条目（manifest: None）spawn 必须拒绝加载（fail-closed）。
    #[test]
    fn test_spawn_unsigned_rejected() {
        let (spawner, reg, lifecycle) = make_spawner(&SUCCESS_REGISTRY);
        let config = make_config(AgentType::Energy, "unsigned");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert!(
            matches!(err, AgentError::CodeLoadFailed(_)),
            "unsigned binary must be rejected, got {err:?}"
        );
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
        assert_eq!(spawner.process_count(), 0);
    }

    /// SEC-002：错签（篡改签名清单的 signature 分量）spawn 必须拒绝。
    #[test]
    fn test_spawn_wrong_signature_rejected() {
        use std::sync::OnceLock;
        static BAD: OnceLock<StaticAgentRegistry> = OnceLock::new();
        let bad_reg = BAD.get_or_init(|| {
            let mut binary = signed_binary(AgentType::Energy);
            if let Some(m) = binary.manifest.as_mut() {
                m.signature[0] ^= 0x01; // 篡改 r 分量
            }
            let entries: &'static [(AgentType, AgentBinary)] =
                Box::leak(Box::new([(AgentType::Energy, binary)]));
            StaticAgentRegistry::new(entries)
        });
        let (spawner, reg, lifecycle) = make_spawner(bad_reg);
        let config = make_config(AgentType::Energy, "wrong-sig");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert!(
            matches!(err, AgentError::CodeLoadFailed(_)),
            "wrong signature must be rejected, got {err:?}"
        );
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
        assert_eq!(spawner.process_count(), 0);
    }
}
