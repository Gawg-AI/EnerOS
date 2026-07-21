//! NSGA-II 多目标 Pareto 求解器（v0.104.0 T2）.
//!
//! 内置确定性 xorshift64* PRNG（D4：rand crate 依赖 std，违反全项目 no_std；
//! seed 构造注入使测试可复现），实现蓝图 §4.3 流程：初始化 → 评估（方向归一
//! D7）→ 逐代 {非支配排序 + 拥挤度 + 锦标赛选择 + 均匀交叉 + 均匀变异补满
//! pop_size（D9）} → 末次排序输出 rank == 0 前沿。
//!
//! 全程零 `std::*`（仅 `alloc::*`/`core::*`）、零 unsafe、生产路径零 panic；
//! `f64::total_cmp` 替代 `partial_cmp + unwrap`（D8）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;

use crate::pareto_front::{
    dominates, MultiObjectiveProblem, Objective, OptDirection, ParetoFront, ParetoSolution,
    ParetoSolver,
};

/// 确定性 xorshift64* PRNG（D4）.
///
/// 迁移序列 `x ^= x >> 12; x ^= x << 25; x ^= x >> 27;` 输出 `x * 0x2545F4914F6CDD1D`；
/// 状态全零会锁死，`new(0)` 以固定非零常量替换。
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    /// 以 `seed` 构造；seed == 0 时替换为固定非零常量，防止零状态锁死.
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// 生成下一个 u64 随机数.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 生成 `[lower, upper)` 区间均匀随机数.
    ///
    /// 取高 53 位映射到 `[0, 1)`，避免低位短周期偏差。
    fn gen_range_f64(&mut self, lower: f64, upper: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lower + unit * (upper - lower)
    }
}

/// NSGA-II 多目标求解器（D4：seed 注入保证确定性）.
#[derive(Debug, Clone)]
pub struct Nsga2Solver {
    /// 均匀交叉概率.
    pub crossover_rate: f64,
    /// 逐基因变异概率.
    pub mutation_rate: f64,
    /// PRNG 种子（同 seed 两次求解结果逐比特一致）.
    pub seed: u64,
}

impl Nsga2Solver {
    /// 以默认参数构造：crossover 0.9 / mutation 0.1 / 固定默认 seed（D4）.
    pub fn new() -> Self {
        Self {
            crossover_rate: 0.9,
            mutation_rate: 0.1,
            seed: 0x0005_DEEC_E66D,
        }
    }

    /// 以指定 seed 构造（交叉/变异率保持默认 0.9 / 0.1）.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            crossover_rate: 0.9,
            mutation_rate: 0.1,
            seed,
        }
    }

    /// 初始化种群：每个变量在 `[lower, upper)` 内均匀随机；目标值占位待评估.
    fn init_population(
        &self,
        problem: &MultiObjectiveProblem,
        size: usize,
        rng: &mut Xorshift64,
    ) -> Vec<ParetoSolution> {
        (0..size)
            .map(|_| ParetoSolution {
                variables: problem
                    .variables
                    .iter()
                    .map(|v| rng.gen_range_f64(v.lower, v.upper))
                    .collect(),
                objectives: alloc::vec![0.0; problem.objectives.len()],
                rank: 0,
                crowding: 0.0,
            })
            .collect()
    }

    /// 逐目标评估解的目标值；Maximize 目标出口统一取负归一（D7）.
    fn evaluate(&self, sol: &mut ParetoSolution, problem: &MultiObjectiveProblem) {
        for (k, obj) in problem.objectives.iter().enumerate() {
            sol.objectives[k] = eval_objective(&sol.variables, obj);
        }
    }

    /// 非支配排序：rank = 支配该解的解数量（rank 0 即非支配，蓝图算法）.
    ///
    /// 内层循环只读 `pop[j]`、只写局部计数，最后一次性写回 `pop[i].rank`，
    /// 避免 `pop[i]`/`pop[j]` 同时可变借用。
    fn non_dominated_sort(&self, pop: &mut [ParetoSolution]) {
        for s in pop.iter_mut() {
            s.rank = 0;
        }
        for i in 0..pop.len() {
            let mut r = 0;
            for j in 0..pop.len() {
                if i != j && dominates(&pop[j], &pop[i]) {
                    r += 1;
                }
            }
            pop[i].rank = r;
        }
    }

    /// 拥挤度计算（D8：`f64::total_cmp` 排序）.
    ///
    /// 长度 ≤ 2 时全置 `f64::MAX`；否则逐目标排序，首尾置 MAX，
    /// 中间点累加 `(next[k] - prev[k]) / range`（range == 0 时该目标跳过）。
    fn crowding_distance(&self, front: &mut [ParetoSolution]) {
        for s in front.iter_mut() {
            s.crowding = 0.0;
        }
        let n = front.len();
        if n <= 2 {
            for s in front.iter_mut() {
                s.crowding = f64::MAX;
            }
            return;
        }
        let obj_len = front[0].objectives.len();
        for k in 0..obj_len {
            front.sort_by(|a, b| a.objectives[k].total_cmp(&b.objectives[k]));
            front[0].crowding = f64::MAX;
            front[n - 1].crowding = f64::MAX;
            let range = front[n - 1].objectives[k] - front[0].objectives[k];
            if range > 0.0 {
                for i in 1..(n - 1) {
                    front[i].crowding +=
                        (front[i + 1].objectives[k] - front[i - 1].objectives[k]) / range;
                }
            }
        }
    }

    /// 锦标赛选择：随机抽两个解，rank 小者胜；平手 crowding 大者胜；再平手取前者.
    fn tournament(&self, population: &[ParetoSolution], rng: &mut Xorshift64) -> ParetoSolution {
        let n = population.len();
        let i = (rng.next_u64() as usize) % n;
        let j = (rng.next_u64() as usize) % n;
        let (a, b) = (&population[i], &population[j]);
        let winner = if a.rank < b.rank {
            a
        } else if b.rank < a.rank {
            b
        } else if a.crowding >= b.crowding {
            a
        } else {
            b
        };
        winner.clone()
    }
}

