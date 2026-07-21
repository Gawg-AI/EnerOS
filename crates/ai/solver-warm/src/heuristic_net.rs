//! 神经网络启发式（`WarmError` + `InferEngine` seam + `MockEngine` +
//! `HeuristicNet<E>` 编解码；`OnnxEngine` 见 `ffi` 模块，`onnx-ffi` 门控）.

use alloc::vec::Vec;

use eneros_solver_core::problem::{LpProblem, VarType};

use crate::candidate::CandidateSolution;
use crate::warm_start::{SolveContext, WarmStartProvider};

/// 热启动错误.
#[derive(Debug, Clone, PartialEq)]
pub enum WarmError {
    /// 模型加载失败.
    ModelLoadFailed,
    /// 推理失败（后端错误码）.
    InferenceFailed(i32),
    /// 输入/输出维度不匹配.
    InvalidDim,
}

/// 推理引擎 seam（D6：`MockEngine` 默认可测，`OnnxEngine` 由 `onnx-ffi` 门控）.
pub trait InferEngine {
    /// 推理：输入特征向量 → 输出向量.
    fn infer(&self, input: &[f32]) -> Result<Vec<f32>, WarmError>;
    /// 输入维度.
    fn input_dim(&self) -> usize;
    /// 输出维度.
    fn output_dim(&self) -> usize;
}

/// Mock 推理引擎（默认后端：预设输出 / 模拟失败，零 unsafe）.
#[derive(Debug, Clone)]
pub struct MockEngine {
    /// 预设输出.
    pub preset_output: Vec<f32>,
    /// 是否模拟推理失败.
    pub fail: bool,
    /// 输入维度（`new` 默认取预设输出长度，`with_input_dim` 显式指定）.
    pub input_dim: usize,
}

impl MockEngine {
    /// 构造（预设输出；输入维度默认 == 输出长度）.
    pub fn new(output: Vec<f32>) -> Self {
        Self {
            input_dim: output.len(),
            preset_output: output,
            fail: false,
        }
    }

    /// 构造（模拟推理失败）.
    pub fn failing() -> Self {
        Self {
            preset_output: Vec::new(),
            fail: true,
            input_dim: 0,
        }
    }

    /// 构造（显式输入维度，编码路径测试用）.
    pub fn with_input_dim(input_dim: usize, output: Vec<f32>) -> Self {
        Self {
            preset_output: output,
            fail: false,
            input_dim,
        }
    }
}

impl InferEngine for MockEngine {
    fn infer(&self, _input: &[f32]) -> Result<Vec<f32>, WarmError> {
        if self.fail {
            Err(WarmError::InferenceFailed(-1))
        } else {
            Ok(self.preset_output.clone())
        }
    }

    fn input_dim(&self) -> usize {
        self.input_dim
    }

    fn output_dim(&self) -> usize {
        self.preset_output.len()
    }
}

/// 神经网络启发式（D7：泛型推理后端可注入；蓝图 72/96 硬编码消除为 engine 维度）.
pub struct HeuristicNet<E: InferEngine> {
    /// 推理引擎.
    pub engine: E,
}

impl<E: InferEngine> HeuristicNet<E> {
    /// 构造（注入推理引擎）.
    pub fn new(engine: E) -> Self {
        Self { engine }
    }

    /// 特征编码（蓝图 §4.4）：负荷预测 → 电价信号 → 末条历史计划逐机组
    /// generation（机组序 × 周期序）拼接；不足 `input_dim` 零填充、超出截断（C38~C40）.
    pub fn encode_input(&self, ctx: &SolveContext) -> Vec<f64> {
        let dim = self.engine.input_dim();
        let mut input = Vec::with_capacity(dim);
        input.extend_from_slice(&ctx.load_forecast);
        input.extend_from_slice(&ctx.price_signal);
        if let Some(last) = ctx.history.last() {
            for s in &last.schedule {
                input.extend_from_slice(&s.generation);
            }
        }
        // resize 一步完成：不足补 0.0、超出截断.
        input.resize(dim, 0.0);
        input
    }

