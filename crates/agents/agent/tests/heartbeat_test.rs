//! Integration tests for v0.37.0 HeartbeatMonitor / HealthStatus / HealthCheck.
//!
//! 验证心跳监控的端到端行为（蓝图 §9.5）：
//! - 完整生命周期：Healthy → Degraded → Unhealthy
//! - 心跳恢复后状态重置
//! - 多 Agent 独立监控
//! - per-Agent 间隔覆盖
//! - 注销停止监控
//! - HealthCheck trait 对象安全
//! - HealthStatus 全 variant 可构造/比较/Debug
//! - 时钟回拨安全（saturating_sub）

use eneros_agent::{AgentId, HealthCheck, HealthStatus, HeartbeatMonitor};

/// 辅助：从 check() 结果中提取指定 Agent 的状态.
fn status_of(results: &[(AgentId, HealthStatus)], id: AgentId) -> Option<HealthStatus> {
    results.iter().find(|(i, _)| *i == id).map(|(_, s)| *s)
}

#[test]
fn integration_heartbeat_full_lifecycle() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id = AgentId::generate();
    m.register(id, 1000);
    m.heartbeat(id, 1000);

    // t=1500: elapsed=500 <= 1000 → Healthy
    let r = m.check(1500);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Healthy));

    // t=3500: elapsed=2500 > 1000 → missed=2 → Degraded
    let r = m.check(3500);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Degraded));

    // t=4500: elapsed=3500 → missed=3 >= 3 → Unhealthy
    let r = m.check(4500);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Unhealthy));
}

#[test]
fn integration_heartbeat_recovery() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id = AgentId::generate();
    m.register(id, 1000);

    // t=3500: elapsed=2500 → missed=2 → Degraded
    let r = m.check(3500);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Degraded));

    // 发送心跳恢复
    m.heartbeat(id, 3600);

    // t=3600: elapsed=0 → Healthy
    let r = m.check(3600);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Healthy));
}

#[test]
fn integration_multiple_agents_independent() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id1 = AgentId::generate();
    let id2 = AgentId::generate();
    let id3 = AgentId::generate();
    m.register(id1, 1000);
    m.register(id2, 1000);
    m.register(id3, 1000);

    // id1 持续心跳，id2/id3 不心跳
    m.heartbeat(id1, 3500);
    let r = m.check(3500);
    assert_eq!(status_of(&r, id1), Some(HealthStatus::Healthy));
    assert_eq!(status_of(&r, id2), Some(HealthStatus::Degraded));
    assert_eq!(status_of(&r, id3), Some(HealthStatus::Degraded));

    // 继续推进：id1 仍心跳，id2/id3 恶化
    m.heartbeat(id1, 4500);
    let r = m.check(4500);
    assert_eq!(status_of(&r, id1), Some(HealthStatus::Healthy));
    assert_eq!(status_of(&r, id2), Some(HealthStatus::Unhealthy));
    assert_eq!(status_of(&r, id3), Some(HealthStatus::Unhealthy));
}

#[test]
fn integration_set_interval_affects_timing() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id_a = AgentId::generate();
    let id_b = AgentId::generate();
    m.register(id_a, 1000);
    m.register(id_b, 1000);
    // idB 设置更短的间隔
    m.set_interval(id_b, 500);

    // 两者在 t=1000 后均不再心跳
    let r = m.check(1750);
    // idA: elapsed=750 <= 1000 → Healthy
    assert_eq!(status_of(&r, id_a), Some(HealthStatus::Healthy));
    // idB: elapsed=750 > 500 → missed=1 → Degraded
    assert_eq!(status_of(&r, id_b), Some(HealthStatus::Degraded));
}

#[test]
fn integration_unregister_stops_monitoring() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id = AgentId::generate();
    m.register(id, 1000);
    m.unregister(id);

    let r = m.check(5000);
    assert!(r.iter().all(|(i, _)| *i != id));
}

#[test]
fn integration_health_check_trait() {
    struct FixedChecker {
        status: HealthStatus,
    }

    impl HealthCheck for FixedChecker {
        fn check_health(&self) -> HealthStatus {
            self.status
        }
    }

    let variants = [
        HealthStatus::Healthy,
        HealthStatus::Degraded,
        HealthStatus::Unhealthy,
        HealthStatus::Dead,
    ];

    for status in variants {
        let checker: Box<dyn HealthCheck> = Box::new(FixedChecker { status });
        assert_eq!(checker.check_health(), status);
    }
}

#[test]
fn integration_health_status_all_variants() {
    let healthy = HealthStatus::Healthy;
    let degraded = HealthStatus::Degraded;
    let unhealthy = HealthStatus::Unhealthy;
    let dead = HealthStatus::Dead;

    // 可比较
    assert_eq!(healthy, HealthStatus::Healthy);
    assert_ne!(healthy, degraded);
    assert_ne!(degraded, unhealthy);
    assert_ne!(unhealthy, dead);

    // 可 Debug 打印
    assert!(format!("{:?}", healthy).contains("Healthy"));
    assert!(format!("{:?}", degraded).contains("Degraded"));
    assert!(format!("{:?}", unhealthy).contains("Unhealthy"));
    assert!(format!("{:?}", dead).contains("Dead"));

    // Copy 语义
    let copy_test = healthy;
    assert_eq!(healthy, copy_test);
}

#[test]
fn integration_clock_rollback_safe() {
    let mut m = HeartbeatMonitor::new(1000, 3);
    let id = AgentId::generate();
    m.register(id, 5000);

    // 时钟回拨：elapsed = 1000.saturating_sub(5000) = 0，0 <= 1000 → 保持 Healthy
    let r = m.check(1000);
    assert_eq!(status_of(&r, id), Some(HealthStatus::Healthy));
}
