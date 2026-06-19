use std::collections::{HashMap, VecDeque};
use crate::init::service::ServiceConfig;

/// Service dependency graph (DAG)
#[derive(Debug, Default)]
pub struct ServiceGraph {
    nodes: HashMap<String, ServiceConfig>,
    edges: HashMap<String, Vec<String>>, // service -> its dependencies
}

impl ServiceGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_service(&mut self, config: ServiceConfig) {
        let name = config.name.clone();
        let deps = config.dependencies.clone();
        self.nodes.insert(name.clone(), config);
        self.edges.insert(name, deps);
    }

    /// Topological sort using Kahn's algorithm
    /// Returns services in dependency order (dependencies first)
    pub fn topological_sort(&self) -> Result<Vec<String>, GraphError> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new(); // dependency -> dependents

        for name in self.nodes.keys() {
            in_degree.entry(name.clone()).or_insert(0);
            adj.entry(name.clone()).or_default();
        }

        for (name, deps) in &self.edges {
            for dep in deps {
                if !self.nodes.contains_key(dep) {
                    return Err(GraphError::MissingDependency(dep.clone()));
                }
                adj.entry(dep.clone()).or_default().push(name.clone());
                *in_degree.entry(name.clone()).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(k, _)| k.clone())
            .collect();

        let mut result = Vec::new();
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());
            if let Some(dependents) = adj.get(&node) {
                for dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep.clone());
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(GraphError::CycleDetected);
        }

        Ok(result)
    }

    pub fn get_service(&self, name: &str) -> Option<&ServiceConfig> {
        self.nodes.get(name)
    }

    pub fn services(&self) -> impl Iterator<Item = &ServiceConfig> {
        self.nodes.values()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("missing dependency: {0}")]
    MissingDependency(String),
    #[error("cycle detected in service dependency graph")]
    CycleDetected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort_simple() {
        let mut graph = ServiceGraph::new();
        graph.add_service(ServiceConfig {
            name: "network".to_string(),
            ..Default::default()
        });
        graph.add_service(ServiceConfig {
            name: "app".to_string(),
            dependencies: vec!["network".to_string()],
            ..Default::default()
        });

        let order = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["network", "app"]);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = ServiceGraph::new();
        graph.add_service(ServiceConfig {
            name: "a".to_string(),
            dependencies: vec!["b".to_string()],
            ..Default::default()
        });
        graph.add_service(ServiceConfig {
            name: "b".to_string(),
            dependencies: vec!["a".to_string()],
            ..Default::default()
        });

        assert!(matches!(graph.topological_sort(), Err(GraphError::CycleDetected)));
    }

    #[test]
    fn test_missing_dependency() {
        let mut graph = ServiceGraph::new();
        graph.add_service(ServiceConfig {
            name: "app".to_string(),
            dependencies: vec!["nonexistent".to_string()],
            ..Default::default()
        });

        assert!(matches!(
            graph.topological_sort(),
            Err(GraphError::MissingDependency(_))
        ));
    }
}