impl Default for Nsga2Solver {
    fn default() -> Self {
        Self::new()
    }
}

/// 单目标评估（蓝图口径）：cost = Σv；carbon = Σ(0.5·v)；lifespan = Σv；
/// 未知目标按 0.0 处理；Maximize 出口取负归一（D7）.
fn eval_objective(vars: &[f64], obj: &Objective) -> f64 {
    let value = match obj.name.as_str() {
        "cost" => vars.iter().sum(),
        "carbon" => vars.iter().map(|v| 0.5 * v).sum(),
        "lifespan" => vars.iter().sum(),
        _ => 0.0,
    };
    if obj.direction == OptDirection::Maximize {
        -value
    } else {
        value
    }
}

impl ParetoSolver for Nsga2Solver {
    /// NSGA-II 求解（D9）.
    ///
    /// 初始化 → 评估 → 逐代 {非支配排序 + 按 rank 分层拥挤度 + 锦标赛选择 +
    /// 均匀交叉 + 均匀变异补满 pop_size} → 末次排序输出 rank == 0 前沿。
    /// 空 variables / 空 objectives / pop_size == 0 返回
    /// `Err(SolverError::InvalidProblem(_))`，不 panic。
    fn solve(
        &self,
        problem: &MultiObjectiveProblem,
        pop_size: usize,
        gen: usize,
    ) -> Result<ParetoFront, SolverError> {
        if problem.variables.is_empty() {
            return Err(SolverError::InvalidProblem(String::from(
                "决策变量列表为空",
            )));
        }
        if problem.objectives.is_empty() {
            return Err(SolverError::InvalidProblem(String::from(
                "优化目标列表为空",
            )));
        }
        if pop_size == 0 {
            return Err(SolverError::InvalidProblem(String::from("种群大小为 0")));
        }

        let mut rng = Xorshift64::new(self.seed);
        let mut population = self.init_population(problem, pop_size, &mut rng);
        for sol in population.iter_mut() {
            self.evaluate(sol, problem);
        }

        for _ in 0..gen {
            self.non_dominated_sort(&mut population);
            // 按 rank 排序后对同 rank 段逐段计算拥挤度（锦标赛选择仅同 rank 比 crowding）.
            population.sort_by_key(|s| s.rank);
            let mut start = 0;
            while start < population.len() {
                let rank = population[start].rank;
                let mut end = start + 1;
                while end < population.len() && population[end].rank == rank {
                    end += 1;
                }
                self.crowding_distance(&mut population[start..end]);
                start = end;
            }

            // 生成下一代：锦标赛选择 + 均匀交叉 + 均匀变异，补满 pop_size（D9）.
            let mut offspring: Vec<ParetoSolution> = Vec::with_capacity(pop_size);
            while offspring.len() < pop_size {
                let p1 = self.tournament(&population, &mut rng);
                let p2 = self.tournament(&population, &mut rng);
                let mut child = if rng.gen_range_f64(0.0, 1.0) < self.crossover_rate {
                    // 均匀交叉：逐基因按 0/1 随机选亲本.
                    let vars = (0..p1.variables.len())
                        .map(|i| {
                            if rng.next_u64() & 1 == 0 {
                                p1.variables[i]
                            } else {
                                p2.variables[i]
                            }
                        })
                        .collect();
                    ParetoSolution {
                        variables: vars,
                        objectives: alloc::vec![0.0; problem.objectives.len()],
                        rank: 0,
                        crowding: 0.0,
                    }
                } else {
                    p1
                };
                // 均匀变异：逐基因按 mutation_rate 重采样至界内.
                for (i, var) in child.variables.iter_mut().enumerate() {
                    if rng.gen_range_f64(0.0, 1.0) < self.mutation_rate {
                        *var = rng
                            .gen_range_f64(problem.variables[i].lower, problem.variables[i].upper);
                    }
                }
                offspring.push(child);
            }
            population = offspring;
            for sol in population.iter_mut() {
                self.evaluate(sol, problem);
            }
        }

        self.non_dominated_sort(&mut population);
        Ok(ParetoFront {
            solutions: population.into_iter().filter(|s| s.rank == 0).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::pareto_front::VariableSpec;

    fn objective(name: &str, direction: OptDirection) -> Objective {
        Objective {
            name: String::from(name),
            direction,
            weight: 1.0,
        }
    }

    fn problem(n_vars: usize, objectives: Vec<Objective>) -> MultiObjectiveProblem {
        MultiObjectiveProblem {
            objectives,
            variables: alloc::vec![
                VariableSpec {
                    lower: 0.0,
                    upper: 10.0
                };
                n_vars
            ],
        }
    }

    fn three_objective_problem() -> MultiObjectiveProblem {
        problem(
            4,
            alloc::vec![
                objective("cost", OptDirection::Minimize),
                objective("carbon", OptDirection::Minimize),
                objective("lifespan", OptDirection::Maximize),
            ],
        )
    }

    /// NS11：init 种群界内（每变量 ∈ [lower, upper)）+ objectives 长度正确.
    #[test]
    fn ns11_init_population_within_bounds() {
        let solver = Nsga2Solver::new();
        let prob = three_objective_problem();
        let mut rng = Xorshift64::new(42);
        let pop = solver.init_population(&prob, 20, &mut rng);
        for sol in &pop {
            assert_eq!(sol.variables.len(), 4);
            assert_eq!(sol.objectives.len(), 3);
            for v in &sol.variables {
                assert!(*v >= 0.0 && *v < 10.0);
            }
            assert_eq!(sol.rank, 0);
            assert_eq!(sol.crowding, 0.0);
        }
    }

    /// NS12：init 种群大小 == pop_size.
    #[test]
    fn ns12_init_population_size() {
        let solver = Nsga2Solver::new();
        let prob = three_objective_problem();
        let mut rng = Xorshift64::new(7);
        assert_eq!(solver.init_population(&prob, 100, &mut rng).len(), 100);
        assert_eq!(solver.init_population(&prob, 1, &mut rng).len(), 1);
        assert_eq!(solver.init_population(&prob, 37, &mut rng).len(), 37);
    }

    /// NS13：同 seed 两次 solve 结果逐比特一致（D4 确定性复现）.
    #[test]
    fn ns13_same_seed_bit_identical() {
        let prob = three_objective_problem();
        let front1 = Nsga2Solver::with_seed(0xC0FFEE)
            .solve(&prob, 50, 20)
            .expect("solve 应成功");
        let front2 = Nsga2Solver::with_seed(0xC0FFEE)
            .solve(&prob, 50, 20)
            .expect("solve 应成功");
        assert_eq!(front1.solutions.len(), front2.solutions.len());
        for (s1, s2) in front1.solutions.iter().zip(front2.solutions.iter()) {
            assert_eq!(s1.variables, s2.variables);
            assert_eq!(s1.objectives, s2.objectives);
        }
    }

    /// NS14：异 seed 结果不同（至少一个 variables 元素不等）.
    #[test]
    fn ns14_different_seed_differs() {
        let prob = three_objective_problem();
        let front1 = Nsga2Solver::with_seed(1)
            .solve(&prob, 50, 10)
            .expect("solve 应成功");
        let front2 = Nsga2Solver::with_seed(2)
            .solve(&prob, 50, 10)
            .expect("solve 应成功");
        let differ = front1
            .solutions
            .iter()
            .zip(front2.solutions.iter())
            .any(|(s1, s2)| s1.variables != s2.variables)
            || front1.solutions.len() != front2.solutions.len();
        assert!(differ);
    }

    /// NS15：evaluate 三目标口径（cost=Σv / carbon=Σ0.5v / lifespan=Σv 后 Maximize 取负）.
    #[test]
    fn ns15_evaluate_three_objectives() {
        let solver = Nsga2Solver::new();
        let prob = three_objective_problem();
        let mut sol = ParetoSolution {
            variables: alloc::vec![1.0, 2.0, 3.0, 4.0],
            objectives: alloc::vec![0.0; 3],
            rank: 0,
            crowding: 0.0,
        };
        solver.evaluate(&mut sol, &prob);
        // Σv = 10.0.
        assert_eq!(sol.objectives[0], 10.0); // cost
        assert_eq!(sol.objectives[1], 5.0); // carbon
        assert_eq!(sol.objectives[2], -10.0); // lifespan（Maximize 取负）
    }

    /// NS16：Maximize 取负归一（单目标 Maximize，vars=[1,2] → objective == -3.0）.
    #[test]
    fn ns16_maximize_negation() {
        let solver = Nsga2Solver::new();
        let prob = problem(
            2,
            alloc::vec![objective("lifespan", OptDirection::Maximize)],
        );
        let mut sol = ParetoSolution {
            variables: alloc::vec![1.0, 2.0],
            objectives: alloc::vec![0.0],
            rank: 0,
            crowding: 0.0,
        };
        solver.evaluate(&mut sol, &prob);
        assert_eq!(sol.objectives[0], -3.0);
        // 对照 Minimize 不取负.
        let prob_min = problem(2, alloc::vec![objective("cost", OptDirection::Minimize)]);
        solver.evaluate(&mut sol, &prob_min);
        assert_eq!(sol.objectives[0], 3.0);
    }

    /// NS17：non_dominated_sort rank 赋值（[1,2]/[2,3]/[3,1] 最小化口径：
    /// [2,3] 被 [1,2] 支配 → rank 1；[3,1] 非支配 → rank 0）.
    #[test]
    fn ns17_non_dominated_sort_rank() {
        let solver = Nsga2Solver::new();
        let mut pop = alloc::vec![
            ParetoSolution {
                variables: alloc::vec![0.0],
                objectives: alloc::vec![1.0, 2.0],
                rank: 5,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![1.0],
                objectives: alloc::vec![2.0, 3.0],
                rank: 5,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![2.0],
                objectives: alloc::vec![3.0, 1.0],
                rank: 5,
                crowding: 0.0,
            },
        ];
        solver.non_dominated_sort(&mut pop);
        assert_eq!(pop[0].rank, 0);
        assert_eq!(pop[1].rank, 1);
        assert_eq!(pop[2].rank, 0);
    }

    /// NS18：crowding len ≤ 2 边界全置 f64::MAX.
    #[test]
    fn ns18_crowding_boundary_max() {
        let solver = Nsga2Solver::new();
        let mut empty: Vec<ParetoSolution> = Vec::new();
        solver.crowding_distance(&mut empty);
        assert!(empty.is_empty());
        let mut one = alloc::vec![ParetoSolution {
            variables: alloc::vec![0.0],
            objectives: alloc::vec![1.0, 2.0],
            rank: 0,
            crowding: 0.0,
        }];
        solver.crowding_distance(&mut one);
        assert_eq!(one[0].crowding, f64::MAX);
        let mut two = alloc::vec![
            ParetoSolution {
                variables: alloc::vec![0.0],
                objectives: alloc::vec![1.0, 2.0],
                rank: 0,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![1.0],
                objectives: alloc::vec![2.0, 3.0],
                rank: 0,
                crowding: 0.0,
            },
        ];
        solver.crowding_distance(&mut two);
        assert_eq!(two[0].crowding, f64::MAX);
        assert_eq!(two[1].crowding, f64::MAX);
    }

    /// NS19：crowding 中间值按相邻差/range 累加（单目标 + 双目标手算对比）.
    #[test]
    fn ns19_crowding_accumulation() {
        let solver = Nsga2Solver::new();
        // 单目标：objectives [1]/[2]/[4]，range=3，中间 = (4-1)/3 = 1.0.
        let mut pop = alloc::vec![
            ParetoSolution {
                variables: alloc::vec![0.0],
                objectives: alloc::vec![1.0],
                rank: 0,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![1.0],
                objectives: alloc::vec![2.0],
                rank: 0,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![2.0],
                objectives: alloc::vec![4.0],
                rank: 0,
                crowding: 0.0,
            },
        ];
        solver.crowding_distance(&mut pop);
        assert_eq!(pop[0].crowding, f64::MAX);
        assert_eq!(pop[1].crowding, 1.0);
        assert_eq!(pop[2].crowding, f64::MAX);

        // 双目标：[0,0]/[1,1]/[2,2]，每维 range=2，中间每维 (2-0)/2=1.0，累加 2.0.
        let mut pop2 = alloc::vec![
            ParetoSolution {
                variables: alloc::vec![0.0],
                objectives: alloc::vec![0.0, 0.0],
                rank: 0,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![1.0],
                objectives: alloc::vec![1.0, 1.0],
                rank: 0,
                crowding: 0.0,
            },
            ParetoSolution {
                variables: alloc::vec![2.0],
                objectives: alloc::vec![2.0, 2.0],
                rank: 0,
                crowding: 0.0,
            },
        ];
        solver.crowding_distance(&mut pop2);
        assert_eq!(pop2[0].crowding, f64::MAX);
        assert_eq!(pop2[1].crowding, 2.0);
        assert_eq!(pop2[2].crowding, f64::MAX);
    }

    /// NS20：solve e2e（4 变量三目标，50 代 × 100 种群，蓝图 §6.2）：
    /// front 非空、每解 objectives.len()==3、全部 rank==0、variables 每维界内.
    #[test]
    fn ns20_solve_e2e() {
        let solver = Nsga2Solver::new();
        let prob = three_objective_problem();
        let front = solver.solve(&prob, 100, 50).expect("solve 应成功");
        assert!(!front.is_empty());
        for sol in &front.solutions {
            assert_eq!(sol.objectives.len(), 3);
            assert_eq!(sol.rank, 0);
            assert_eq!(sol.variables.len(), 4);
            for v in &sol.variables {
                assert!(*v >= 0.0 && *v < 10.0);
            }
        }
    }

    /// NS21：InvalidProblem 三分支（空 variables / 空 objectives / pop_size==0
    /// 各返回 Err 且不 panic）.
    #[test]
    fn ns21_invalid_problem_branches() {
        let solver = Nsga2Solver::new();
        let objectives = alloc::vec![objective("cost", OptDirection::Minimize)];
        // 空 variables.
        let prob_no_vars = MultiObjectiveProblem {
            objectives: objectives.clone(),
            variables: Vec::new(),
        };
        let err = solver.solve(&prob_no_vars, 10, 1);
        assert!(matches!(err, Err(SolverError::InvalidProblem(_))));
        // 空 objectives.
        let prob_no_objs = MultiObjectiveProblem {
            objectives: Vec::new(),
            variables: alloc::vec![VariableSpec {
                lower: 0.0,
                upper: 1.0,
            }],
        };
        let err = solver.solve(&prob_no_objs, 10, 1);
        assert!(matches!(err, Err(SolverError::InvalidProblem(_))));
        // pop_size == 0.
        let prob_ok = problem(2, objectives);
        let err = solver.solve(&prob_ok, 0, 1);
        assert!(matches!(err, Err(SolverError::InvalidProblem(_))));
    }

    /// NS22：性能 50 代 × 100 种群 < 10s（D12：std::time::Instant 仅 cfg(test)）.
    #[test]
    fn ns22_performance_under_10s() {
        let solver = Nsga2Solver::new();
        let prob = three_objective_problem();
        let start = Instant::now();
        let front = solver.solve(&prob, 100, 50).expect("solve 应成功");
        let elapsed = start.elapsed();
        println!("NS22 50 代 × 100 种群耗时: {:?}", elapsed);
        assert!(!front.is_empty());
        assert!(
            elapsed.as_secs() < 10,
            "50 代 × 100 种群耗时 {:?} 超过 10s",
            elapsed
        );
    }
}
