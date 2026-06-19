use eneros_os::init::{ServiceGraph, ServiceConfig};

#[test]
fn test_service_graph_topological_sort() {
    let mut graph = ServiceGraph::new();
    graph.add_service(ServiceConfig {
        name: "a".to_string(),
        ..Default::default()
    });
    graph.add_service(ServiceConfig {
        name: "b".to_string(),
        dependencies: vec!["a".to_string()],
        ..Default::default()
    });
    graph.add_service(ServiceConfig {
        name: "c".to_string(),
        dependencies: vec!["a".to_string(), "b".to_string()],
        ..Default::default()
    });

    let order = graph.topological_sort().unwrap();
    let a_pos = order.iter().position(|x| x == "a").unwrap();
    let b_pos = order.iter().position(|x| x == "b").unwrap();
    let c_pos = order.iter().position(|x| x == "c").unwrap();
    assert!(a_pos < b_pos);
    assert!(b_pos < c_pos);
}
