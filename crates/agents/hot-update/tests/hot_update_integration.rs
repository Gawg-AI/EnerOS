//! v0.124.0 Agent 热更新 集成测试（host std 环境）.
//!
//! - INT1：★ 完整热更新流程——register_agent(v1) → prepare(v2 + 真实模块字节，
//!   SM3 哈希填入 new_version.module_hash) → request_started×2 →
//!   request_finished×2 → drain → swap → slot_load 断言 v2 字段 → verify →
//!   Completed → swap 返回的旧 Arc 随管理器 drop 后，v1 实例仅剩测试持有的在途
//!   引用（strong_count == 1），drop 后归零即卸载（零中断语义验证）；
//! - INT2：★ 出口判定复演（蓝图 §7.5：平滑替换）——drain 期间模拟活跃请求逐个
//!   完成（request_started 后先 drain 一次保持 Draining → request_finished 至 0
//!   → 再 drain 通过）；swap 前 slot_load 拿 v1、swap 后 slot_load 拿 v2，全程
//!   无错误返回，swap 前 load 出的 v1 Arc 在切换后仍可正常读取（请求不中断语义）；
//! - INT3：验证失败回滚——with_probe 注册返回 false 的探针 → swap 后 verify →
//!   Failed + Err(VerificationFailed) → rollback → slot_load 恢复 v1；
//! - INT4：故障注入（蓝图 §6.5：排空超时）——request_started 后不 finished，
//!   now_ms 推进超过 drain_timeout_ms → drain 返回 DrainTimeout{active:1} 且未
//!   执行 swap（slot_load 仍为 v1）；
//! - PERF5：release 模式打印 swap 切换延迟均值（循环 10000 次，每轮
//!   prepare → drain → 计时 swap → verify 复位）；仅当 `ENEROS_PERF_GATE=1`
//!   （值恰为 "1"，口径 `var(...).as_deref() == Ok("1")`，规避终端残留空串误
//!   激活）时断言均值 < 10ms（蓝图 §6.3/§7.2），默认仅打印不断言（与
//!   v0.113.0~v0.122.0 PERF 先例一致）。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env` 可用），通过
//! `eneros_agent_hot_update` crate 公共 API 调用被测的 no_std 代码。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::sync::Arc;

use eneros_agent_hot_update::{
    AgentHotUpdater, AgentHotUpdaterImpl, AgentVersion, HotUpdateError, HotUpdateState,
};
use eneros_crypto::sm3_hash;
use heapless::String as HString;

/// 构造测试版本（module_hash 默认全 0，需要 SM3 校验的用例单独填真实哈希）.
fn ver(agent_id: u32, version: &str, abi: u16) -> AgentVersion {
    AgentVersion {
        agent_id,
        version: HString::try_from(version).unwrap(),
        module_hash: [0u8; 32],
        entry_point: HString::try_from("agent_main").unwrap(),
        abi_version: abi,
    }
}

/// 读取槽位当前版本号字符串（断言便捷函数）.
fn current_version_str(u: &AgentHotUpdaterImpl, agent_id: u32) -> String {
    u.slot_load(agent_id).unwrap().version.as_str().to_string()
}

/// INT1：★ 完整热更新流程——七态链路 + swap 后路由 v2 + 旧实例引用计数归零语义.
#[test]
fn int1_full_hot_update_flow() {
    let module_v2 = b"eneros-agent-foo-v2-module-bytes";
    let mut u = AgentHotUpdaterImpl::new();
    u.register_agent(1, ver(1, "1.0.0", 1));

    // swap 前 load 一份 v1 在途引用（模拟切换瞬间仍在执行的请求）
    let v1_inflight = u.slot_load(1).unwrap();
    assert_eq!(v1_inflight.version.as_str(), "1.0.0");

    // prepare：v2 + 真实模块字节，SM3 哈希填入 new_version.module_hash（D4）
    let mut v2 = ver(1, "2.0.0", 1);
    v2.module_hash = sm3_hash(module_v2);
    u.prepare(1, v2, module_v2, 1000).unwrap();
    assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Downloading);
    assert_eq!(u.get_state(1).unwrap().start_ms, 1000);
    assert_eq!(
        u.get_state(1).unwrap().old_version.version.as_str(),
        "1.0.0"
    );

    // 2 个活跃请求进入并完成
    u.request_started(1);
    u.request_started(1);
    u.request_finished(1);
    u.request_finished(1);

    // drain：活跃=0 → Ok 可 swap
    u.drain(1, 30_000, 1010).unwrap();
    let ctx = u.get_state(1).unwrap();
    assert_eq!(ctx.state, HotUpdateState::Draining);
    assert_eq!(ctx.active_requests, 0);

    // swap：原子切换到 v2 → Verifying
    u.swap(1).unwrap();
    assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Verifying);
    // slots load 断言 v2 全字段（新请求路由到新版本）
    let v2_now = u.slot_load(1).unwrap();
    assert_eq!(v2_now.agent_id, 1);
    assert_eq!(v2_now.version.as_str(), "2.0.0");
    assert_eq!(v2_now.module_hash, sm3_hash(module_v2));
    assert_eq!(v2_now.entry_point.as_str(), "agent_main");
    assert_eq!(v2_now.abi_version, 1);

    // verify：SM3 哈希匹配 + 默认探针恒 true → Completed
    assert!(u.verify(1).unwrap());
    assert_eq!(u.get_state(1).unwrap().state, HotUpdateState::Completed);

    // v1 实例 strong_count 归零语义：swap 时管理器持有的旧 Arc 已随返回值 drop，
    // v1 仅剩测试持有的在途引用；在途请求结束后 drop，引用计数归零即卸载（零中断）
    assert_eq!(Arc::strong_count(&v1_inflight), 1);
    // 切换后 v1 在途引用仍可正常读取（请求不中断）
    assert_eq!(v1_inflight.version.as_str(), "1.0.0");
    drop(v1_inflight);
    // v2 当前 = 管理器 1 份 + 本测试 load 1 份
    assert_eq!(Arc::strong_count(&v2_now), 2);
}

