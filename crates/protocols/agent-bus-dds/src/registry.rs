//! DDS Topic 注册表（v0.76.0）.
//!
//! 管理 Topic 规范的注册、查询与通配符匹配。基于 `BTreeMap` 实现，no_std 兼容。
//!
//! # 偏差声明
//!
//! - **D1**：使用 `alloc::collections::BTreeMap` 替代 `std::collections::HashMap`（no_std 兼容）
//! - **D4**：`match_pattern` 实现简化通配符匹配（仅支持 `*` 后缀通配，不引入 `regex` 依赖）
//! - **D5**：不实现 `load_from_toml`（`toml` crate 需 `std`；`configs/topics.toml` 作为配置模板，
//!   运行时加载由后续版本 std 环境实现）

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::topic::{standard_topics, validate_topic_name, TopicError, TopicSpec};

/// Topic 注册表.
///
/// 管理 `TopicSpec` 的注册、查询与通配符匹配。使用 `BTreeMap` 存储以兼容 no_std
/// （**D1**：替代 `HashMap`），并支持按名称前缀的有序遍历。
pub struct TopicRegistry {
    specs: BTreeMap<String, TopicSpec>,
}

impl TopicRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            specs: BTreeMap::new(),
        }
    }

    /// 创建并预加载 8 个标准 Topic 的注册表.
    ///
    /// 标准 Topic 已经过校验，直接插入（标准集唯一，无重复）。
    pub fn with_standards() -> Self {
        let mut r = Self::new();
        for spec in standard_topics() {
            r.specs.insert(spec.name.clone(), spec);
        }
        r
    }

    /// 注册 Topic 规范.
    ///
    /// # 行为
    ///
    /// - topic 名非法 → `Err(TopicError::InvalidName)`
    /// - 同名已注册且 `default_qos` 一致 → `Ok(())`（幂等）
    /// - 同名已注册但 `default_qos` 不一致 → `Err(TopicError::Conflict)`
    /// - 否则插入并返回 `Ok(())`
    pub fn register(&mut self, spec: TopicSpec) -> Result<(), TopicError> {
        validate_topic_name(&spec.name)?;
        if let Some(existing) = self.specs.get(&spec.name) {
            if existing.default_qos != spec.default_qos {
                return Err(TopicError::Conflict {
                    name: spec.name.clone(),
                });
            }
            return Ok(()); // 幂等：同名且 QoS 一致
        }
        self.specs.insert(spec.name.clone(), spec);
        Ok(())
    }

    /// 精确查询 Topic.
    pub fn lookup(&self, name: &str) -> Option<&TopicSpec> {
        self.specs.get(name)
    }

    /// 通配符匹配.
    ///
    /// **D4**：仅支持 `*` 后缀通配（如 `/power/state/*` 匹配所有以 `/power/state/` 开头的 topic）。
    /// 不引入 `regex` 依赖（regex crate 体积大且需 std）。
    ///
    /// # 行为
    ///
    /// - pattern 以 `*` 结尾：返回所有名称以前缀（去掉 `*` 后）开头的 `&TopicSpec`
    /// - pattern 不含 `*`：精确匹配，返回单元素 `Vec` 或空 `Vec`
    pub fn match_pattern(&self, pattern: &str) -> Vec<&TopicSpec> {
        if let Some(prefix) = pattern.strip_suffix('*') {
            // 后缀通配：匹配前缀
            self.specs
                .values()
                .filter(|s| s.name.starts_with(prefix))
                .collect()
        } else {
            // 无通配符：精确匹配
            self.specs
                .get(pattern)
                .map(|s| alloc::vec![s])
                .unwrap_or_default()
        }
    }
}

impl Default for TopicRegistry {
    fn default() -> Self {
        Self::new()
    }
}
