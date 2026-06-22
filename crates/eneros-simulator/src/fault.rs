//! 故障注入框架
//!
//! 提供 [`FaultInjector`] 向电网注入故障，并维护当前活跃故障列表；
//! 提供 [`FaultScenarioLibrary`] 预置典型故障场景（N-1、N-2、级联、保护拒动/误动）。
//!
//! ## 设计说明
//!
//! 本模块的 [`FaultType`] / [`FaultSpec`] 与 `eneros_analysis::short_circuit` 中的
//! 同名类型是独立的：分析模块的类型面向短路计算（母线 ID 为 `ElementId`、阻抗为
//! 复数），本模块的类型面向场景脚本（母线/支路 ID 为字符串、阻抗为标幺值实数），
//! 便于从 TOML 场景文件直接反序列化。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 故障类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FaultType {
    /// 三相短路
    ThreePhase,
    /// 单相接地
    SinglePhaseToGround,
    /// 相间短路
    PhaseToPhase,
    /// 两相接地
    TwoPhaseToGround,
    /// 断线故障
    OpenCircuit,
}

/// 故障规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultSpec {
    /// 故障 ID（自动生成或指定）
    pub fault_id: String,
    /// 故障母线 ID（母线故障时）
    pub bus_id: Option<String>,
    /// 故障支路 ID（线路故障时）
    pub branch_id: Option<String>,
    /// 故障类型
    pub fault_type: FaultType,
    /// 故障阻抗（标幺值）
    #[serde(default = "default_impedance")]
    pub impedance: f64,
    /// 故障持续时间（秒）
    pub duration: f64,
}

fn default_impedance() -> f64 {
    0.0
}

/// 故障注入器
pub struct FaultInjector {
    /// 当前活跃故障
    active_faults: HashMap<String, FaultSpec>,
    /// 故障计数器
    fault_counter: u64,
}

/// 故障注入结果
#[derive(Debug, Clone)]
pub struct FaultInjectionResult {
    /// 故障 ID
    pub fault_id: String,
    /// 是否成功注入
    pub injected: bool,
    /// 故障电流（A，如果计算了）
    pub fault_current: Option<f64>,
    /// 消息
    pub message: String,
}

/// 故障场景库
pub struct FaultScenarioLibrary {
    scenarios: Vec<FaultScenario>,
}

/// 预置故障场景
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultScenario {
    /// 场景名称
    pub name: String,
    /// 场景描述
    pub description: String,
    /// 故障规格列表
    pub faults: Vec<FaultSpec>,
    /// 场景类型
    pub scenario_type: ScenarioType,
}

/// 场景类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioType {
    /// N-1 故障（单设备停运）
    N1,
    /// N-2 故障（双设备停运）
    N2,
    /// 级联故障
    Cascading,
    /// 保护拒动
    ProtectionFailure,
    /// 保护误动
    ProtectionMaloperation,
}

impl FaultInjector {
    pub fn new() -> Self {
        Self {
            active_faults: HashMap::new(),
            fault_counter: 0,
        }
    }

    /// 生成故障 ID
    fn generate_fault_id(&mut self) -> String {
        self.fault_counter += 1;
        format!("fault_{}", self.fault_counter)
    }

    /// 注入故障
    pub fn inject(&mut self, mut spec: FaultSpec) -> FaultInjectionResult {
        if spec.fault_id.is_empty() {
            spec.fault_id = self.generate_fault_id();
        }
        let fault_id = spec.fault_id.clone();

        // 验证故障规格
        if spec.bus_id.is_none() && spec.branch_id.is_none() {
            return FaultInjectionResult {
                fault_id,
                injected: false,
                fault_current: None,
                message: "故障必须指定 bus_id 或 branch_id".to_string(),
            };
        }

        if spec.duration <= 0.0 {
            return FaultInjectionResult {
                fault_id,
                injected: false,
                fault_current: None,
                message: "故障持续时间必须大于 0".to_string(),
            };
        }

        // 校验阻抗：负阻抗物理无意义
        if spec.impedance < 0.0 {
            return FaultInjectionResult {
                fault_id,
                injected: false,
                fault_current: None,
                message: "故障阻抗不能为负".to_string(),
            };
        }

        // 计算故障电流（简化）
        let fault_current = match spec.fault_type {
            FaultType::ThreePhase => Some(10000.0 / (spec.impedance + 0.01)),
            FaultType::SinglePhaseToGround => Some(8000.0 / (spec.impedance + 0.01)),
            FaultType::PhaseToPhase => Some(8660.0 / (spec.impedance + 0.01)),
            FaultType::TwoPhaseToGround => Some(9000.0 / (spec.impedance + 0.01)),
            FaultType::OpenCircuit => None,
        };

        self.active_faults.insert(fault_id.clone(), spec);

        FaultInjectionResult {
            fault_id,
            injected: true,
            fault_current,
            message: "故障注入成功".to_string(),
        }
    }

