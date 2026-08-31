//! 工具面（[11] §4.3）：七种高层、有限、可审计生产工具。**禁止工具**
//! （raw shell/network/file、protocol 读写句柄、AuthorizedWriter、
//! Gateway、delivery、修改 RuleBundle/SafetyCatalog/ProductManifest、
//! 安装模型/skill、密钥、跨租户）在类型上是**不存在的**——封闭枚举
//! 没有对应变体，allowlist 无法引用。

use alloc::vec::Vec;

/// 生产工具（封闭集；[11] §4.3 允许清单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolId {
    /// 只读受控投影。
    QuerySnapshot,
    /// 只读目录查询（asset/schema/rule refs）。
    QueryCatalog,
    /// 带分类/引用的已批准文档检索。
    RetrieveApprovedDocument,
    /// 无副作用候选仿真。
    SimulateCandidate,
    /// 无副作用目标估算。
    EstimateObjective,
    /// 只生成候选（不进入执行）。
    SubmitIntentCandidate,
    /// 请求人工审批（暂停，不自动批准）。
    RequestHumanApproval,
}

impl ToolId {
    pub const ALL: [ToolId; 7] = [
        ToolId::QuerySnapshot,
        ToolId::QueryCatalog,
        ToolId::RetrieveApprovedDocument,
        ToolId::SimulateCandidate,
        ToolId::EstimateObjective,
        ToolId::SubmitIntentCandidate,
        ToolId::RequestHumanApproval,
    ];

    /// 稳定数字码（白名单摘要用；0 保留，未登记码 fail-closed）。
    pub const fn code(self) -> u8 {
        match self {
            ToolId::QuerySnapshot => 1,
            ToolId::QueryCatalog => 2,
            ToolId::RetrieveApprovedDocument => 3,
            ToolId::SimulateCandidate => 4,
            ToolId::EstimateObjective => 5,
            ToolId::SubmitIntentCandidate => 6,
            ToolId::RequestHumanApproval => 7,
        }
    }

    pub const fn from_code(code: u8) -> Option<ToolId> {
        match code {
            1 => Some(ToolId::QuerySnapshot),
            2 => Some(ToolId::QueryCatalog),
            3 => Some(ToolId::RetrieveApprovedDocument),
            4 => Some(ToolId::SimulateCandidate),
            5 => Some(ToolId::EstimateObjective),
            6 => Some(ToolId::SubmitIntentCandidate),
            7 => Some(ToolId::RequestHumanApproval),
            _ => None,
        }
    }

    /// 只读（无副作用；可审计任意调用）。
    pub const fn is_read_only(self) -> bool {
        matches!(
            self,
            ToolId::QuerySnapshot
                | ToolId::QueryCatalog
                | ToolId::RetrieveApprovedDocument
                | ToolId::SimulateCandidate
                | ToolId::EstimateObjective
        )
    }

    /// 候选提交/人工审批（产物面）。
    pub const fn is_submission(self) -> bool {
        matches!(
            self,
            ToolId::SubmitIntentCandidate | ToolId::RequestHumanApproval
        )
    }
}

/// 调用参数（有界字节；勿存放未脱敏 chain-of-thought）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub tool: ToolId,
    pub args: Vec<u8>,
}

/// 工具结果（有界；含摘要——行动日志可验证）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub tool: ToolId,
    pub ok: bool,
    pub payload: Vec<u8>,
    /// SHA-256(payload)。
    pub digest: [u8; 32],
}

impl ToolResult {
    pub const PAYLOAD_MAX: usize = 8192;

    pub fn new(tool: ToolId, ok: bool, payload: Vec<u8>) -> Result<Self, &'static str> {
        if payload.len() > Self::PAYLOAD_MAX {
            return Err("tool result too large");
        }
        let digest = crate::sha256_bytes(&payload);
        Ok(Self {
            tool,
            ok,
            payload,
            digest,
        })
    }
}

/// 工具运行时（执行面由装配方提供；同一闭包可注入确定性替身）。
pub trait ToolRuntime {
    /// 调用（有界；错误 → 结构化 ToolError，不吞）。
    fn call(
        &mut self,
        call: &ToolCall,
        budget_ns: u64,
        now_ns: u64,
    ) -> Result<ToolResult, &'static str>;
}

/// 无副作用替身运行时（测试/沙箱）：payload 回显 + 预算记账。
#[derive(Debug, Clone, Default)]
pub struct NoopToolRuntime {
    pub calls: Vec<(ToolId, usize)>,
}

impl NoopToolRuntime {
    pub fn new() -> Self {
        Self { calls: Vec::new() }
    }
}

impl ToolRuntime for NoopToolRuntime {
    fn call(
        &mut self,
        call: &ToolCall,
        budget_ns: u64,
        _now_ns: u64,
    ) -> Result<ToolResult, &'static str> {
        let elapsed = call.args.len() as u64 * 1000;
        if elapsed > budget_ns {
            return Err("tool budget");
        }
        self.calls.push((call.tool, call.args.len()));
        ToolResult::new(call.tool, true, call.args.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_stable_and_fail_closed() {
        for (code, t) in [
            (1, ToolId::QuerySnapshot),
            (2, ToolId::QueryCatalog),
            (3, ToolId::RetrieveApprovedDocument),
            (4, ToolId::SimulateCandidate),
            (5, ToolId::EstimateObjective),
            (6, ToolId::SubmitIntentCandidate),
            (7, ToolId::RequestHumanApproval),
        ] {
            assert_eq!(t.code(), code);
            assert_eq!(ToolId::from_code(code), Some(t));
        }
        assert_eq!(ToolId::from_code(0), None);
        assert_eq!(ToolId::from_code(8), None);
    }

    #[test]
    fn read_only_and_submission_partition() {
        for t in ToolId::ALL {
            assert!(t.is_read_only() || t.is_submission());
        }
        assert!(ToolId::SubmitIntentCandidate.is_submission());
        assert!(!ToolId::SubmitIntentCandidate.is_read_only());
        assert!(ToolId::QuerySnapshot.is_read_only());
    }

    #[test]
    fn result_bounded_and_digested() {
        let r = ToolResult::new(ToolId::QuerySnapshot, true, vec![1, 2, 3]).unwrap();
        assert_eq!(r.digest, crate::sha256_bytes(&[1, 2, 3]));
        assert!(ToolResult::new(ToolId::QuerySnapshot, true, vec![0u8; 9000]).is_err());
    }

    #[test]
    fn noop_runtime_records_calls() {
        let mut rt = NoopToolRuntime::new();
        let call = ToolCall {
            tool: ToolId::QuerySnapshot,
            args: vec![7; 16],
        };
        let r = rt.call(&call, 1_000_000, 0).unwrap();
        assert!(r.ok);
        assert_eq!(rt.calls.len(), 1);
        // 预算耗尽 → 结构化错误。
        let r = rt.call(&call, 1, 0);
        assert_eq!(r, Err("tool budget"));
    }

    /// [11] §4.3：禁止工具在类型上不存在（静态路径断言）。
    #[test]
    fn forbidden_tools_unrepresentable() {
        // 封闭枚举无 writer/gateway/shell/密钥变体——编译期事实；此处
        // 双保险：ALL 内没有任何「写设备」语义工具。
        assert!(ToolId::ALL
            .iter()
            .all(|t| t.is_read_only() || t.is_submission()));
        let _ = String::new();
    }
}
