//! 请求优先级（D5：普通枚举，不使用原子类型）.

/// 推理请求优先级.
///
/// 派生 `Ord`，按声明顺序递增：`Low < Normal < High < Critical`。
/// 调度器按优先级降序派发（`Critical` 最先执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum RequestPriority {
    /// 低优先级.
    Low,
    /// 普通优先级（默认）.
    #[default]
    Normal,
    /// 高优先级.
    High,
    /// 关键优先级（最高）.
    Critical,
}