    /// 清除故障
    pub fn clear(&mut self, fault_id: &str) -> Result<(), String> {
        if self.active_faults.remove(fault_id).is_some() {
            Ok(())
        } else {
            Err(format!("故障 {} 不存在", fault_id))
        }
    }

    /// 清除所有故障
    pub fn clear_all(&mut self) {
        self.active_faults.clear();
    }

    /// 获取活跃故障列表
    pub fn active_faults(&self) -> Vec<&FaultSpec> {
        self.active_faults.values().collect()
    }

    /// 获取活跃故障数量
    pub fn active_count(&self) -> usize {
        self.active_faults.len()
    }
}

impl Default for FaultInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl FaultScenarioLibrary {
    pub fn new() -> Self {
        let scenarios = vec![
            FaultScenario {
                name: "n1_bus_fault".to_string(),
                description: "N-1 故障：单母线三相短路".to_string(),
                scenario_type: ScenarioType::N1,
                faults: vec![FaultSpec {
                    fault_id: String::new(),
                    bus_id: Some("bus_1".to_string()),
                    branch_id: None,
                    fault_type: FaultType::ThreePhase,
                    impedance: 0.0,
                    duration: 0.1,
                }],
            },
            FaultScenario {
                name: "n2_double_line".to_string(),
                description: "N-2 故障：双回线同时跳闸".to_string(),
                scenario_type: ScenarioType::N2,
                faults: vec![
                    FaultSpec {
                        fault_id: String::new(),
                        bus_id: None,
                        branch_id: Some("branch_1".to_string()),
                        fault_type: FaultType::OpenCircuit,
                        impedance: 0.0,
                        duration: 999.0,
                    },
                    FaultSpec {
                        fault_id: String::new(),
                        bus_id: None,
                        branch_id: Some("branch_2".to_string()),
                        fault_type: FaultType::OpenCircuit,
                        impedance: 0.0,
                        duration: 999.0,
                    },
                ],
            },
            FaultScenario {
                name: "cascading_failure".to_string(),
                description: "级联故障：母线故障 → 线路过载跳闸".to_string(),
                scenario_type: ScenarioType::Cascading,
                faults: vec![
                    FaultSpec {
                        fault_id: String::new(),
                        bus_id: Some("bus_3".to_string()),
                        branch_id: None,
                        fault_type: FaultType::ThreePhase,
                        impedance: 0.0,
                        duration: 0.1,
                    },
                    FaultSpec {
                        fault_id: String::new(),
                        bus_id: None,
                        branch_id: Some("branch_5".to_string()),
                        fault_type: FaultType::OpenCircuit,
                        impedance: 0.0,
                        duration: 999.0,
                    },
                ],
            },
            FaultScenario {
                name: "protection_failure".to_string(),
                description: "保护拒动：故障未被及时清除".to_string(),
                scenario_type: ScenarioType::ProtectionFailure,
                faults: vec![FaultSpec {
                    fault_id: String::new(),
                    bus_id: Some("bus_2".to_string()),
                    branch_id: None,
                    fault_type: FaultType::ThreePhase,
                    impedance: 0.0,
                    duration: 2.0, // 长时间故障，模拟保护拒动
                }],
            },
            FaultScenario {
                name: "protection_maloperation".to_string(),
                description: "保护误动：无故障时跳闸".to_string(),
                scenario_type: ScenarioType::ProtectionMaloperation,
                faults: vec![FaultSpec {
                    fault_id: String::new(),
                    bus_id: None,
                    branch_id: Some("branch_3".to_string()),
                    fault_type: FaultType::OpenCircuit,
                    impedance: 0.0,
                    duration: 999.0,
                }],
            },
        ];
        Self { scenarios }
    }

    /// 列出所有场景
    pub fn list(&self) -> &[FaultScenario] {
        &self.scenarios
    }

    /// 按名称查找场景
    pub fn find(&self, name: &str) -> Option<&FaultScenario> {
        self.scenarios.iter().find(|s| s.name == name)
    }

