//! Integration tests for SystemAgent with real ELF spawn (AGT-003).
//!
//! Verifies SystemAgent end-to-end behavior with the ProcessHandle-based spawner:
//! - start/stop/suspend/resume lifecycle orchestration
//! - tick heartbeat crash recovery (Unhealthy -> AgentRecovered)
//! - tick OOM victim suspension (lowest priority alive agent)
//! - tick overheat event
//! - get_system_stats (6 fields)
//! - find_oom_victim victim selection
//! - multiple ticks on healthy system produce no events
//! - start/stop heartbeat registration/unregistration

use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;

use eneros_agent::{
    AgentBinary, AgentConfig, AgentRegistry, AgentSpawner, AgentState, AgentType, CheckpointStore,
    CrashRecovery, HeartbeatMonitor, InMemoryCheckpointStore, LifecycleManager,
    StaticAgentRegistry, SystemAgent, SystemConfig, SystemEvent,
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
static TEST_REGISTRY: StaticAgentRegistry = StaticAgentRegistry::new(&[(
    AgentType::Energy,
    AgentBinary {
        elf: MINIMAL_ELF,
        manifest: None,
    },
)]);

/// SEC-002：带真实 SM2 签名的 registry（成功路径测试用）。
fn signed_manifest() -> eneros_agent::AgentPackageManifest {
    use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair, Sm2PrivateKey};
    use eneros_crypto::CsRng;

    let name = "energy";
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
        agent_type: AgentType::Energy,
        budget: Default::default(),
        elf_hash: hash,
        signature: sig_bytes,
        signer_pubkey: pk_bytes,
    }
}

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

fn signed_registry() -> &'static StaticAgentRegistry {
    use std::sync::OnceLock;
    static REGS: OnceLock<StaticAgentRegistry> = OnceLock::new();
    REGS.get_or_init(|| {
        let entries: &'static [(AgentType, AgentBinary)] = Box::leak(Box::new([(
            AgentType::Energy,
            AgentBinary {
                elf: MINIMAL_ELF,
                manifest: Some(signed_manifest()),
            },
        )]));
        StaticAgentRegistry::new(entries)
    })
}

// ---- Helpers ----

fn make_config(name: &str) -> AgentConfig {
    AgentConfig {
        agent_type: AgentType::Energy,
        name: String::from(name),
        binary_path: None,
        config_path: None,
        priority_override: None,
        mem_override: None,
    }
}

fn make_config_with_priority(name: &str, priority: u8) -> AgentConfig {
    AgentConfig {
        agent_type: AgentType::Energy,
        name: String::from(name),
        binary_path: None,
        config_path: None,
        priority_override: Some(priority),
        mem_override: None,
    }
}

#[allow(clippy::type_complexity)]
fn make_system_agent() -> (
    SystemAgent,
    Rc<RefCell<AgentRegistry>>,
    Rc<RefCell<LifecycleManager>>,
    Rc<RefCell<HeartbeatMonitor>>,
) {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
    let recovery = Rc::new(CrashRecovery::with_defaults(
        reg.clone(),
        heartbeat.clone(),
        lifecycle.clone(),
        checkpoint_store,
    ));
    let spawner = Rc::new(
        AgentSpawner::new(
            reg.clone(),
            lifecycle.clone(),
            signed_registry(),
            &test_anchor(),
        )
        .expect("valid test anchor"),
    );
    let sa = SystemAgent::new(
        reg.clone(),
        spawner,
        recovery,
        heartbeat.clone(),
        lifecycle.clone(),
        SystemConfig::default(),
    );
    (sa, reg, lifecycle, heartbeat)
}

// ---- Integration tests ----

#[test]
fn test_system_agent_start_and_stop_agent() {
    let (sa, reg, _lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa
        .start_agent(config, 1000)
        .expect("start_agent should succeed");
    assert!(
        reg.borrow().get(id).is_some(),
        "agent should be registered after start"
    );

    sa.stop_agent(id).expect("stop_agent should succeed");
    assert!(
        reg.borrow().get(id).is_none(),
        "agent should be unregistered after stop"
    );
}

#[test]
fn test_system_agent_suspend_and_resume() {
    let (sa, _reg, lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );

    sa.suspend_agent(id).expect("suspend should succeed");
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Suspended)
    );

    sa.resume_agent(id).expect("resume should succeed");
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
}

