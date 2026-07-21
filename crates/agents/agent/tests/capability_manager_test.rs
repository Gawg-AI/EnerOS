//! 能力管理器集成测试 (v0.40.0).
//!
//! 测试 CapabilityManager 的端到端行为：签发、校验、冻结、解冻、撤销、过期清理。

use eneros_agent::capability::{
    CapabilityManager, CapabilityTokenBuilder, PermissionSet, ResourceTarget,
};
use eneros_agent::{AgentError, AgentId};
use eneros_crypto::{CsRng, Sm2KeyPair};

/// 生成测试用密钥对与 RNG.
fn make_keypair() -> (Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
    (kp, rng)
}

/// 构造测试用 builder.
fn make_builder(
    owner: AgentId,
    target: ResourceTarget,
    perm: PermissionSet,
    ttl: u64,
) -> CapabilityTokenBuilder {
    CapabilityTokenBuilder::new()
        .owner(owner)
        .target(target)
        .permission(perm)
        .ttl(ttl)
}

#[test]
fn test_manager_issue_and_check_access_end_to_end() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    // Agent 2 请求对 Agent 3 的 READ|WRITE 权限
    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ | PermissionSet::WRITE,
        3600000,
    );
    let token = manager.issue(builder, 1000).expect("issue failed");

    // 验证令牌已签名
    assert_ne!(token.signature, [0u8; 64]);
    // 验证令牌在存储中
    assert_eq!(manager.store().len(), 1);
    // 验证 check_access 通过
    let result = manager.check_access(
        AgentId(2),
        &ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        2000,
    );
    assert!(
        result.is_ok(),
        "check_access should succeed, got {:?}",
        result
    );
    // 验证返回的令牌 ID 匹配
    assert_eq!(result.unwrap().token_id, token.token_id);
}

#[test]
fn test_manager_check_access_wrong_target_rejected() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    // Agent 2 有 target=Agent(3) 的令牌
    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        3600000,
    );
    manager.issue(builder, 1000).expect("issue failed");

    // 请求 target=Agent(4) 应被拒绝（D9 验证）
    let result = manager.check_access(
        AgentId(2),
        &ResourceTarget::Agent(AgentId(4)),
        PermissionSet::READ,
        2000,
    );
    assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    if let Err(AgentError::NoCapability { agent, target }) = result {
        assert_eq!(agent, AgentId(2));
        assert!(
            target.contains("AgentId(4)"),
            "target should contain AgentId(4), got: {}",
            target
        );
    }
}

#[test]
fn test_manager_freeze_blocks_check_access() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        3600000,
    );
    let token = manager.issue(builder, 1000).expect("issue failed");

    // 冻结前 check_access 通过
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_ok());

    // 冻结 Agent 2
    let count = manager.freeze(AgentId(2));
    assert_eq!(count, 1);
    assert!(manager.is_frozen(token.token_id));

    // 冻结后 check_access 被拒绝
    let result = manager.check_access(
        AgentId(2),
        &ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        2000,
    );
    assert!(matches!(result, Err(AgentError::NoCapability { .. })));
}

#[test]
fn test_manager_unfreeze_restores_access() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        3600000,
    );
    let token = manager.issue(builder, 1000).expect("issue failed");

    // 冻结
    manager.freeze(AgentId(2));
    assert!(manager.is_frozen(token.token_id));

    // 冻结后 check_access 被拒绝
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_err());

    // 解冻
    assert!(manager.unfreeze(token.token_id));
    assert!(!manager.is_frozen(token.token_id));

    // 解冻后 check_access 恢复
    let result = manager.check_access(
        AgentId(2),
        &ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        2000,
    );
    assert!(
        result.is_ok(),
        "check_access should succeed after unfreeze, got {:?}",
        result
    );
}

#[test]
fn test_manager_revoke_removes_token() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        3600000,
    );
    let token = manager.issue(builder, 1000).expect("issue failed");
    assert_eq!(manager.store().len(), 1);

    // 撤销令牌
    assert!(manager.revoke(token.token_id));
    assert_eq!(manager.store().len(), 0);
    assert!(manager.is_revoked(token.token_id));

    // 撤销后 check_access 被拒绝
    let result = manager.check_access(
        AgentId(2),
        &ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        2000,
    );
    assert!(matches!(result, Err(AgentError::NoCapability { .. })));

    // 撤销不存在的令牌返回 false
    assert!(!manager.revoke(999));
}

