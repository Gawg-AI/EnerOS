//! 能力令牌（Capability Token）集成测试 (v0.39.0).
//!
//! 端到端验证能力令牌的构建、签名、验证、篡改检测、权限检查和约束检查。

use eneros_agent::capability::{
    CapabilityToken, CapabilityTokenBuilder, ConstraintPack, ConstraintType, DeviceId,
    PermissionSet, ResourceTarget, SocketAddr, SystemResource, TokenVerifier,
};
use eneros_agent::{AgentError, AgentId};
use eneros_crypto::{CsRng, Sm2KeyPair};

/// 生成测试用密钥对和 RNG.
///
/// 注意：`CsRng::new()` 使用固定种子，返回的 `rng` 状态已前进至密钥对生成之后，
/// 调用方应继续使用同一 `rng` 实例以获取不同的随机值。
fn setup() -> (Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("keypair generation failed");
    (kp, rng)
}

/// 构建一个标准测试令牌.
fn build_test_token(kp: &Sm2KeyPair, rng: &mut CsRng) -> CapabilityToken {
    CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::Agent(AgentId(200)))
        .permission(PermissionSet::READ | PermissionSet::WRITE)
        .constraints(ConstraintPack {
            max_power: 100.0,
            min_power: 10.0,
            soc_limit: (20.0, 80.0),
            voltage_limit: (200.0, 240.0),
            frequency_limit: (49.5, 50.5),
        })
        .ttl(3600000)
        .build_and_sign(kp, AgentId(1), 1000, rng)
        .expect("token build failed")
}

#[test]
fn test_end_to_end_build_and_verify() {
    let (kp, mut rng) = setup();
    let token = build_test_token(&kp, &mut rng);
    let result = token.verify(&kp.public_key);
    assert!(result.is_ok(), "verify should succeed, got {:?}", result);
}

#[test]
fn test_end_to_end_tamper_detect() {
    let (kp, mut rng) = setup();
    let mut token = build_test_token(&kp, &mut rng);
    // 篡改权限字段
    token.permissions = PermissionSet::ALL;
    let result = token.verify(&kp.public_key);
    assert!(
        matches!(result, Err(AgentError::TokenSignatureInvalid)),
        "tampered permissions should fail verification, got {:?}",
        result
    );
}

#[test]
fn test_token_verifier_end_to_end() {
    let (kp, mut rng) = setup();
    let token = build_test_token(&kp, &mut rng);
    let verifier = TokenVerifier::new(kp.public_key);
    let result = verifier.verify(&token);
    assert!(
        result.is_ok(),
        "verifier.verify should succeed, got {:?}",
        result
    );
    // issuer_pk() 应返回构造时传入的公钥
    assert_eq!(*verifier.issuer_pk(), kp.public_key);
}

#[test]
fn test_permission_set_combinations() {
    // READ | WRITE | EXECUTE
    let p = PermissionSet::READ | PermissionSet::WRITE | PermissionSet::EXECUTE;
    assert!(p.contains(PermissionSet::READ));
    assert!(p.contains(PermissionSet::WRITE));
    assert!(p.contains(PermissionSet::EXECUTE));
    assert!(!p.contains(PermissionSet::CONTROL));

    // ALL 包含所有权限
    assert!(PermissionSet::ALL.contains(PermissionSet::READ));
    assert!(PermissionSet::ALL.contains(PermissionSet::WRITE));
    assert!(PermissionSet::ALL.contains(PermissionSet::EXECUTE));
    assert!(PermissionSet::ALL.contains(PermissionSet::CONTROL));
    assert!(PermissionSet::ALL.contains(PermissionSet::CONFIG));
    assert!(PermissionSet::ALL.contains(PermissionSet::ADMIN));
    assert!(PermissionSet::ALL.is_all());

    // NONE 不包含任何权限
    assert!(!PermissionSet::NONE.contains(PermissionSet::READ));
    assert!(!PermissionSet::NONE.contains(PermissionSet::WRITE));
    assert!(!PermissionSet::NONE.contains(PermissionSet::EXECUTE));
    assert!(!PermissionSet::NONE.contains(PermissionSet::CONTROL));
    assert!(PermissionSet::NONE.is_empty());
}

#[test]
fn test_constraint_pack_power_boundary() {
    let pack = ConstraintPack {
        max_power: 100.0,
        min_power: 0.0,
        soc_limit: (0.0, 100.0),
        voltage_limit: (0.0, 1000.0),
        frequency_limit: (0.0, 100.0),
    };
    // 边界值：99.9 < 100.0 → 满足
    assert!(pack.check_constraint(99.9, ConstraintType::MaxPower));
    // 边界值：100.0 == 100.0 → 满足（<=）
    assert!(pack.check_constraint(100.0, ConstraintType::MaxPower));
    // 边界值：100.1 > 100.0 → 违反
    assert!(!pack.check_constraint(100.1, ConstraintType::MaxPower));
    // clamp 将超限值截断到上限
    assert_eq!(pack.clamp(150.0, ConstraintType::MaxPower), 100.0);
}