#[test]
fn test_system_agent_tick_heartbeat_crash_recovery() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    let events = sa.tick(4001);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::AgentRecovered { agent } if *agent == id)),
        "expected AgentRecovered for agent {:?}, got: {:?}",
        id,
        events
    );
}

#[test]
fn test_system_agent_tick_oom_suspends_lowest_priority() {
    let (mut sa, reg, lifecycle, _heartbeat) = make_system_agent();

    let _high = sa
        .start_agent(make_config_with_priority("high", 200), 1000)
        .unwrap();
    let low = sa
        .start_agent(make_config_with_priority("low", 100), 1000)
        .unwrap();

    sa.monitor.set_values(0.5, 95, 100, 50.0);

    let events = sa.tick(1500);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::OomVictimSuspended { agent } if *agent == low)),
        "expected OomVictimSuspended for low-priority agent, got: {:?}",
        events
    );
    assert_eq!(
        lifecycle.borrow().current_state(low),
        Ok(AgentState::Suspended)
    );
    let high = reg.borrow().list_all()[0].agent_id;
    assert_eq!(
        lifecycle.borrow().current_state(high),
        Ok(AgentState::Running)
    );
}

#[test]
fn test_system_agent_tick_overheat_event() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    sa.monitor.set_values(0.5, 50, 100, 85.0);

    let events = sa.tick(1000);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::Overheat { temp: 85.0 })),
        "expected Overheat event with temp=85.0, got: {:?}",
        events
    );
}

#[test]
fn test_system_agent_get_system_stats() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    sa.start_agent(make_config("agent-1"), 1000).unwrap();
    sa.monitor.set_values(0.5, 50, 100, 65.0);

    let stats = sa.get_system_stats();
    assert_eq!(stats.cpu_usage, 0.5);
    assert_eq!(stats.mem_usage, 0.5);
    assert_eq!(stats.temperature, 65.0);
    assert_eq!(stats.agent_count, 1);
    assert_eq!(stats.alive_agents, 1);
    assert_eq!(stats.error_agents, 0);
}

#[test]
fn test_system_agent_find_oom_victim() {
    let (sa, _reg, _lifecycle, _heartbeat) = make_system_agent();

    let _a1 = sa
        .start_agent(make_config_with_priority("a1", 200), 1000)
        .unwrap();
    let low = sa
        .start_agent(make_config_with_priority("low", 50), 1000)
        .unwrap();
    let _a3 = sa
        .start_agent(make_config_with_priority("a3", 255), 1000)
        .unwrap();

    let victim = sa.find_oom_victim();
    assert_eq!(
        victim,
        Some(low),
        "lowest priority (50) should be OOM victim"
    );
}

#[test]
fn test_system_agent_multiple_ticks() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    sa.start_agent(make_config("agent-1"), 1000).unwrap();

    let events1 = sa.tick(1100);
    assert!(events1.is_empty(), "got: {:?}", events1);

    let events2 = sa.tick(1300);
    assert!(events2.is_empty(), "got: {:?}", events2);

    let events3 = sa.tick(1500);
    assert!(events3.is_empty(), "got: {:?}", events3);
}

#[test]
fn test_system_agent_start_registers_heartbeat() {
    let (mut sa, _reg, _lifecycle, heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should be tracked by heartbeat monitor after start"
    );

    let events = sa.tick(1500);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SystemEvent::AgentCrashed { agent }
            | SystemEvent::AgentRecovered { agent }
            | SystemEvent::AgentRecoveryFailed { agent }
            if *agent == id
        )),
        "got: {:?}",
        events
    );
    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should still be healthy after tick"
    );
}

#[test]
fn test_system_agent_stop_unregisters() {
    let (mut sa, _reg, _lifecycle, heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should be tracked by heartbeat before stop"
    );

    sa.stop_agent(id).unwrap();
    assert!(
        !heartbeat.borrow().is_healthy(id),
        "agent should not be tracked by heartbeat after stop"
    );

    let events = sa.tick(4001);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SystemEvent::AgentCrashed { agent }
            | SystemEvent::AgentRecovered { agent }
            | SystemEvent::AgentRecoveryFailed { agent }
            if *agent == id
        )),
        "got: {:?}",
        events
    );
}