/// INT2：★ 出口判定复演（蓝图 §7.5）——平滑替换：drain 期间活跃请求逐个完成，
/// swap 前后路由目标切换，全程无错误返回，请求不中断语义成立.
#[test]
fn int2_smooth_replacement_exit_criteria() {
    let module_v2 = b"eneros-agent-bar-v2-module-bytes";
    let mut u = AgentHotUpdaterImpl::new();
    u.register_agent(7, ver(7, "1.2.3", 2));
    let mut v2 = ver(7, "2.0.0", 2);
    v2.module_hash = sm3_hash(module_v2);
    u.prepare(7, v2, module_v2, 5000).unwrap();

    // 模拟 drain 期间的活跃请求：先进入 2 个
    u.request_started(7);
    u.request_started(7);
    // 第一次 drain：活跃=2 未超时 → Ok 但保持 Draining（D2 单步评估，调用方重试）
    u.drain(7, 30_000, 5010).unwrap();
    let ctx = u.get_state(7).unwrap();
    assert_eq!(ctx.state, HotUpdateState::Draining);
    assert_eq!(ctx.active_requests, 2);
    // 排空期间槽位仍路由 v1（未 swap）
    assert_eq!(current_version_str(&u, 7), "1.2.3");

    // 活跃请求逐个完成（期间又有 1 个新请求进入旧版本，属正常在途流量）
    u.request_finished(7);
    u.request_started(7);
    u.drain(7, 30_000, 5020).unwrap();
    assert_eq!(u.get_state(7).unwrap().active_requests, 2);
    u.request_finished(7);
    u.request_finished(7);
    // 活跃归零后再 drain → 通过
    u.drain(7, 30_000, 5030).unwrap();
    assert_eq!(u.get_state(7).unwrap().active_requests, 0);

    // swap 前 load 拿 v1（此刻进入的请求仍由旧版本服务）
    let before_swap = u.slot_load(7).unwrap();
    assert_eq!(before_swap.version.as_str(), "1.2.3");
    // swap：原子切换，全程无错误返回
    u.swap(7).unwrap();
    // swap 后 load 拿 v2（新请求路由到新版本）
    let after_swap = u.slot_load(7).unwrap();
    assert_eq!(after_swap.version.as_str(), "2.0.0");
    assert_eq!(after_swap.abi_version, 2);
    // 请求不中断语义：swap 前 load 出的 v1 Arc 持有方继续可用，字段读取无撕裂；
    // 持有者释放后旧实例引用计数归零自动卸载（ArcHotSwap 内存安全不变式承载）
    assert_eq!(before_swap.version.as_str(), "1.2.3");
    assert_eq!(Arc::strong_count(&before_swap), 1);

    // verify 双过 → Completed（完整平滑替换闭环，全程无 Err 返回）
    assert!(u.verify(7).unwrap());
    assert_eq!(u.get_state(7).unwrap().state, HotUpdateState::Completed);
    drop(before_swap);
    drop(after_swap);
}

/// INT3：验证失败回滚——探针返回 false → Failed + VerificationFailed →
/// rollback → 槽位恢复 v1.
#[test]
fn int3_verify_failure_rollback() {
    /// 恒不健康探针（D4：with_probe 注册，模拟新版本健康检查未通过）
    fn always_unhealthy(_agent_id: u32) -> bool {
        false
    }
    let module_v2 = b"eneros-agent-baz-v2-module-bytes";
    let mut u = AgentHotUpdaterImpl::with_probe(always_unhealthy);
    u.register_agent(3, ver(3, "1.0.0", 1));
    let mut v2 = ver(3, "2.0.0", 1);
    // SM3 哈希正确填入——隔离探针单因素失败
    v2.module_hash = sm3_hash(module_v2);
    u.prepare(3, v2, module_v2, 100).unwrap();
    u.drain(3, 30_000, 110).unwrap();
    u.swap(3).unwrap();
    // swap 后已路由 v2
    assert_eq!(current_version_str(&u, 3), "2.0.0");

    // verify：探针 false → Failed + Err(VerificationFailed)
    let err = u.verify(3).unwrap_err();
    assert_eq!(err, HotUpdateError::VerificationFailed);
    assert_eq!(u.get_state(3).unwrap().state, HotUpdateState::Failed);

    // rollback：切回旧版本，load 恢复 v1
    u.rollback(3).unwrap();
    let restored = u.slot_load(3).unwrap();
    assert_eq!(restored.version.as_str(), "1.0.0");
    assert_eq!(restored.abi_version, 1);
    assert_eq!(u.get_state(3).unwrap().state, HotUpdateState::Failed);
    // 回滚后旧版本可继续服务：新一轮 prepare 允许开启（Failed 终态可重入）
    let mut v2_retry = ver(3, "2.0.1", 1);
    v2_retry.module_hash = sm3_hash(module_v2);
    u.prepare(3, v2_retry, module_v2, 200).unwrap();
    assert_eq!(u.get_state(3).unwrap().state, HotUpdateState::Downloading);
}