    /// 输出解码（蓝图 §4.4，D9）：连续列 clamp 列界（缺省 0.0 / +∞）；
    /// Binary/Integer >0.5→1 否则 0，并按蓝图公式 `c *= 1−|v−0.5|·2` 累积置信度，
    /// 最终 `/= num_vars`（C41~C43）.
    pub fn decode_output(&self, output: &[f64], problem: &LpProblem) -> CandidateSolution {
        let mut continuous = Vec::new();
        let mut integer = Vec::new();
        let mut confidence = 1.0f64;
        for (i, var_type) in problem.var_types.iter().enumerate() {
            let val = output.get(i).copied().unwrap_or(0.0);
            match var_type {
                VarType::Continuous => {
                    let lower = problem.lower_bounds.get(i).copied().unwrap_or(0.0);
                    let upper = problem
                        .upper_bounds
                        .get(i)
                        .copied()
                        .unwrap_or(f64::INFINITY);
                    continuous.push(val.max(lower).min(upper));
                }
                VarType::Integer | VarType::Binary => {
                    integer.push(if val > 0.5 { 1 } else { 0 });
                    confidence *= 1.0 - (val - 0.5).abs() * 2.0;
                }
            }
        }
        confidence /= problem.variables.len() as f64;
        CandidateSolution {
            continuous,
            integer,
            confidence,
        }
    }
}

impl<E: InferEngine> WarmStartProvider for HeuristicNet<E> {
    /// 生成热启动候选解：encode → f32 转换 → infer（Err 透传）→ 维度校验
    /// （≠ output_dim → `InvalidDim`）→ f64 回转 → decode（C44~C46）.
    fn generate(
        &self,
        problem: &LpProblem,
        ctx: &SolveContext,
    ) -> Result<CandidateSolution, WarmError> {
        let input = self.encode_input(ctx);
        let input_f32: Vec<f32> = input.iter().map(|&v| v as f32).collect();
        let output_f32 = self.engine.infer(&input_f32)?;
        if output_f32.len() != self.engine.output_dim() {
            return Err(WarmError::InvalidDim);
        }
        let output: Vec<f64> = output_f32.iter().map(|&v| v as f64).collect();
        Ok(self.decode_output(&output, problem))
    }
}

#[cfg(test)]
mod tests {
    use eneros_solver_core::problem::{ConstraintMatrix, ObjectiveSense};
    use eneros_solver_core::result::SolveStatus;
    use eneros_solver_milp::{DayAheadPlan, UnitSchedule};

    use super::*;

    fn problem(var_types: &[VarType], lower: &[f64], upper: &[f64]) -> LpProblem {
        let n = var_types.len();
        LpProblem {
            variables: (0..n).map(|i| alloc::format!("x{}", i)).collect(),
            lower_bounds: lower.to_vec(),
            upper_bounds: upper.to_vec(),
            var_types: var_types.to_vec(),
            objective: alloc::vec![0.0; n],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(0, 0, alloc::vec![0], alloc::vec![], alloc::vec![]),
            rhs_lower: alloc::vec![],
            rhs_upper: alloc::vec![],
        }
    }

    /// 2 机组 × periods 周期历史计划（generation 可区分：G1=1000+t，G2=2000+t）.
    fn history_plan(periods: usize) -> DayAheadPlan {
        DayAheadPlan {
            schedule: vec![
                UnitSchedule {
                    unit_id: String::from("G1"),
                    commitments: vec![true; periods],
                    generation: (0..periods).map(|t| 1000.0 + t as f64).collect(),
                },
                UnitSchedule {
                    unit_id: String::from("G2"),
                    commitments: vec![true; periods],
                    generation: (0..periods).map(|t| 2000.0 + t as f64).collect(),
                },
            ],
            total_cost: 0.0,
            solve_status: SolveStatus::Optimal,
        }
    }

