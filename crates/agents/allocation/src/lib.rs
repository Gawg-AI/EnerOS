//! # allocation kernel（蓝图 62 卷 62.3 WP-62.3.2 · S6 + D-4 收敛）
//!
//! "容量集合聚合 + 目标分派"的数值纪律**唯一实现**：有限性门
//! （[`sanitize`]）、包络判据（[`envelope`]）、比例分派
//! （[`proportional`]）、merit 序（[`merit_order`]）、均分兜底
//! （[`equal_split`]）。
//!
//! 设计裁决（蓝图 62 卷 62.3）：
//!
//! - 无外部依赖，`#![no_std]` + alloc；kernel 内统一 **f64 kW**；
//!   MW/Wh 换算留在各聚合器边界（本卷不改任何聚合器的单位与 pub 类型）。
//! - **kernel 不规定吞错策略**：非有限输入经 [`sanitize`] 显式暴露为
//!   `None`/`false`，按 0 计、按拒绝计还是按异常计由聚合器自行决定
//!   （ADR-029 三聚合器归属不动，只换内核实现）。
//! - 确定性：同输入同输出（定输入序迭代 + 全序排序键，无浮点迭代
//!   顺序漂移、无随机）。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::vec::Vec;

/// 包络判据容差（与 federation 区域求解 band 检查一致）。
const ENVELOPE_EPS: f64 = 1e-9;

/// 有限性门（单源）：非有限（NaN/±Inf）→ `None`。
///
/// 聚合器在边界调用：`None` 的处置（按 0 计 / 按拒绝计 / 按异常计）
/// 是聚合器政策，kernel 不代办。
pub fn sanitize(v: f64) -> Option<f64> {
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

/// 包络判据：`min_sum ≤ target ≤ max_sum`（含 [`ENVELOPE_EPS`] 容差；
/// 任一输入非有限 → `false`——fail-closed）。
pub fn envelope(min_sum: f64, max_sum: f64, target: f64) -> bool {
    match (sanitize(min_sum), sanitize(max_sum), sanitize(target)) {
        (Some(lo), Some(hi), Some(t)) => t >= lo - ENVELOPE_EPS && t <= hi + ENVELOPE_EPS,
        _ => false,
    }
}

/// 比例分派（dr 语义）：`share_i = cap_i / Σcap × target`，钳位
/// `[0, cap_i]`。
///
/// `None` = 任一容量非有限、Σcap ≤ 0、target 非有限或 target > Σcap
/// （容量不足的判拒语义保留在聚合器）。输出顺序 = 输入顺序（每输入
/// 一条——形状不变纪律）；cap 全非负时 Σ share ≤ target（钳位只减
/// 不增——负容量为垃圾输入，份额钳 0，Σ 关系不保证）。
pub fn proportional<T: Copy>(target: f64, caps: &[(T, f64)]) -> Option<Vec<(T, f64)>> {
    let t = sanitize(target)?;
    let mut total = 0.0f64;
    for (_, c) in caps {
        total += sanitize(*c)?;
    }
    if total <= 0.0 || t > total {
        return None;
    }
    Some(
        caps.iter()
            .map(|(id, c)| {
                let raw = (c / total) * t;
                (*id, raw.min(*c).max(0.0))
            })
            .collect(),
    )
}

/// merit 序（确定性 tie-break 单源）：按 `(rank, tiebreak, id)` 全序
/// 升序排列——同 rank 同 tiebreak 由 id 定序，同输入恒同序（稳定序 =
/// 输入序）。
///
/// 调用方把多字段排序键折入 `(rank, tiebreak)`（如 federation 的
/// `(price_rank, site_id)`）。
pub fn merit_order<T: Copy + Ord>(items: &[(T, u64, u64)]) -> Vec<T> {
    let mut keyed: Vec<(u64, u64, T)> = items.iter().map(|(t, r, b)| (*r, *b, *t)).collect();
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, _, t)| t).collect()
}

