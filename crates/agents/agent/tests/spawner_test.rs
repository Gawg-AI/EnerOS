//! Integration tests for AGT-003 real ELF spawn (ProcessHandle-based).
//!
//! Verifies the end-to-end spawn flow:
//! - Real ELF parse + ProcessHandle creation (not Box<dyn AgentEntry>)
//! - Created -> Ready -> Running lifecycle on success
//! - Error state on missing ELF / budget exceeded
//! - spawn_blocking equivalent to spawn
//! - Multiple agents independent
//! - priority_override / mem_override applied to both registry and ProcessHandle

use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;

use eneros_agent::{
    AgentBinary, AgentConfig, AgentError, AgentRegistry, AgentSpawner, AgentState, AgentType,
    LifecycleManager, ProcessState, StaticAgentRegistry,
};

// ---- Minimal valid ELF64 PIE AArch64 binary (128 bytes) ----

const fn build_minimal_test_elf() -> [u8; 128] {
    let mut buf = [0u8; 128];
    buf[0] = 0x7f;
    buf[1] = 0x45;
    buf[2] = 0x4c;
    buf[3] = 0x46;
    buf[4] = 2;
    buf[5] = 1;
    buf[6] = 1;
    buf[16] = 3;
    buf[17] = 0; // ET_DYN
    buf[18] = 0xB7;
    buf[19] = 0; // EM_AARCH64
    buf[24] = 0x00;
    buf[25] = 0x10; // e_entry = 0x1000
    buf[32] = 64; // e_phoff = 64
    buf[54] = 56; // e_phentsize
    buf[56] = 1; // e_phnum = 1
    let ph = 64;
    buf[ph] = 1; // PT_LOAD
    buf[ph + 4] = 5; // PF_R | PF_X
    buf[ph + 8] = 120; // p_offset
    buf[ph + 16] = 0x00;
    buf[ph + 17] = 0x10; // p_vaddr = 0x1000
    buf[ph + 32] = 8; // p_filesz
    buf[ph + 40] = 8; // p_memsz
    buf[ph + 48] = 0x00;
    buf[ph + 49] = 0x10; // p_align = 0x1000
    buf[120] = 0x1f;
    buf[121] = 0x20;
    buf[122] = 0x03;
    buf[123] = 0xd5; // NOP
    buf[124] = 0xc0;
    buf[125] = 0x03;
    buf[126] = 0x5f;
    buf[127] = 0xd6; // RET
    buf
}

static MINIMAL_ELF: &[u8] = &build_minimal_test_elf();

#[allow(dead_code)]
static SUCCESS_REGISTRY: StaticAgentRegistry = StaticAgentRegistry::new(&[
    (
        AgentType::Energy,
        AgentBinary {
            elf: MINIMAL_ELF,
            manifest: None,
        },
    ),
    (
        AgentType::Device,
        AgentBinary {
            elf: MINIMAL_ELF,
            manifest: None,
        },
    ),
    (
        AgentType::Market,
        AgentBinary {
            elf: MINIMAL_ELF,
            manifest: None,
        },
    ),
    (
        AgentType::Grid,
        AgentBinary {
            elf: MINIMAL_ELF,
            manifest: None,
        },
    ),
]);

static FAIL_REGISTRY: StaticAgentRegistry = StaticAgentRegistry::new(&[]);