    /// TH9：编码后维度 == `engine.input_dim()`（C39）.
    #[test]
    fn th9_encode_dim_equals_input_dim() {
        let ctx = SolveContext {
            load_forecast: vec![1.0, 2.0, 3.0],
            price_signal: vec![4.0, 5.0, 6.0],
            history: vec![],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(10, vec![]));
        assert_eq!(net.encode_input(&ctx).len(), 10);
    }

    /// TH10：拼接顺序 负荷 → 电价 → 历史 generation（机组序 × 周期序，2×24，C38）.
    #[test]
    fn th10_encode_concat_order() {
        let ctx = SolveContext {
            load_forecast: (0..24).map(|t| t as f64).collect(),
            price_signal: (0..24).map(|t| 100.0 + t as f64).collect(),
            history: vec![history_plan(24)],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(96, vec![]));
        let input = net.encode_input(&ctx);
        assert_eq!(input.len(), 96);
        // 负荷段 [0..24)
        assert_eq!(input[0], 0.0);
        assert_eq!(input[23], 23.0);
        // 电价位 [24..48)
        assert_eq!(input[24], 100.0);
        assert_eq!(input[47], 123.0);
        // 历史 generation：机组 G1 全周期 [48..72)，再机组 G2 [72..96)
        assert_eq!(input[48], 1000.0);
        assert_eq!(input[71], 1023.0);
        assert_eq!(input[72], 2000.0);
        assert_eq!(input[95], 2023.0);
    }

    /// TH11：不足 input_dim 零填充（C39）.
    #[test]
    fn th11_encode_zero_padding() {
        let ctx = SolveContext {
            load_forecast: vec![1.0, 2.0],
            price_signal: vec![3.0],
            history: vec![],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(6, vec![]));
        assert_eq!(net.encode_input(&ctx), vec![1.0, 2.0, 3.0, 0.0, 0.0, 0.0]);
    }

    /// TH12：超出 input_dim 截断（C39）.
    #[test]
    fn th12_encode_truncate() {
        let ctx = SolveContext {
            load_forecast: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            price_signal: vec![6.0, 7.0, 8.0],
            history: vec![],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(4, vec![]));
        assert_eq!(net.encode_input(&ctx), vec![1.0, 2.0, 3.0, 4.0]);
    }

    /// TH13：空 history 仅 负荷+电价+零填充（C40，不 panic）.
    #[test]
    fn th13_encode_empty_history() {
        let ctx = SolveContext {
            load_forecast: vec![1.0],
            price_signal: vec![2.0],
            history: vec![],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(4, vec![]));
        assert_eq!(net.encode_input(&ctx), vec![1.0, 2.0, 0.0, 0.0]);
    }

    /// TH14：decode 连续列 clamp 上界与下界（C41）.
    #[test]
    fn th14_decode_continuous_clamp() {
        let p = problem(
            &[VarType::Continuous, VarType::Continuous],
            &[0.0, 5.0],
            &[10.0, 20.0],
        );
        let net = HeuristicNet::new(MockEngine::new(vec![]));
        let c = net.decode_output(&[12.0, 3.0], &p);
        assert_eq!(c.continuous, vec![10.0, 5.0]);
        assert!(c.integer.is_empty());
    }

    /// TH15：decode 二值化 0.9 → 1，蓝图公式因子 0.2 累积（C42/C43）.
    #[test]
    fn th15_decode_binarize_0_9() {
        let p = problem(&[VarType::Binary], &[0.0], &[1.0]);
        let net = HeuristicNet::new(MockEngine::new(vec![]));
        let c = net.decode_output(&[0.9], &p);
        assert_eq!(c.integer, vec![1]);
        // 因子 = 1 − |0.9−0.5|·2 = 0.2；confidence = 0.2 / 1
        assert!((c.confidence - 0.2).abs() < 1e-9);
    }