#[test]
fn test_expired_token_check() {
    let (kp, mut rng) = setup();
    // ttl(1000) at now=5000 → expires_at = 6000
    let token = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::Agent(AgentId(200)))
        .permission(PermissionSet::READ)
        .ttl(1000)
        .build_and_sign(&kp, AgentId(1), 5000, &mut rng)
        .expect("token build failed");
    assert_eq!(token.expires_at, Some(6000));
    // now=5000 < 6000 → 未过期
    assert!(!token.is_expired(5000));
    // now=6000 >= 6000 → 过期
    assert!(token.is_expired(6000));
    // now=5999 < 6000 → 未过期
    assert!(!token.is_expired(5999));
}

#[test]
fn test_different_owners_independent() {
    let (kp, mut rng) = setup();
    let t1 = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::Agent(AgentId(200)))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("t1 build failed");
    let t2 = CapabilityTokenBuilder::new()
        .owner(AgentId(200))
        .target(ResourceTarget::Agent(AgentId(200)))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("t2 build failed");
    // 两个令牌均验证通过
    assert!(t1.verify(&kp.public_key).is_ok(), "t1 verify failed");
    assert!(t2.verify(&kp.public_key).is_ok(), "t2 verify failed");
    // token_id 不同（RNG 状态前进）
    assert_ne!(t1.token_id, t2.token_id, "token_ids should differ");
    // owner 不同
    assert_ne!(t1.owner, t2.owner);
}

#[test]
fn test_multiple_tokens_batch_verify() {
    let (kp, mut rng) = setup();
    let verifier = TokenVerifier::new(kp.public_key);
    let mut tokens = Vec::new();
    for _ in 0..3 {
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(100))
            .target(ResourceTarget::Agent(AgentId(200)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("token build failed");
        tokens.push(token);
    }
    // 所有令牌验证通过
    for (i, t) in tokens.iter().enumerate() {
        let result = verifier.verify(t);
        assert!(result.is_ok(), "token {} verify failed: {:?}", i, result);
    }
    // 三个 token_id 互不相同
    assert_ne!(tokens[0].token_id, tokens[1].token_id);
    assert_ne!(tokens[1].token_id, tokens[2].token_id);
    assert_ne!(tokens[0].token_id, tokens[2].token_id);
}

#[test]
fn test_resource_target_variants() {
    let (kp, mut rng) = setup();
    let verifier = TokenVerifier::new(kp.public_key);

    // Device 变体
    let t_device = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::Device(DeviceId(42)))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("device token build failed");
    assert!(
        verifier.verify(&t_device).is_ok(),
        "device token verify failed"
    );

    // File 变体
    let t_file = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::File(String::from("/etc/config.toml")))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("file token build failed");
    assert!(verifier.verify(&t_file).is_ok(), "file token verify failed");

    // Network 变体
    let t_network = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::Network(SocketAddr {
            ipv4: 0xC0A80101,
            port: 8080,
        }))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("network token build failed");
    assert!(
        verifier.verify(&t_network).is_ok(),
        "network token verify failed"
    );

    // SystemResource 变体
    let t_sysres = CapabilityTokenBuilder::new()
        .owner(AgentId(100))
        .target(ResourceTarget::SystemResource(SystemResource::Cpu))
        .permission(PermissionSet::READ)
        .ttl(3600000)
        .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
        .expect("system resource token build failed");
    assert!(
        verifier.verify(&t_sysres).is_ok(),
        "system resource token verify failed"
    );
}

#[test]
fn test_unsigned_token_verify_fails() {
    // 手动构造一个未签名令牌（signature 全零，不通过 builder）
    let token = CapabilityToken {
        token_id: 12345,
        owner: AgentId(100),
        target: ResourceTarget::Agent(AgentId(200)),
        permissions: PermissionSet::READ | PermissionSet::WRITE,
        constraints: ConstraintPack {
            max_power: 100.0,
            min_power: 10.0,
            soc_limit: (20.0, 80.0),
            voltage_limit: (200.0, 240.0),
            frequency_limit: (49.5, 50.5),
        },
        issued_at: 1000,
        expires_at: Some(2000),
        issuer: AgentId(1),
        signature: [0u8; 64],
    };
    // 任意公钥均可触发 TokenNotSigned 检查（签名全零先于验签）
    let (kp, _rng) = setup();
    let result = token.verify(&kp.public_key);
    assert!(
        matches!(result, Err(AgentError::TokenNotSigned)),
        "unsigned token should fail with TokenNotSigned, got {:?}",
        result
    );
}