/// SEC-002 集成测试辅助：固定私钥（d=1）确定性 SM2 签名清单。
fn signed_manifest(agent_type: AgentType) -> eneros_agent::AgentPackageManifest {
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
    let hash = eneros_crypto::sm3_hash(MINIMAL_ELF);

    let mut d = [0u8; 32];
    d[31] = 1;
    let sk = Sm2PrivateKey::from_bytes(&d).expect("fixed sk");
    let keypair = Sm2KeyPair::from_private_key(&sk).expect("keypair");

    let mut msg = Vec::new();
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

    eneros_agent::AgentPackageManifest {
        name: String::from(name),
        version: String::from(version),
        agent_type,
        budget: Default::default(),
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

// ---- Helpers ----

/// 测试信任锚 = d=1 测试签名钥对应公钥（与 `signed_manifest` 同钥；
/// 蓝图 66 WP-66.1.2 锚定装配）。
fn test_anchor() -> [u8; 65] {
    use eneros_crypto::sm2::{Sm2KeyPair, Sm2PrivateKey};
    let mut d = [0u8; 32];
    d[31] = 1;
    let sk = Sm2PrivateKey::from_bytes(&d).expect("fixed sk");
    Sm2KeyPair::from_private_key(&sk)
        .expect("keypair")
        .public_key
        .to_bytes_uncompressed()
}

/// SIG13 负例锚 = d=2 钥公钥（≠ 测试签名钥）。
fn other_anchor() -> [u8; 65] {
    use eneros_crypto::sm2::{Sm2KeyPair, Sm2PrivateKey};
    let mut d = [0u8; 32];
    d[31] = 2;
    let sk = Sm2PrivateKey::from_bytes(&d).expect("fixed sk");
    Sm2KeyPair::from_private_key(&sk)
        .expect("keypair")
        .public_key
        .to_bytes_uncompressed()
}

fn make_spawner(
    agents: &'static StaticAgentRegistry,
) -> (
    AgentSpawner,
    Rc<RefCell<AgentRegistry>>,
    Rc<RefCell<LifecycleManager>>,
) {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let spawner = AgentSpawner::new(reg.clone(), lifecycle.clone(), agents, &test_anchor())
        .expect("valid test anchor");
    (spawner, reg, lifecycle)
}

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

// ---- Integration tests ----

#[test]
fn integration_spawn_full_success_path() {
    let (spawner, _reg, lifecycle) = make_spawner(signed_registry());
    let config = make_config(AgentType::Energy, "e1");
    let id = spawner.spawn(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
    let process = spawner.get_process(id).expect("process handle");
    assert_eq!(process.state, ProcessState::Running);
    assert!(process.is_alive());
}

#[test]
fn integration_spawn_no_elf_error_state() {
    let (spawner, reg, lifecycle) = make_spawner(&FAIL_REGISTRY);
    let config = make_config(AgentType::Energy, "e1");
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert!(matches!(err, AgentError::CodeLoadFailed(_)));
    assert_eq!(reg.borrow().count(), 1);
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
    assert_eq!(spawner.process_count(), 0);
}

#[test]
fn integration_spawn_memory_budget_exceeded() {
    let (spawner, reg, lifecycle) = make_spawner(signed_registry());
    let mut config = make_config(AgentType::Energy, "e1");
    config.mem_override = Some(1);
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert!(matches!(err, AgentError::CodeLoadFailed(_)));
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
}

#[test]
fn integration_spawn_blocking_same_as_spawn() {
    let (spawner, _reg, lifecycle) = make_spawner(signed_registry());
    let config = make_config(AgentType::Energy, "e1");
    let id = spawner.spawn_blocking(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
}

#[test]
fn integration_spawn_multiple_agents() {
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
    for id in [id1, id2, id3] {
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert!(spawner.get_process(id).is_some());
    }
}

#[test]
fn integration_spawn_with_overrides() {
    let (spawner, reg, _lifecycle) = make_spawner(signed_registry());
    let mut config = make_config(AgentType::Device, "d1");
    config.priority_override = Some(200);
    config.mem_override = Some(1024);
    let id = spawner.spawn(config, 1000).unwrap();
    assert_eq!(reg.borrow().get(id).unwrap().priority, 200);
    assert_eq!(reg.borrow().get(id).unwrap().mem_quota, 1024);
    let process = spawner.get_process(id).unwrap();
    assert_eq!(process.budget.priority, 200);
    assert_eq!(process.budget.max_memory_bytes, 1024);
}

/// SIG13（蓝图 66 WP-66.1.2，体检 C3）：锚定准入——包签名钥 ≠ 注入锚时，
/// 即使签名本身自洽（旧 verify() 会放行的自证形态）也必须拒绝，错误词形
/// `agent pubkey not pinned`。
#[test]
fn sig13_spawn_rejects_non_pinned_signer() {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let spawner = AgentSpawner::new(
        reg.clone(),
        lifecycle.clone(),
        signed_registry(),
        &other_anchor(),
    )
    .expect("valid non-anchor test key");
    let config = make_config(AgentType::Energy, "e1");
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert!(
        matches!(err, AgentError::CodeLoadFailed(ref m) if m.contains("agent pubkey not pinned")),
        "SIG13：非锚钥自签必须被拒且词形含 agent pubkey not pinned，得：{err:?}"
    );
    // 拒绝路径落 Error 终态（与既有失败语义一致）
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
}

/// 无效锚字节（非 65B 压缩点格式）= 构造失败（fail-closed，蓝图 66：
/// 禁止 None 兜底回退自证）。
#[test]
fn spawner_construction_fails_on_invalid_anchor() {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let bad_anchor = [0u8; 65]; // 全零非法点
    let err = AgentSpawner::new(reg, lifecycle, &FAIL_REGISTRY, &bad_anchor).err();
    assert!(
        matches!(err, Some(AgentError::CodeLoadFailed(ref m)) if m.contains("invalid trust anchor")),
        "无效锚必须使构造失败且词形匹配（fail-closed），得：{err:?}"
    );
}
