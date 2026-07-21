//! Agent 描述符集成测试

use eneros_agent::*;

#[test]
fn test_multiple_agents_have_unique_ids() {
    let a1 = AgentDescriptor::new(AgentType::System, "agent-1", 1000);
    let a2 = AgentDescriptor::new(AgentType::Energy, "agent-2", 2000);
    let a3 = AgentDescriptor::new(AgentType::Device, "agent-3", 3000);
    assert_ne!(a1.agent_id, a2.agent_id);
    assert_ne!(a2.agent_id, a3.agent_id);
    assert_ne!(a1.agent_id, a3.agent_id);
}

#[test]
fn test_agent_metadata_construction() {
    let metadata = AgentMetadata {
        name: String::from("test-agent"),
        version: String::from("1.0.0"),
        author: String::from("test"),
        description: String::from("test description"),
        entry_point: String::from("main"),
        required_capabilities: vec![String::from("net.send"), String::from("fs.read")],
    };
    assert_eq!(metadata.name, "test-agent");
    assert_eq!(metadata.required_capabilities.len(), 2);
}

#[test]
fn test_capability_ref_expiry() {
    let cap = CapabilityRef {
        cap_id: 1,
        granted_at: 100,
        expires_at: Some(1000),
    };
    assert!(!cap.is_expired(999));
    assert!(cap.is_expired(1000));
    assert!(cap.is_expired(1001));

    let cap_no_expiry = CapabilityRef {
        cap_id: 2,
        granted_at: 100,
        expires_at: None,
    };
    assert!(!cap_no_expiry.is_expired(u64::MAX));
}

#[test]
fn test_agent_descriptor_clone() {
    let a1 = AgentDescriptor::new(AgentType::Energy, "energy-agent", 5000);
    let a2 = a1.clone();
    assert_eq!(a1.agent_id, a2.agent_id);
    assert_eq!(a1.name, a2.name);
    assert_eq!(a1.agent_type, a2.agent_type);
}

#[test]
fn test_agent_error_display() {
    assert_eq!(
        format!("{}", AgentError::InvalidDescriptor),
        "invalid agent descriptor"
    );
    assert_eq!(
        format!("{}", AgentError::QuotaExceeded),
        "agent quota exceeded"
    );
    assert_eq!(
        format!("{}", AgentError::InvalidTrustLevel),
        "invalid trust level"
    );
    assert_eq!(format!("{}", AgentError::DuplicateId), "duplicate agent id");
}
