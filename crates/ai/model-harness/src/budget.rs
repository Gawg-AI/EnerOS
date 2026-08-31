//! 运行预算（[11] §3.6）：每请求 deadline / max input·output tokens /
//! max tool calls / max memory / cancellation / concurrency class /
//! 本地·远端数据策略。任何预算耗尽 → 结构化 Rejected/NoAction，绝不
//! fallback 到未认证默认命令。

/// 每请求预算（[11] §3.6 字段集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarnessBudget {
    /// 截止时刻（ns；now > deadline → 立即拒绝）。
    pub deadline_at_ns: u64,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
    /// 工具调用上限（WP-6.2 loop 使用；6.1 恒 0）。
    pub max_tool_calls: u32,
    pub max_memory_bytes: u64,
    /// 最大运行时长（ns；与 deadline 双保险——deadline 是绝对时刻，
    /// 本字段是相对时长）。
    pub max_runtime_ns: u64,
    /// 并发类（0 = 独占；1.. 共享类；调度层背压）。
    pub concurrency_class: u8,
}

/// 预算追踪器（单调递增计数；fail-closed 在耗尽点即停）。
#[derive(Debug, Clone)]
pub struct BudgetTracker {
    pub budget: HarnessBudget,
    started_at_ns: u64,
    input_tokens: u64,
    output_tokens: u64,
    tool_calls: u64,
    memory_reserved: u64,
    cancelled: bool,
}

impl BudgetTracker {
    pub fn new(budget: HarnessBudget, now_ns: u64) -> Self {
        Self {
            budget,
            started_at_ns: now_ns,
            input_tokens: 0,
            output_tokens: 0,
            tool_calls: 0,
            memory_reserved: 0,
            cancelled: false,
        }
    }

    /// 取消（cancellation；外部触发，预算不再消耗）。
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn reserve_memory(&mut self, bytes: u64) -> Result<(), &'static str> {
        if self.memory_reserved.saturating_add(bytes) > self.budget.max_memory_bytes {
            return Err("memory-budget");
        }
        self.memory_reserved += bytes;
        Ok(())
    }

    pub fn note_input_tokens(&mut self, n: u64) -> Result<(), &'static str> {
        self.input_tokens = self.input_tokens.saturating_add(n);
        if self.input_tokens > self.budget.max_input_tokens as u64 {
            return Err("input-token-budget");
        }
        Ok(())
    }

    pub fn note_output_tokens(&mut self, n: u64) -> Result<(), &'static str> {
        self.output_tokens = self.output_tokens.saturating_add(n);
        if self.output_tokens > self.budget.max_output_tokens as u64 {
            return Err("output-token-budget");
        }
        Ok(())
    }

    pub fn note_tool_call(&mut self) -> Result<(), &'static str> {
        self.tool_calls += 1;
        if self.tool_calls > self.budget.max_tool_calls as u64 {
            return Err("tool-call-budget");
        }
        Ok(())
    }

    /// 任一预算越界 → 结构化原因（None = 未耗尽）。
    pub fn exhausted(&self, now_ns: u64) -> Option<&'static str> {
        if self.cancelled {
            return Some("cancelled");
        }
        if now_ns > self.budget.deadline_at_ns {
            return Some("deadline");
        }
        if now_ns.saturating_sub(self.started_at_ns) > self.budget.max_runtime_ns {
            return Some("runtime");
        }
        if self.input_tokens > self.budget.max_input_tokens as u64 {
            return Some("input-token-budget");
        }
        if self.output_tokens > self.budget.max_output_tokens as u64 {
            return Some("output-token-budget");
        }
        if self.tool_calls > self.budget.max_tool_calls as u64 {
            return Some("tool-call-budget");
        }
        if self.memory_reserved > self.budget.max_memory_bytes {
            return Some("memory-budget");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> HarnessBudget {
        HarnessBudget {
            deadline_at_ns: 10_000,
            max_input_tokens: 100,
            max_output_tokens: 50,
            max_tool_calls: 3,
            max_memory_bytes: 1024,
            // 时长预算放大——deadline/runtime 分开验证。
            max_runtime_ns: 1_000_000,
            concurrency_class: 0,
        }
    }

    #[test]
    fn deadline_and_token_budgets() {
        let t = BudgetTracker::new(budget(), 0);
        assert_eq!(t.exhausted(9_999), None);
        assert_eq!(t.exhausted(10_001), Some("deadline"));
        let mut t = BudgetTracker::new(budget(), 0);
        t.note_input_tokens(99).unwrap();
        assert_eq!(t.note_input_tokens(1), Ok(())); // 100 ≤ 100
        assert_eq!(t.note_input_tokens(1), Err("input-token-budget")); // 101 > 100
        let mut t = BudgetTracker::new(budget(), 0);
        t.note_output_tokens(51).unwrap_err();
        assert_eq!(t.exhausted(10_000), Some("output-token-budget"));
    }

    #[test]
    fn tool_and_memory_budgets() {
        let mut t = BudgetTracker::new(budget(), 0);
        for _ in 0..3 {
            t.note_tool_call().unwrap();
        }
        // 第四次调用 → 拒绝原因（在 note 点与 exhausted 点同时可见）。
        assert_eq!(t.note_tool_call(), Err("tool-call-budget"));
        let mut t = BudgetTracker::new(budget(), 0);
        t.reserve_memory(700).unwrap();
        // 超限在 reserve 点拒绝（fail-closed 在请求点，不等到 exhausted）。
        assert_eq!(t.reserve_memory(700), Err("memory-budget"));
        // 未超限时 exhausted 不报 memory（预留受控）。
        assert_eq!(t.exhausted(10_000), None);
    }

    #[test]
    fn runtime_and_cancel() {
        let mut runtime_budget = budget();
        runtime_budget.max_runtime_ns = 5_000;
        let t = BudgetTracker::new(runtime_budget, 10);
        assert_eq!(t.exhausted(5_011), Some("runtime"));
        assert_eq!(t.exhausted(5_009), None);
        let mut t = BudgetTracker::new(budget(), 0);
        t.cancel();
        assert_eq!(t.exhausted(0), Some("cancelled"));
    }
}