/// 均分兜底（multi_dispatch 语义）：`share = target / n`，逐条钳位
/// `[p_min, p_max]`（先 max 后 min——p_min > p_max 的退化输入结果 =
/// p_max，现状保真）。空表 → 空分配。
///
/// `None` = target 或任一界非有限（调用方已门 target 的场景恒 `Some`）。
pub fn equal_split<T: Copy>(target: f64, caps: &[(T, f64, f64)]) -> Option<Vec<(T, f64)>> {
    let t = sanitize(target)?;
    if caps.is_empty() {
        return Some(Vec::new());
    }
    let share = t / caps.len() as f64;
    let mut out = Vec::with_capacity(caps.len());
    for (id, p_min, p_max) in caps {
        let lo = sanitize(*p_min)?;
        let hi = sanitize(*p_max)?;
        out.push((*id, share.max(lo).min(hi)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性伪随机（LCG；无 rand 依赖——kernel 零外部依赖纪律）。
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0
        }
        fn f64_pos(&mut self) -> f64 {
            (self.next() % 10_000) as f64 / 10.0
        }
    }

    #[test]
    fn sanitize_gates_non_finite() {
        assert_eq!(sanitize(1.5), Some(1.5));
        assert_eq!(sanitize(0.0), Some(0.0));
        assert_eq!(sanitize(f64::NAN), None);
        assert_eq!(sanitize(f64::INFINITY), None);
        assert_eq!(sanitize(f64::NEG_INFINITY), None);
    }

    #[test]
    fn envelope_band_and_fail_closed() {
        assert!(envelope(10.0, 100.0, 50.0));
        assert!(envelope(10.0, 100.0, 10.0));
        assert!(envelope(10.0, 100.0, 100.0));
        assert!(envelope(10.0, 100.0, 100.0 + 5e-10), "容差内");
        assert!(!envelope(10.0, 100.0, 100.1));
        assert!(!envelope(10.0, 100.0, 9.0));
        assert!(!envelope(f64::NAN, 100.0, 50.0), "非有限 fail-closed");
        assert!(!envelope(10.0, f64::INFINITY, 50.0));
        assert!(!envelope(10.0, 100.0, f64::NAN));
    }

    #[test]
    fn proportional_basic_and_insufficient() {
        let caps = [(1u64, 150.0), (2u64, 200.0)];
        let out = proportional(175.0, &caps).expect("ok");
        assert_eq!(out.len(), 2);
        assert!((out[0].1 - 75.0).abs() < 1e-9, "150/350×175 = 75");
        assert!((out[1].1 - 100.0).abs() < 1e-9, "200/350×175 = 100");
        assert_eq!(proportional(351.0, &caps), None, "target > Σcap");
        assert!(proportional(0.0, &caps).is_some(), "target=0 可行");
        // 负 target：现状语义 = 份额钳位 [0, cap] 后全 0（符号政策归
        // 聚合器——kernel 只做有限性门，不扩政策）。
        let neg = proportional(-1.0, &caps).expect("现状=不拒");
        assert!(neg.iter().all(|(_, v)| *v == 0.0), "负 target 全 0 分配");
        assert_eq!(proportional(f64::NAN, &caps), None);
        assert_eq!(
            proportional(10.0, &[(1u64, f64::NAN)]),
            None,
            "非有限容量拒"
        );
        assert_eq!(proportional(10.0, &[(1u64, -5.0)]), None, "Σcap ≤ 0 拒");
    }

    #[test]
    fn proportional_sum_never_exceeds_target_property() {
        let mut rng = Lcg(0xD4);
        for _ in 0..1000 {
            let n = (rng.next() % 6 + 1) as usize;
            let caps: Vec<(u64, f64)> = (0..n).map(|i| (i as u64, rng.f64_pos())).collect();
            let target = rng.f64_pos();
            if let Some(out) = proportional(target, &caps) {
                let sum: f64 = out.iter().map(|(_, v)| *v).sum();
                assert!(sum <= target + 1e-9, "Σ share {sum} ≤ target {target}");
                for ((_, c), (_, v)) in caps.iter().zip(out.iter()) {
                    assert!((0.0..=*c).contains(v), "share {v} ∈ [0, cap {c}]");
                }
            }
        }
    }

    #[test]
    fn merit_order_total_order_tiebreak() {
        let items = [(3usize, 2u64, 30u64), (1, 0, 10), (2, 2, 20)];
        assert_eq!(merit_order(&items), vec![1, 2, 3]);
        // 同 (rank, tiebreak) → id 全序兜底（同输入恒同序）。
        let tied = [(9usize, 1u64, 1u64), (7, 1, 1), (8, 1, 1)];
        assert_eq!(merit_order(&tied), vec![7, 8, 9]);
    }

    #[test]
    fn equal_split_semantics_and_degenerate_bounds() {
        let caps = [(1u64, 0.0, 10.0), (2u64, 0.0, 5.0)];
        let out = equal_split(10.0, &caps).expect("ok");
        assert_eq!(out[0].1, 5.0);
        assert_eq!(out[1].1, 5.0, "clamp 到 p_max");
        let neg = [((1u64), -2.0, 0.0), (2, -2.0, 0.0)];
        let out = equal_split(-10.0, &neg).expect("ok");
        assert_eq!(out[0].1, -2.0, "clamp 到 p_min");
        let degenerate = [(1u64, 5.0, 1.0)];
        let out = equal_split(10.0, &degenerate).expect("ok");
        assert_eq!(out[0].1, 1.0, "p_min > p_max 退化 = p_max（现状保真）");
        let empty: Vec<(u64, f64)> = Vec::new();
        assert_eq!(equal_split(10.0, &[] as &[(u64, f64, f64)]), Some(empty));
        assert_eq!(equal_split(f64::NAN, &caps), None);
        assert_eq!(equal_split(10.0, &[(1u64, 0.0, f64::NAN)]), None);
    }

    #[test]
    fn same_input_same_output_1000_runs() {
        let mut rng = Lcg(0xC0FFEE);
        for _ in 0..1000 {
            let caps: Vec<(u64, f64)> = (0..5).map(|i| (i, (rng.next() % 1000) as f64)).collect();
            let target = (rng.next() % 2000) as f64;
            let ranked: Vec<(u64, u64, u64)> =
                caps.iter().map(|(i, _)| (*i, rng.next() % 3, *i)).collect();
            let bounds: Vec<(u64, f64, f64)> = caps.iter().map(|(i, c)| (*i, 0.0, *c)).collect();

            let a = proportional(target, &caps);
            let b = proportional(target, &caps);
            match (a, b) {
                (Some(x), Some(y)) => {
                    assert_eq!(x.len(), y.len());
                    for (l, r) in x.iter().zip(y.iter()) {
                        assert_eq!(l.0, r.0);
                        assert_eq!(l.1.to_bits(), r.1.to_bits(), "位级一致");
                    }
                }
                (x, y) => assert_eq!(x.is_none(), y.is_none()),
            }
            assert_eq!(
                merit_order(&ranked),
                merit_order(&ranked),
                "merit 序位级一致"
            );
            let x = equal_split(target, &bounds);
            let y = equal_split(target, &bounds);
            match (x, y) {
                (Some(x), Some(y)) => {
                    for (l, r) in x.iter().zip(y.iter()) {
                        assert_eq!(l.1.to_bits(), r.1.to_bits(), "位级一致");
                    }
                }
                (x, y) => assert_eq!(x.is_none(), y.is_none()),
            }
        }
    }
}
