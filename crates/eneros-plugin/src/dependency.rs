//! 插件依赖解析与拓扑排序
//!
//! 提供依赖存在性检查与基于 Kahn 算法的拓扑排序，
//! 用于确定插件的加载顺序并检测循环依赖。

use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::registry::PluginRegistry;

/// 检查插件的所有依赖是否都已注册
///
/// 遍历清单中声明的依赖插件名，若任一依赖未在注册表中找到，
/// 返回 `PluginError::DependencyMissing`。
pub fn check_dependencies(
    registry: &PluginRegistry,
    manifest: &PluginManifest,
) -> Result<(), PluginError> {
    for dep in &manifest.dependencies.plugins {
        if !registry.contains(dep) {
            return Err(PluginError::DependencyMissing(dep.clone()));
        }
    }
    Ok(())
}

/// 解析插件加载顺序（拓扑排序，Kahn 算法）
///
/// 输入一组插件清单，返回按依赖顺序排序的插件名列表（被依赖者在前）。
/// - 若存在循环依赖，返回 `PluginError::DependencyMissing`（描述循环链）。
/// - 若某插件依赖了不在输入列表中的插件，返回 `PluginError::DependencyMissing`。
/// - 同层无依赖节点按名称字典序输出，保证结果稳定。
pub fn resolve_load_order(manifests: &[PluginManifest]) -> Result<Vec<String>, PluginError> {
    // 构建名称 -> 依赖列表 映射，以及全部名称集合
    let mut name_to_deps: HashMap<&str, &Vec<String>> = HashMap::new();
    let mut all_names: HashSet<&str> = HashSet::new();
    for m in manifests {
        all_names.insert(m.plugin.name.as_str());
        name_to_deps.insert(m.plugin.name.as_str(), &m.dependencies.plugins);
    }

    // 构建入度表与邻接表（dep -> name 表示 dep 先加载）
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for name in &all_names {
        in_degree.entry(name).or_insert(0);
        adj.entry(name).or_default();
    }

    for (name, deps) in &name_to_deps {
        for dep in deps.iter() {
            // 依赖的插件必须在输入列表中
            if !all_names.contains(dep.as_str()) {
                return Err(PluginError::DependencyMissing(format!(
                    "dependency '{}' of plugin '{}' not in load set",
                    dep, name
                )));
            }
            adj.get_mut(dep.as_str()).unwrap().push(name);
            *in_degree.get_mut(*name).unwrap() += 1;
        }
    }

    // Kahn 算法：初始入度为 0 的节点入队（按名称排序保证稳定）
    let mut queue: VecDeque<&str> = {
        let mut zero: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();
        zero.sort();
        zero.into()
    };

    let mut result: Vec<String> = Vec::with_capacity(manifests.len());
    while let Some(name) = queue.pop_front() {
        result.push(name.to_string());
        // 收集新入度为 0 的节点，排序后入队，保证稳定输出
        let mut new_zero: Vec<&str> = Vec::new();
        for &neighbor in adj.get(name).unwrap() {
            let d = in_degree.get_mut(neighbor).unwrap();
            *d -= 1;
            if *d == 0 {
                new_zero.push(neighbor);
            }
        }
        new_zero.sort();
        for n in new_zero {
            queue.push_back(n);
        }
    }

    if result.len() != manifests.len() {
        // 存在循环依赖，收集环中节点（入度 > 0 的未处理节点）
        let mut cycle_nodes: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(&n, _)| n)
            .collect();
        cycle_nodes.sort();
        return Err(PluginError::DependencyMissing(format!(
            "circular dependency detected among: {}",
            cycle_nodes.join(", ")
        )));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        DependenciesSection, PluginManifest, PluginMetadata, PluginSection, PluginType,
    };
    use crate::registry::{PluginEntry, PluginRegistry};

    fn make_manifest(name: &str, deps: &[&str]) -> PluginManifest {
        PluginManifest {
            plugin: PluginSection {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                api_version: "0.27.0".to_string(),
                plugin_type: PluginType::Agent,
                description: String::new(),
                author: String::new(),
            },
            dependencies: DependenciesSection {
                plugins: deps.iter().map(|s| s.to_string()).collect(),
            },
            security: Default::default(),
        }
    }

    fn register(registry: &PluginRegistry, name: &str) {
        let metadata = PluginMetadata {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.27.0".to_string(),
            plugin_type: PluginType::Agent,
            description: String::new(),
        };
        registry.register(PluginEntry::new(metadata)).unwrap();
    }

    #[test]
    fn test_check_dependencies_ok() {
        let registry = PluginRegistry::new();
        register(&registry, "dep-a");
        register(&registry, "dep-b");
        let manifest = make_manifest("main", &["dep-a", "dep-b"]);
        assert!(check_dependencies(&registry, &manifest).is_ok());
    }

    #[test]
    fn test_check_dependencies_missing() {
        let registry = PluginRegistry::new();
        register(&registry, "dep-a");
        let manifest = make_manifest("main", &["dep-a", "dep-b"]);
        let result = check_dependencies(&registry, &manifest);
        assert!(matches!(result, Err(PluginError::DependencyMissing(_))));
    }

    #[test]
    fn test_check_dependencies_empty() {
        let registry = PluginRegistry::new();
        let manifest = make_manifest("main", &[]);
        assert!(check_dependencies(&registry, &manifest).is_ok());
    }

    #[test]
    fn test_resolve_load_order_no_deps() {
        let manifests = vec![
            make_manifest("a", &[]),
            make_manifest("b", &[]),
            make_manifest("c", &[]),
        ];
        let order = resolve_load_order(&manifests).unwrap();
        assert_eq!(order.len(), 3);
        // 无依赖时按名称排序
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_resolve_load_order_chain() {
        // c 依赖 b，b 依赖 a => 顺序 a, b, c
        let manifests = vec![
            make_manifest("a", &[]),
            make_manifest("b", &["a"]),
            make_manifest("c", &["b"]),
        ];
        let order = resolve_load_order(&manifests).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_resolve_load_order_diamond() {
        // d 依赖 b 和 c，b 依赖 a，c 依赖 a => a 先，然后 b/c，最后 d
        let manifests = vec![
            make_manifest("a", &[]),
            make_manifest("b", &["a"]),
            make_manifest("c", &["a"]),
            make_manifest("d", &["b", "c"]),
        ];
        let order = resolve_load_order(&manifests).unwrap();
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], "a");
        assert_eq!(order[3], "d");
        // b 和 c 在 a 之后、d 之前
        let b_pos = order.iter().position(|n| n == "b").unwrap();
        let c_pos = order.iter().position(|n| n == "c").unwrap();
        assert!(b_pos < 3 && b_pos > 0);
        assert!(c_pos < 3 && c_pos > 0);
    }

    #[test]
    fn test_resolve_load_order_circular() {
        // a -> c, b -> a, c -> b 形成环
        let manifests = vec![
            make_manifest("a", &["c"]),
            make_manifest("b", &["a"]),
            make_manifest("c", &["b"]),
        ];
        let result = resolve_load_order(&manifests);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            PluginError::DependencyMissing(msg) => {
                assert!(msg.contains("circular dependency detected among"), "msg = {}", msg);
                // 验证三个节点都在错误信息中
                assert!(msg.contains("a"), "msg should contain 'a': {}", msg);
                assert!(msg.contains("b"), "msg should contain 'b': {}", msg);
                assert!(msg.contains("c"), "msg should contain 'c': {}", msg);
            }
            other => panic!("expected DependencyMissing, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_load_order_missing_dep() {
        // a 依赖 b，但 b 不在列表中
        let manifests = vec![make_manifest("a", &["b"])];
        let result = resolve_load_order(&manifests);
        assert!(matches!(result, Err(PluginError::DependencyMissing(_))));
    }

    #[test]
    fn test_resolve_load_order_empty() {
        let manifests: Vec<PluginManifest> = vec![];
        let order = resolve_load_order(&manifests).unwrap();
        assert!(order.is_empty());
    }

    #[test]
    fn test_resolve_load_order_self_cycle() {
        let manifests = vec![make_manifest("a", &["a"])];
        let result = resolve_load_order(&manifests);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_load_order_stable_output() {
        // 两个无依赖的插件多次解析应得到相同顺序
        let manifests = vec![make_manifest("zeta", &[]), make_manifest("alpha", &[])];
        let order1 = resolve_load_order(&manifests).unwrap();
        let order2 = resolve_load_order(&manifests).unwrap();
        assert_eq!(order1, order2);
        assert_eq!(order1, vec!["alpha", "zeta"]);
    }
}
