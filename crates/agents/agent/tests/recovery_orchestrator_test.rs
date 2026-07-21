//! v0.42.0 故障恢复编排集成测试
//!
//! 验证 DependencyGraph + RecoveryOrchestrator 的端到端行为。

use eneros_agent::{AgentError, AgentId, AgentType, DependencyGraph, RecoveryOrchestrator};

#[test]
fn test_dependency_graph_basic() {
    let mut g = DependencyGraph::new();
    g.add_dependency(AgentId(2), AgentId(1));
    g.add_dependency(AgentId(3), AgentId(2));
    let sorted = g.topological_sort().unwrap();
    assert_eq!(sorted.len(), 3);
    let p1 = sorted.iter().position(|&x| x == AgentId(1)).unwrap();
    let p2 = sorted.iter().position(|&x| x == AgentId(2)).unwrap();
    let p3 = sorted.iter().position(|&x| x == AgentId(3)).unwrap();
    assert!(p1 < p2);
    assert!(p2 < p3);
}

#[test]
fn test_dependency_graph_cycle_detection() {
    let mut g = DependencyGraph::new();
    g.add_dependency(AgentId(1), AgentId(2));
    g.add_dependency(AgentId(2), AgentId(1));
    assert!(g.has_cycle());
    let result = g.topological_sort();
    assert!(
        matches!(result, Err(AgentError::CircularDependency)),
        "expected CircularDependency, got {:?}",
        result
    );
}

#[test]
fn test_recovery_orchestrator_single_agent() {
    let mut orch = RecoveryOrchestrator::new();
    orch.schedule_recovery(AgentId(1), AgentType::System);
    assert_eq!(orch.pending_count(), 1);

    let next = orch.process_next();
    assert_eq!(next, Some(AgentId(1)));
    assert!(orch.in_progress().contains(&AgentId(1)));

    orch.on_agent_recovered(AgentId(1));
    assert!(orch.recovered().contains(&AgentId(1)));
    assert!(orch.is_complete());
}

#[test]
fn test_recovery_orchestrator_ordered_recovery() {
    let mut orch = RecoveryOrchestrator::new();
    orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
    orch.schedule_recovery(AgentId(1), AgentType::System);
    orch.schedule_recovery(AgentId(2), AgentType::Device);

    let first = orch.process_next();
    assert_eq!(first, Some(AgentId(1)));

    let second = orch.process_next();
    assert_eq!(second, None);

    orch.on_agent_recovered(AgentId(1));
    let third = orch.process_next();
    assert_eq!(third, Some(AgentId(2)));

    orch.on_agent_recovered(AgentId(2));
    assert!(orch.is_complete());
}

#[test]
fn test_recovery_orchestrator_dependency_blocked() {
    let mut orch = RecoveryOrchestrator::new();
    orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
    orch.schedule_recovery(AgentId(2), AgentType::Device);
    let next = orch.process_next();
    assert_eq!(next, None);
    assert_eq!(orch.pending_count(), 1);
}

#[test]
fn test_recovery_orchestrator_failed_dependency_not_blocked() {
    let mut orch = RecoveryOrchestrator::new();
    orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
    orch.schedule_recovery(AgentId(1), AgentType::System);
    orch.schedule_recovery(AgentId(2), AgentType::Device);

    let first = orch.process_next();
    assert_eq!(first, Some(AgentId(1)));
    orch.on_agent_failed(AgentId(1));

    let second = orch.process_next();
    assert_eq!(second, Some(AgentId(2)));
}

#[test]
fn test_recovery_orchestrator_priority_ordering() {
    let mut orch = RecoveryOrchestrator::new();
    orch.schedule_recovery(AgentId(1), AgentType::Custom(0));
    orch.schedule_recovery(AgentId(2), AgentType::System);
    orch.schedule_recovery(AgentId(3), AgentType::Device);
    orch.schedule_recovery(AgentId(4), AgentType::Market);

    let order: Vec<AgentId> = [
        orch.process_next(),
        orch.process_next(),
        orch.process_next(),
        orch.process_next(),
    ]
    .into_iter()
    .map(|x| x.unwrap())
    .collect();

    assert_eq!(order[0], AgentId(2));
    assert_eq!(order[1], AgentId(3));
    assert_eq!(order[2], AgentId(4));
    assert_eq!(order[3], AgentId(1));
}

#[test]
fn test_recovery_orchestrator_batch_schedule() {
    let mut orch = RecoveryOrchestrator::new();
    let agents = [AgentId(1), AgentId(2), AgentId(3)];
    let types = [AgentType::System, AgentType::Device, AgentType::Market];
    orch.schedule_batch(&agents, &types);
    assert_eq!(orch.pending_count(), 3);

    let mut count = 0;
    while orch.process_next().is_some() {
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn test_recovery_orchestrator_is_complete() {
    let mut orch = RecoveryOrchestrator::new();
    orch.schedule_recovery(AgentId(1), AgentType::System);
    orch.schedule_recovery(AgentId(2), AgentType::Device);
    assert!(!orch.is_complete());

    let a = orch.process_next().unwrap();
    let b = orch.process_next().unwrap();
    assert!(!orch.is_complete());

    orch.on_agent_recovered(a);
    assert!(!orch.is_complete());

    orch.on_agent_recovered(b);
    assert!(orch.is_complete());
}

#[test]
fn test_recovery_orchestrator_pending_count() {
    let mut orch = RecoveryOrchestrator::new();
    orch.schedule_recovery(AgentId(1), AgentType::System);
    orch.schedule_recovery(AgentId(2), AgentType::Device);
    assert_eq!(orch.pending_count(), 2);

    let _ = orch.process_next();
    assert_eq!(orch.pending_count(), 2);

    orch.on_agent_recovered(AgentId(1));
    assert_eq!(orch.pending_count(), 1);
}
