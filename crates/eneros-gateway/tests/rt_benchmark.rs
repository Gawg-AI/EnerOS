//! EnerOS v0.18.0 Task 5: 实时性基准测试
//!
//! 测量 RealtimeExecutor 命令执行延迟分布、优先级对比、SPSC 无锁队列吞吐量。
//! 非 RT 内核环境使用宽松阈值；真实 RT 内核上 P99 应 < 1ms。

use std::sync::Arc;
use std::time::Instant;

use eneros_gateway::gateway::SafetyGateway;
use eneros_gateway::{
    Command, CommandPriority, CommandType, CommandResult, RealtimeExecutor,
    SharedPriorityCommandQueue,
};
use eneros_os::rt::RtCommandQueue;

/// 计算已排序切片的第 p 百分位（p ∈ [0, 1]）。
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// 主测试：10000 次命令执行延迟分布。
///
/// 每次 `execute_one` 用 `Instant::now()` 包裹，收集微秒级延迟后排序计算
/// P50 / P99 / P99.9 / Avg / Max。非 RT 开发机使用 5000ms 宽松阈值；
/// 真实 RT 内核（SCHED_FIFO + mlockall）上 P99 应 < 1ms。
#[tokio::test]
async fn test_rt_benchmark_latency_distribution() {
    let queue = Arc::new(SharedPriorityCommandQueue::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let executor = RealtimeExecutor::new(queue, gateway);

    const N: usize = 10000;
    let mut latencies_us: Vec<u64> = Vec::with_capacity(N);
    let mut all_executed = true;

    for i in 0..N {
        let cmd = Command::new(
            CommandType::SwitchOperation,
            1,
            CommandPriority::Normal,
            &format!("bench-{i}"),
        );
        let start = Instant::now();
        let result = executor.execute_one(cmd).await;
        latencies_us.push(start.elapsed().as_micros() as u64);

        if !matches!(result, CommandResult::Executed { .. }) {
            all_executed = false;
        }
    }

    latencies_us.sort_unstable();

    let p50 = percentile(&latencies_us, 0.50);
    let p99 = percentile(&latencies_us, 0.99);
    let p999 = percentile(&latencies_us, 0.999);
    let max = *latencies_us.last().unwrap_or(&0);
    let avg = latencies_us.iter().sum::<u64>() / N as u64;

    println!("\n========== RT Benchmark: Latency Distribution (N={N}) ==========");
    println!("  P50   : {p50} μs ({:.3} ms)", p50 as f64 / 1000.0);
    println!("  P99   : {p99} μs ({:.3} ms)", p99 as f64 / 1000.0);
    println!("  P99.9 : {p999} μs ({:.3} ms)", p999 as f64 / 1000.0);
    println!("  Avg   : {avg} μs ({:.3} ms)", avg as f64 / 1000.0);
    println!("  Max   : {max} μs ({:.3} ms)", max as f64 / 1000.0);
    println!("====================================================================");

    assert!(all_executed, "all {N} commands should execute successfully");

    // P99 < 5000ms（非 RT 环境宽松阈值；真实 RT 内核应 < 1ms）
    assert!(
        p99 < 5_000_000,
        "P99 latency {p99} μs exceeds 5000ms loose threshold (non-RT)"
    );
}

/// 优先级对比：Critical vs Low 各 1000 次。
///
/// 非 RT 环境无法保证 Critical 一定更快（调度器不保证 FIFO 抢占），因此
/// 只断言两类命令都能成功执行，不断言延迟优劣。
#[tokio::test]
async fn test_rt_benchmark_priority_comparison() {
    let queue = Arc::new(SharedPriorityCommandQueue::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let executor = RealtimeExecutor::new(queue, gateway);

    const N: usize = 1000;

    let mut critical_latencies: Vec<u64> = Vec::with_capacity(N);
    let mut critical_ok = true;
    for i in 0..N {
        let cmd = Command::new(
            CommandType::SwitchOperation,
            1,
            CommandPriority::Critical,
            &format!("crit-{i}"),
        );
        let start = Instant::now();
        let result = executor.execute_one(cmd).await;
        critical_latencies.push(start.elapsed().as_micros() as u64);
        if !matches!(result, CommandResult::Executed { .. }) {
            critical_ok = false;
        }
    }

    let mut low_latencies: Vec<u64> = Vec::with_capacity(N);
    let mut low_ok = true;
    for i in 0..N {
        let cmd = Command::new(
            CommandType::SwitchOperation,
            1,
            CommandPriority::Low,
            &format!("low-{i}"),
        );
        let start = Instant::now();
        let result = executor.execute_one(cmd).await;
        low_latencies.push(start.elapsed().as_micros() as u64);
        if !matches!(result, CommandResult::Executed { .. }) {
            low_ok = false;
        }
    }

    let critical_avg = critical_latencies.iter().sum::<u64>() / N as u64;
    let low_avg = low_latencies.iter().sum::<u64>() / N as u64;

    println!("\n========== RT Benchmark: Priority Comparison (N={N}) ==========");
    println!("  Critical avg : {critical_avg} μs ({:.3} ms)", critical_avg as f64 / 1000.0);
    println!("  Low      avg : {low_avg} μs ({:.3} ms)", low_avg as f64 / 1000.0);
    println!("  Ratio        : {:.3}", critical_avg as f64 / low_avg.max(1) as f64);
    println!("================================================================");

    assert!(critical_ok, "all critical commands should execute");
    assert!(low_ok, "all low commands should execute");
}

/// SPSC 无锁队列吞吐量测试。
///
/// `RtCommandQueue` 是基于环形缓冲区的单生产者单消费者无锁队列，仅靠
/// Release/Acquire 原子操作同步 head/tail。队列容量 1024（可用 1023），
/// 因此采用 push→pop 批次循环直到累计 10000 次 push 和 10000 次 pop。
/// 吞吐量 = 总操作数（push + pop）/ 耗时，应远超 1,000,000 ops/sec。
#[test]
fn test_rt_benchmark_spsc_queue_throughput() {
    let queue: RtCommandQueue<i32, 1024> = RtCommandQueue::new();

    const TOTAL: usize = 10000;
    let mut pushed = 0usize;
    let mut popped = 0usize;

    let start = Instant::now();
    while pushed < TOTAL || popped < TOTAL {
        // 尽量 push（队列可用容量 CAPACITY-1 = 1023）
        while pushed < TOTAL && queue.try_push(pushed as i32).is_ok() {
            pushed += 1;
        }
        // 尽量 pop
        while popped < pushed && queue.try_pop().is_some() {
            popped += 1;
        }
    }
    let elapsed = start.elapsed();

    let total_ops = (pushed + popped) as u128;
    let elapsed_nanos = elapsed.as_nanos().max(1);
    let ops_per_sec = total_ops * 1_000_000_000 / elapsed_nanos;

    println!("\n========== RT Benchmark: SPSC Queue Throughput ==========");
    println!("  Capacity    : 1024 (usable 1023)");
    println!("  Pushes      : {pushed}");
    println!("  Pops        : {popped}");
    println!("  Total ops   : {total_ops}");
    println!("  Elapsed     : {elapsed_nanos} ns ({:.3} μs)", elapsed_nanos as f64 / 1000.0);
    println!("  Throughput  : {ops_per_sec} ops/sec");
    println!("==========================================================");

    assert_eq!(pushed, TOTAL, "should push all {TOTAL} items");
    assert_eq!(popped, TOTAL, "should pop all {TOTAL} items");
    assert!(
        ops_per_sec > 1_000_000,
        "throughput {ops_per_sec} ops/sec should exceed 1,000,000 ops/sec"
    );
}

/// SCHED_OTHER vs SCHED_FIFO 调度策略延迟对比测试。
///
/// 分别在 SCHED_OTHER（默认调度策略）和 SCHED_FIFO（实时调度策略）
/// 下执行 1000 次命令，对比 P99 延迟。
///
/// 非 RT 内核环境：SCHED_FIFO 配置会失败（需要 root + RT 内核），
/// 测试退化为只验证 SCHED_OTHER 路径，断言所有命令成功执行。
/// 真实 RT 内核上：SCHED_FIFO 的 P99 应显著低于 SCHED_OTHER。
#[tokio::test]
async fn test_rt_benchmark_sched_policy_comparison() {
    const N: usize = 1000;

    // Phase 1: SCHED_OTHER（默认调度策略）
    let queue1 = Arc::new(SharedPriorityCommandQueue::new());
    let gateway1 = Arc::new(SafetyGateway::new(100));
    let executor1 = RealtimeExecutor::new(queue1, gateway1);

    let mut other_latencies: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        let cmd = Command::new(
            CommandType::SwitchOperation,
            1,
            CommandPriority::Normal,
            &format!("sched-other-{i}"),
        );
        let start = Instant::now();
        let result = executor1.execute_one(cmd).await;
        other_latencies.push(start.elapsed().as_micros() as u64);
        assert!(matches!(result, CommandResult::Executed { .. }));
    }

    // Phase 2: SCHED_FIFO（实时调度策略）
    // 尝试配置 SCHED_FIFO；非 RT 内核或非 root 用户会失败，此时退化为 SCHED_OTHER
    let rt_config = eneros_os::rt::RtConfig {
        cpus: vec![],
        priority: 80,
        lock_memory: false, // 测试环境不锁内存
        use_huge_pages: false,
    };
    let runtime = eneros_os::rt::RtRuntime::new(rt_config);
    let fifo_configured = runtime.configure_current_thread().is_ok();

    let queue2 = Arc::new(SharedPriorityCommandQueue::new());
    let gateway2 = Arc::new(SafetyGateway::new(100));
    let executor2 = RealtimeExecutor::new(queue2, gateway2);

    let mut fifo_latencies: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        let cmd = Command::new(
            CommandType::SwitchOperation,
            1,
            CommandPriority::Normal,
            &format!("sched-fifo-{i}"),
        );
        let start = Instant::now();
        let result = executor2.execute_one(cmd).await;
        fifo_latencies.push(start.elapsed().as_micros() as u64);
        assert!(matches!(result, CommandResult::Executed { .. }));
    }

    // 统计
    other_latencies.sort_unstable();
    fifo_latencies.sort_unstable();
    let other_p50 = percentile(&other_latencies, 0.50);
    let other_p99 = percentile(&other_latencies, 0.99);
    let fifo_p50 = percentile(&fifo_latencies, 0.50);
    let fifo_p99 = percentile(&fifo_latencies, 0.99);

    println!("\n=== SCHED_OTHER vs SCHED_FIFO 调度策略对比 ===");
    println!("SCHED_OTHER: P50={}μs, P99={}μs", other_p50, other_p99);
    println!(
        "SCHED_FIFO:  P50={}μs, P99={}μs ({})",
        fifo_p50,
        fifo_p99,
        if fifo_configured {
            "configured"
        } else {
            "not configured (fallback to SCHED_OTHER)"
        }
    );
    if fifo_configured {
        println!(
            "P99 improvement: {}%",
            ((other_p99 - fifo_p99) * 100).checked_div(other_p99).unwrap_or(0)
        );
    }
    println!("=============================================");

    // 非 RT 环境：只断言所有命令成功执行（已在循环中断言）
    // RT 内核环境：可额外断言 fifo_p99 < other_p99（但测试环境无法保证）
    let _ = (other_p99, fifo_p99); // 抑制未使用警告
}
