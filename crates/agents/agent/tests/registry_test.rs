//! Agent 注册表集成测试

use eneros_agent::*;

#[test]
fn integration_register_workflow() {
    let mut reg = AgentRegistry::new();
    let descs = vec![
        AgentDescriptor::new(AgentType::System, "sys", 1000),
        AgentDescriptor::new(AgentType::Energy, "energy", 2000),
        AgentDescriptor::new(AgentType::Market, "market", 3000),
        AgentDescriptor::new(AgentType::Grid, "grid", 4000),
        AgentDescriptor::new(AgentType::Device, "device", 5000),
    ];
    let mut ids = Vec::new();
    for d in descs {
        ids.push(reg.register(d).unwrap());
    }
    assert_eq!(reg.count(), 5);

    // lookup by ID
    assert!(reg.get(ids[0]).is_some());
    assert_eq!(reg.get(ids[1]).unwrap().name, "energy");
    // lookup by type
    assert_eq!(reg.find_by_type(AgentType::Energy).len(), 1);
    assert_eq!(reg.find_by_type(AgentType::Grid).len(), 1);
    // lookup by name
    assert!(reg.find_by_name("market").is_some());
    assert!(reg.find_by_name("nonexistent").is_none());
    // enumerate
    assert_eq!(reg.list_all().len(), 5);
    // all Created -> not alive
    assert_eq!(reg.list_alive().len(), 0);

    // unregister 2 (System + Market)
    reg.unregister(ids[0]).unwrap();
    reg.unregister(ids[2]).unwrap();
    assert_eq!(reg.count(), 3);
    // System and Market gone
    assert_eq!(reg.find_by_type(AgentType::System).len(), 0);
    assert_eq!(reg.find_by_type(AgentType::Market).len(), 0);
    // Energy, Grid, Device remain
    assert_eq!(reg.find_by_type(AgentType::Energy).len(), 1);
    assert_eq!(reg.find_by_type(AgentType::Grid).len(), 1);
    assert_eq!(reg.find_by_type(AgentType::Device).len(), 1);
}

#[test]
fn integration_stress_sequential_register() {
    let mut reg = AgentRegistry::new();
    let mut ids = Vec::new();
    for i in 0..100 {
        let name = format!("agent_{}", i);
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, &name, i as u64))
            .unwrap();
        ids.push(id);
    }
    assert_eq!(reg.count(), 100);
    for id in &ids {
        assert!(reg.get(*id).is_some(), "agent id not found after register");
    }
}

#[test]
fn integration_type_index_consistency_after_unregisters() {
    let mut reg = AgentRegistry::new();
    let mut ids = Vec::new();
    for i in 0..10 {
        let name = format!("e{}", i);
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, &name, i as u64))
            .unwrap();
        ids.push(id);
    }
    // unregister even-indexed: 0, 2, 4, 6, 8
    for i in (0..10).step_by(2) {
        reg.unregister(ids[i]).unwrap();
    }
    assert_eq!(reg.count_by_type(AgentType::Energy), 5);
    let found = reg.find_by_type(AgentType::Energy);
    assert_eq!(found.len(), 5);
    // remaining IDs must match odd-indexed: 1, 3, 5, 7, 9 (in order)
    let remaining: Vec<AgentId> = found.iter().map(|d| d.agent_id).collect();
    let expected: Vec<AgentId> = vec![ids[1], ids[3], ids[5], ids[7], ids[9]];
    assert_eq!(remaining, expected);
}

#[test]
fn integration_mixed_types_stats() {
    let mut reg = AgentRegistry::new();
    let id_sys = reg
        .register(AgentDescriptor::new(AgentType::System, "sys", 0))
        .unwrap();
    let id_energy = reg
        .register(AgentDescriptor::new(AgentType::Energy, "energy", 0))
        .unwrap();
    let _id_market = reg
        .register(AgentDescriptor::new(AgentType::Market, "market", 0))
        .unwrap();
    let _id_grid = reg
        .register(AgentDescriptor::new(AgentType::Grid, "grid", 0))
        .unwrap();
    let _id_device = reg
        .register(AgentDescriptor::new(AgentType::Device, "device", 0))
        .unwrap();

    // All start in Created state — is_alive() returns false for Created.
    let stats = reg.stats();
    assert_eq!(stats.total, 5);
    assert_eq!(stats.alive, 0);
    assert_eq!(stats.by_type.len(), 5);
    assert_eq!(stats.by_type.get(&AgentType::System).copied(), Some(1));
    assert_eq!(stats.by_type.get(&AgentType::Energy).copied(), Some(1));
    assert_eq!(stats.by_type.get(&AgentType::Market).copied(), Some(1));
    assert_eq!(stats.by_type.get(&AgentType::Grid).copied(), Some(1));
    assert_eq!(stats.by_type.get(&AgentType::Device).copied(), Some(1));

    // set one to Running — now alive=1
    reg.get_mut(id_energy).unwrap().state = AgentState::Running;
    let stats2 = reg.stats();
    assert_eq!(stats2.alive, 1);

    // id_sys used to confirm it remains Created (not alive)
    assert_eq!(reg.get(id_sys).unwrap().state, AgentState::Created);
}
