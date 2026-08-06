//! 指标注册表（v0.120.0）.
//!
//! 对应蓝图 §4.5 `MetricRegistry`：容量 64 项，拒绝重复注册，提供
//! `contains`/`get_type` 查询。注册错误统一映射为
//! [`MetricsError::NotRegistered`]（与蓝图口径一致）。

use crate::metrics::{MetricDef, MetricType, MetricsError};

/// 指标注册表（容量 64）.
pub struct MetricRegistry {
    /// 已注册的指标定义.
    defs: heapless::Vec<MetricDef, 64>,
}

impl MetricRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            defs: heapless::Vec::new(),
        }
    }

    /// 注册指标定义；同名重复或容量已满时返回错误，注册表内容不变.
    pub fn register(&mut self, def: MetricDef) -> Result<(), MetricsError> {
        if self.contains(def.name.as_str()) {
            return Err(MetricsError::NotRegistered);
        }
        self.defs.push(def).map_err(|_| MetricsError::NotRegistered)
    }

    /// 查询指标是否已注册.
    pub fn contains(&self, name: &str) -> bool {
        self.defs.iter().any(|d| d.name.as_str() == name)
    }

    /// 查询已注册指标的类型.
    pub fn get_type(&self, name: &str) -> Option<MetricType> {
        self.defs
            .iter()
            .find(|d| d.name.as_str() == name)
            .map(|d| d.metric_type)
    }
}

impl Default for MetricRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use heapless::String as HString;

    use super::*;
    use crate::metrics::MetricDef;

    /// 构造一个测试用指标定义.
    fn make_def(name: &str) -> MetricDef {
        MetricDef {
            name: HString::<64>::try_from(name).unwrap(),
            metric_type: MetricType::Gauge,
            help: HString::<128>::try_from("测试指标").unwrap(),
            unit: HString::<16>::try_from("count").unwrap(),
        }
    }

    /// REG1：注册成功.
    #[test]
    fn reg1_register_success() {
        let mut r = MetricRegistry::new();
        assert!(r.register(make_def("a.b")).is_ok());
        assert!(r.contains("a.b"));
    }

    /// REG2：重复注册被拒绝且注册表内容不变.
    #[test]
    fn reg2_duplicate_rejected() {
        let mut r = MetricRegistry::new();
        r.register(make_def("a.b")).unwrap();
        let len_before = r.defs.len();
        assert!(r.register(make_def("a.b")).is_err());
        assert_eq!(r.defs.len(), len_before);
    }

    /// REG3：容量上限 64，第 65 项注册被拒绝.
    #[test]
    fn reg3_capacity_limit() {
        let mut r = MetricRegistry::new();
        let mut buf = [0u8; 32];
        for i in 0..64usize {
            // 生成唯一指标名 m.N
            let name = format_name(&mut buf, i);
            assert!(r.register(make_def(name)).is_ok());
        }
        assert_eq!(r.defs.len(), 64);
        assert!(r.register(make_def("m.overflow")).is_err());
        assert_eq!(r.defs.len(), 64);
    }

    /// REG4：get_type / contains 查询.
    #[test]
    fn reg4_query() {
        let mut r = MetricRegistry::new();
        let mut def = make_def("sys.cpu");
        def.metric_type = MetricType::Counter;
        r.register(def).unwrap();
        assert!(r.contains("sys.cpu"));
        assert_eq!(r.get_type("sys.cpu"), Some(MetricType::Counter));
        assert!(!r.contains("sys.ghost"));
        assert_eq!(r.get_type("sys.ghost"), None);
    }

    /// 生成形如 `m.N` 的唯一指标名（借用缓冲避免 alloc 格式化依赖）.
    fn format_name(buf: &mut [u8; 32], i: usize) -> &str {
        let mut pos = 0usize;
        for b in b"m." {
            buf[pos] = *b;
            pos += 1;
        }
        // 十进制写入序号
        let mut digits = [0u8; 10];
        let mut n = i;
        let mut dlen = 0usize;
        if n == 0 {
            digits[0] = b'0';
            dlen = 1;
        } else {
            while n > 0 {
                digits[dlen] = b'0' + (n % 10) as u8;
                dlen += 1;
                n /= 10;
            }
        }
        for k in 0..dlen {
            buf[pos] = digits[dlen - 1 - k];
            pos += 1;
        }
        core::str::from_utf8(&buf[..pos]).unwrap()
    }
}