#[test]
fn test_manager_cleanup_expired_removes_expired() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    // token 1: ttl=1000, issued_at=1000, expires_at=2000（now=3000 过期）
    manager
        .issue(
            make_builder(
                AgentId(2),
                ResourceTarget::Agent(AgentId(3)),
                PermissionSet::READ,
                1000,
            ),
            1000,
        )
        .expect("issue 1 failed");
    // token 2: ttl=5000, issued_at=1000, expires_at=6000（now=3000 未过期）
    manager
        .issue(
            make_builder(
                AgentId(4),
                ResourceTarget::Agent(AgentId(5)),
                PermissionSet::WRITE,
                5000,
            ),
            1000,
        )
        .expect("issue 2 failed");

    assert_eq!(manager.store().len(), 2);

    // 清理过期令牌
    let cleaned = manager.cleanup_expired(3000);
    assert_eq!(cleaned, 1);
    assert_eq!(manager.store().len(), 1);

    // 验证未过期令牌仍可访问
    let result = manager.check_access(
        AgentId(4),
        &ResourceTarget::Agent(AgentId(5)),
        PermissionSet::WRITE,
        3000,
    );
    assert!(result.is_ok());
}

#[test]
fn test_manager_multiple_agents_isolation() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    // Agent 2 有对 Agent 3 的 READ 权限
    manager
        .issue(
            make_builder(
                AgentId(2),
                ResourceTarget::Agent(AgentId(3)),
                PermissionSet::READ,
                3600000,
            ),
            1000,
        )
        .expect("issue 1 failed");

    // Agent 4 有对 Agent 5 的 WRITE 权限
    manager
        .issue(
            make_builder(
                AgentId(4),
                ResourceTarget::Agent(AgentId(5)),
                PermissionSet::WRITE,
                3600000,
            ),
            1000,
        )
        .expect("issue 2 failed");

    // Agent 2 可以 READ Agent 3
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_ok());

    // Agent 2 不能 WRITE Agent 3（无 WRITE 权限）
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::WRITE,
            2000
        )
        .is_err());

    // Agent 4 不能 READ Agent 3（无令牌）
    assert!(manager
        .check_access(
            AgentId(4),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_err());

    // Agent 4 可以 WRITE Agent 5
    assert!(manager
        .check_access(
            AgentId(4),
            &ResourceTarget::Agent(AgentId(5)),
            PermissionSet::WRITE,
            2000
        )
        .is_ok());

    // 冻结 Agent 2 不影响 Agent 4
    manager.freeze(AgentId(2));
    assert!(manager
        .check_access(
            AgentId(4),
            &ResourceTarget::Agent(AgentId(5)),
            PermissionSet::WRITE,
            2000
        )
        .is_ok());
}

#[test]
fn test_manager_multiple_tokens_same_agent() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    // Agent 2 持有 2 个令牌：不同 target + 不同权限
    manager
        .issue(
            make_builder(
                AgentId(2),
                ResourceTarget::Agent(AgentId(3)),
                PermissionSet::READ,
                3600000,
            ),
            1000,
        )
        .expect("issue 1 failed");
    manager
        .issue(
            make_builder(
                AgentId(2),
                ResourceTarget::Agent(AgentId(4)),
                PermissionSet::WRITE,
                3600000,
            ),
            1000,
        )
        .expect("issue 2 failed");

    assert_eq!(manager.store().len(), 2);

    // Agent 2 可以 READ Agent 3
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_ok());

    // Agent 2 可以 WRITE Agent 4
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(4)),
            PermissionSet::WRITE,
            2000
        )
        .is_ok());

    // Agent 2 不能 WRITE Agent 3（只有 READ）
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::WRITE,
            2000
        )
        .is_err());

    // 冻结 Agent 2 的所有令牌
    let count = manager.freeze(AgentId(2));
    assert_eq!(count, 2);

    // 冻结后两个 target 都无法访问
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000
        )
        .is_err());
    assert!(manager
        .check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(4)),
            PermissionSet::WRITE,
            2000
        )
        .is_err());
}

#[test]
fn test_manager_verify_token_signature() {
    let (kp, _) = make_keypair();
    let mut manager = CapabilityManager::new(kp, AgentId(1));

    let builder = make_builder(
        AgentId(2),
        ResourceTarget::Agent(AgentId(3)),
        PermissionSet::READ,
        3600000,
    );
    let token = manager.issue(builder, 1000).expect("issue failed");

    // 验证签名有效
    assert!(manager.verify_token(&token).is_ok());

    // 篡改令牌后验证失败
    let mut tampered = token.clone();
    tampered.permissions = PermissionSet::ALL;
    assert!(manager.verify_token(&tampered).is_err());
}

#[test]
fn test_manager_no_capability_error() {
    let (kp, _) = make_keypair();
    let manager = CapabilityManager::new(kp, AgentId(1));

    // Agent 99 无任何令牌
    let result = manager.check_access(
        AgentId(99),
        &ResourceTarget::Agent(AgentId(42)),
        PermissionSet::READ,
        2000,
    );
    assert!(
        matches!(result, Err(AgentError::NoCapability { .. })),
        "expected NoCapability, got {:?}",
        result
    );
    if let Err(AgentError::NoCapability { agent, target }) = result {
        assert_eq!(agent, AgentId(99));
        assert!(
            target.contains("AgentId(42)"),
            "target should contain AgentId(42), got: {}",
            target
        );
    }
}