    /// TH16：0.5 → 0（严格大于）；蓝图公式下 0/1 值因子为 0 → 信度整体归零（C43）.
    #[test]
    fn th16_decode_binarize_half_and_confidence_zero() {
        let p = problem(&[VarType::Binary, VarType::Binary], &[0.0; 2], &[1.0; 2]);
        let net = HeuristicNet::new(MockEngine::new(vec![]));
        let c = net.decode_output(&[0.5, 1.0], &p);
        // 0.5 不 > 0.5 → 0；1.0 → 1
        assert_eq!(c.integer, vec![0, 1]);
        // 因子(0.5) = 1 − 0·2 = 1.0；因子(1.0) = 1 − 0.5·2 = 0.0 → 累积归零
        assert_eq!(c.confidence, 0.0);
    }

    /// TH17：蓝图置信度公式累积（多整数列乘积）后除以 num_vars（C43）.
    #[test]
    fn th17_decode_confidence_formula_accumulate() {
        let p = problem(
            &[VarType::Binary, VarType::Binary, VarType::Continuous],
            &[0.0; 3],
            &[10.0; 3],
        );
        let net = HeuristicNet::new(MockEngine::new(vec![]));
        let c = net.decode_output(&[0.9, 0.7, 5.0], &p);
        assert_eq!(c.integer, vec![1, 1]);
        assert_eq!(c.continuous, vec![5.0]);
        // 因子(0.9)=0.2、因子(0.7)=0.6 → 0.2·0.6=0.12；/3 = 0.04
        assert!((c.confidence - 0.04).abs() < 1e-9);
    }

    /// TH18：MockEngine 驱动 generate 端到端（encode → infer → decode，C44）.
    #[test]
    fn th18_generate_e2e_mock_engine() {
        let p = problem(
            &[VarType::Binary, VarType::Continuous],
            &[0.0, 0.0],
            &[1.0, 10.0],
        );
        let ctx = SolveContext {
            load_forecast: vec![1.0, 2.0],
            price_signal: vec![3.0, 4.0],
            history: vec![],
        };
        let net = HeuristicNet::new(MockEngine::with_input_dim(4, vec![0.9, 5.0]));
        let c = net.generate(&p, &ctx).unwrap();
        assert_eq!(c.integer, vec![1]);
        assert_eq!(c.continuous, vec![5.0]);
        // 因子(0.9f32→f64) ≈ 0.2；/2 ≈ 0.1
        assert!((c.confidence - 0.1).abs() < 1e-6);
    }

    /// TH19：infer Err 透传为 InferenceFailed（C46，不吞没）.
    #[test]
    fn th19_generate_infer_err_propagates() {
        let p = problem(&[VarType::Binary], &[0.0], &[1.0]);
        let ctx = SolveContext::default();
        let net = HeuristicNet::new(MockEngine::failing());
        assert_eq!(
            net.generate(&p, &ctx).unwrap_err(),
            WarmError::InferenceFailed(-1)
        );
    }

    /// TH20：infer 返回维度 ≠ output_dim → InvalidDim（C45）.
    #[test]
    fn th20_generate_dim_mismatch_invalid_dim() {
        /// 声称 output_dim=3 但实际返回 2 的 stub 引擎.
        struct DimMismatchEngine;
        impl InferEngine for DimMismatchEngine {
            fn infer(&self, _input: &[f32]) -> Result<Vec<f32>, WarmError> {
                Ok(vec![1.0, 2.0])
            }
            fn input_dim(&self) -> usize {
                2
            }
            fn output_dim(&self) -> usize {
                3
            }
        }
        let p = problem(&[VarType::Binary], &[0.0], &[1.0]);
        let ctx = SolveContext::default();
        let net = HeuristicNet::new(DimMismatchEngine);
        assert_eq!(net.generate(&p, &ctx).unwrap_err(), WarmError::InvalidDim);
    }
}