/// INT4：故障注入（蓝图 §6.5）——排空超时：drain 返回 DrainTimeout{active:1}
/// 且未执行 swap（槽位仍路由 v1）.
#[test]
fn int4_drain_timeout_no_swap() {
    let module_v2 = b"eneros-agent-qux-v2-module-bytes";
    let mut u = AgentHotUpdaterImpl::new();
    u.register_agent(5, ver(5, "1.0.0", 1));
    let mut v2 = ver(5, "2.0.0", 1);
    v2.module_hash = sm3_hash(module_v2);
    u.prepare(5, v2, module_v2, 1000).unwrap();

    // 1 个长请求进入后不再完成
    u.request_started(5);
    // 未超时窗口内 drain：Ok，保持 Draining（活跃=1）
    u.drain(5, 100, 1050).unwrap();
    assert_eq!(u.get_state(5).unwrap().state, HotUpdateState::Draining);
    // now_ms 推进超过 drain_timeout_ms（1201 - 1000 = 201 > 100）
    let err = u.drain(5, 100, 1201).unwrap_err();
    assert_eq!(err, HotUpdateError::DrainTimeout { active: 1 });
    // 超时后不 swap：槽位仍路由 v1，状态停留 Draining（由运维决策重试/放弃）
    assert_eq!(current_version_str(&u, 5), "1.0.0");
    assert_eq!(u.get_state(5).unwrap().state, HotUpdateState::Draining);
    // 长请求最终完成，排空可恢复（超时非永久失败）
    u.request_finished(5);
    u.drain(5, 100, 1202).unwrap();
    assert_eq!(u.get_state(5).unwrap().active_requests, 0);
    u.swap(5).unwrap();
    assert_eq!(current_version_str(&u, 5), "2.0.0");
}

/// PERF5：蓝图 §6.3/§7.2 性能基准——swap 切换延迟均值（10000 次，每轮
/// prepare → drain → 计时 swap → verify 复位；仅 swap 调用计入耗时）。
///
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与
/// v0.113.0~v0.122.0 PERF 先例一致）时启用断言门禁（均值 < 10ms），面向
/// 性能 CI / 目标硬件场景。
#[test]
fn perf5_swap_latency() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let module = b"eneros-agent-perf-v2-module-bytes";
    let rounds = 10_000u32;

    let mut u = AgentHotUpdaterImpl::new();
    u.register_agent(1, ver(1, "1.0.0", 1));

    // 预热一轮（进入稳态，排除首次分配/缓存冷启动影响）
    let mut warm_v2 = ver(1, "2.0.0", 1);
    warm_v2.module_hash = sm3_hash(module);
    u.prepare(1, warm_v2, module, 0).unwrap();
    u.drain(1, 30_000, 1).unwrap();
    u.swap(1).unwrap();
    assert!(u.verify(1).unwrap());

    // 正式测量：每轮 prepare → drain → 计时 swap → verify 复位（Completed 终态
    // 允许下一轮 prepare）；计时区间仅含 swap 调用本身（上下文查找 + ArcHotSwap
    // 原子交换 + 状态迁移，即「切换延迟」全链路）
    let mut total = std::time::Duration::ZERO;
    for i in 0..rounds {
        let mut v2 = ver(1, "2.0.0", 1);
        v2.module_hash = sm3_hash(module);
        u.prepare(1, v2, module, std::hint::black_box(i as u64 * 10))
            .unwrap();
        u.drain(1, 30_000, i as u64 * 10 + 1).unwrap();
        let start = std::time::Instant::now();
        u.swap(1).unwrap();
        total += start.elapsed();
        assert!(u.verify(1).unwrap());
    }
    let avg = total / rounds;
    println!(
        "[PERF5] swap 切换延迟: {rounds} 次总 {total:?}，均值 {avg:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <10ms 断言门禁）"
    );

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2：切换 < 10ms）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            avg.as_nanos() < 10_000_000,
            "PERF5 swap 切换延迟均值 {avg:?} 超过 10ms 上限（蓝图 §7.2）"
        );
    }
}