    /// 按类型筛选场景
    pub fn filter_by_type(&self, scenario_type: ScenarioType) -> Vec<&FaultScenario> {
        self.scenarios
            .iter()
            .filter(|s| s.scenario_type == scenario_type)
            .collect()
    }
}

impl Default for FaultScenarioLibrary {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fault_injector_new() {
        let injector = FaultInjector::new();
        assert_eq!(injector.active_count(), 0);
        assert!(injector.active_faults().is_empty());
    }

    #[test]
    fn test_inject_three_phase() {
        let mut injector = FaultInjector::new();
        let spec = FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_1".to_string()),
            branch_id: None,
            fault_type: FaultType::ThreePhase,
            impedance: 0.0,
            duration: 0.1,
        };
        let result = injector.inject(spec);
        assert!(result.injected, "三相短路注入应成功");
        assert!(result.fault_current.is_some(), "三相短路应计算故障电流");
        // 三相短路电流 = 10000 / (0 + 0.01) = 1000000 A
        let expected = 10000.0 / 0.01;
        let actual = result.fault_current.unwrap();
        assert!(
            (actual - expected).abs() < 1e-6,
            "故障电流不匹配：期望 {}，实际 {}",
            expected,
            actual
        );
        assert!(!result.fault_id.is_empty(), "应自动生成故障 ID");
        assert_eq!(injector.active_count(), 1);
    }

    #[test]
    fn test_inject_single_phase_to_ground() {
        let mut injector = FaultInjector::new();
        let spec = FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_2".to_string()),
            branch_id: None,
            fault_type: FaultType::SinglePhaseToGround,
            impedance: 0.0,
            duration: 0.2,
        };
        let result = injector.inject(spec);
        assert!(result.injected, "单相接地注入应成功");
        assert!(result.fault_current.is_some(), "单相接地应计算故障电流");
        // 单相接地电流 = 8000 / (0 + 0.01) = 800000 A
        let expected = 8000.0 / 0.01;
        let actual = result.fault_current.unwrap();
        assert!(
            (actual - expected).abs() < 1e-6,
            "故障电流不匹配：期望 {}，实际 {}",
            expected,
            actual
        );
        assert_eq!(injector.active_count(), 1);
    }

    #[test]
    fn test_inject_open_circuit() {
        let mut injector = FaultInjector::new();
        let spec = FaultSpec {
            fault_id: String::new(),
            bus_id: None,
            branch_id: Some("branch_1".to_string()),
            fault_type: FaultType::OpenCircuit,
            impedance: 0.0,
            duration: 999.0,
        };
        let result = injector.inject(spec);
        assert!(result.injected, "断线故障注入应成功");
        assert!(
            result.fault_current.is_none(),
            "断线故障不应计算故障电流"
        );
        assert_eq!(injector.active_count(), 1);
    }

    #[test]
    fn test_clear_fault() {
        let mut injector = FaultInjector::new();
        let spec = FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_1".to_string()),
            branch_id: None,
            fault_type: FaultType::ThreePhase,
            impedance: 0.0,
            duration: 0.1,
        };
        let result = injector.inject(spec);
        assert_eq!(injector.active_count(), 1);
        // 清除故障
        injector.clear(&result.fault_id).expect("清除故障失败");
        assert_eq!(injector.active_count(), 0);
        // 再次清除应失败
        assert!(
            injector.clear(&result.fault_id).is_err(),
            "清除不存在的故障应返回错误"
        );
    }

    #[test]
    fn test_scenario_library_list() {
        let library = FaultScenarioLibrary::new();
        let scenarios = library.list();
        // 预置 5 个场景
        assert_eq!(scenarios.len(), 5);
        // 验证场景名称
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"n1_bus_fault"));
        assert!(names.contains(&"n2_double_line"));
        assert!(names.contains(&"cascading_failure"));
        assert!(names.contains(&"protection_failure"));
        assert!(names.contains(&"protection_maloperation"));
    }

    #[test]
    fn test_scenario_library_find() {
        let library = FaultScenarioLibrary::new();
        // 查找存在的场景
        let scenario = library
            .find("n1_bus_fault")
            .expect("应找到 n1_bus_fault 场景");
        assert_eq!(scenario.name, "n1_bus_fault");
        assert_eq!(scenario.scenario_type, ScenarioType::N1);
        assert_eq!(scenario.faults.len(), 1);
        assert_eq!(scenario.faults[0].fault_type, FaultType::ThreePhase);
        // 查找不存在的场景
        assert!(library.find("nonexistent").is_none());
    }

    #[test]
    fn test_inject_negative_impedance() {
        // 负阻抗物理无意义，应拒绝注入
        let mut injector = FaultInjector::new();
        let spec = FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_1".to_string()),
            branch_id: None,
            fault_type: FaultType::ThreePhase,
            impedance: -1.0,
            duration: 0.1,
        };
        let result = injector.inject(spec);
        assert!(!result.injected, "负阻抗应注入失败");
        assert!(
            result.fault_current.is_none(),
            "注入失败时不应计算故障电流"
        );
        assert_eq!(injector.active_count(), 0, "失败时不应记录活跃故障");
        assert!(
            result.message.contains("阻抗"),
            "错误消息应提及阻抗，实际：{}",
            result.message
        );
    }

    /// 验证 `clear_all` 清除所有活跃故障。
    #[test]
    fn test_clear_all() {
        let mut injector = FaultInjector::new();
        // 注入多个故障
        injector.inject(FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_1".to_string()),
            branch_id: None,
            fault_type: FaultType::ThreePhase,
            impedance: 0.0,
            duration: 0.1,
        });
        injector.inject(FaultSpec {
            fault_id: String::new(),
            bus_id: None,
            branch_id: Some("branch_1".to_string()),
            fault_type: FaultType::OpenCircuit,
            impedance: 0.0,
            duration: 999.0,
        });
        assert_eq!(injector.active_count(), 2, "应有两个活跃故障");
        // 清除所有故障
        injector.clear_all();
        assert_eq!(injector.active_count(), 0, "清除所有故障后应无活跃故障");
        assert!(injector.active_faults().is_empty(), "活跃故障列表应为空");
    }

    /// 验证 `active_faults` 返回当前活跃故障列表。
    #[test]
    fn test_active_faults() {
        let mut injector = FaultInjector::new();
        // 初始无活跃故障
        assert!(injector.active_faults().is_empty());
        // 注入两个故障
        let result1 = injector.inject(FaultSpec {
            fault_id: String::new(),
            bus_id: Some("bus_1".to_string()),
            branch_id: None,
            fault_type: FaultType::ThreePhase,
            impedance: 0.0,
            duration: 0.1,
        });
        let result2 = injector.inject(FaultSpec {
            fault_id: String::new(),
            bus_id: None,
            branch_id: Some("branch_1".to_string()),
            fault_type: FaultType::OpenCircuit,
            impedance: 0.0,
            duration: 999.0,
        });
        // 验证活跃故障列表包含已注入的故障
        let active = injector.active_faults();
        assert_eq!(active.len(), 2, "应有两个活跃故障");
        let fault_ids: Vec<&str> = active.iter().map(|f| f.fault_id.as_str()).collect();
        assert!(
            fault_ids.contains(&result1.fault_id.as_str()),
            "活跃故障列表应包含第一个故障"
        );
        assert!(
            fault_ids.contains(&result2.fault_id.as_str()),
            "活跃故障列表应包含第二个故障"
        );
    }

    /// 验证 `FaultScenarioLibrary::filter_by_type` 按类型过滤场景。
    #[test]
    fn test_filter_by_type() {
        let library = FaultScenarioLibrary::new();
        // 按类型筛选场景
        let n1_scenarios = library.filter_by_type(ScenarioType::N1);
        assert_eq!(n1_scenarios.len(), 1, "应有 1 个 N-1 场景");
        assert_eq!(n1_scenarios[0].name, "n1_bus_fault");

        let n2_scenarios = library.filter_by_type(ScenarioType::N2);
        assert_eq!(n2_scenarios.len(), 1, "应有 1 个 N-2 场景");
        assert_eq!(n2_scenarios[0].name, "n2_double_line");

        let cascading_scenarios = library.filter_by_type(ScenarioType::Cascading);
        assert_eq!(cascading_scenarios.len(), 1, "应有 1 个级联故障场景");
        assert_eq!(cascading_scenarios[0].name, "cascading_failure");

        let protection_failure_scenarios =
            library.filter_by_type(ScenarioType::ProtectionFailure);
        assert_eq!(
            protection_failure_scenarios.len(),
            1,
            "应有 1 个保护拒动场景"
        );
        assert_eq!(protection_failure_scenarios[0].name, "protection_failure");

        let protection_malop_scenarios =
            library.filter_by_type(ScenarioType::ProtectionMaloperation);
        assert_eq!(
            protection_malop_scenarios.len(),
            1,
            "应有 1 个保护误动场景"
        );
        assert_eq!(protection_malop_scenarios[0].name, "protection_maloperation");
    }
}
