//! EnerOS AI 决策链 **host demo** 入口（蓝图 40 WP-40.5：`eneros-ai-runner-demo`）。
//!
//! 装配真实后端：LlamaCppEngine（llama.cpp）+ HighsSolver（HiGHS）+
//! SafetyValidator + FileCommandSink（JSONL recording）。
//!
//! # 证据类别（WP-40.5）
//!
//! 本入口的证据类别固定为 **`DEMO_ONLY`**——host JSONL recording 不产生
//! 产品证据；产品链路为 `svc-ai-worker`（no_std/target，经 EIPC 获取可信
//! 输入并返回 proposal）。`FileExecutor` 不实现生产 DeliveryChannel。
//!
//! # 无输入语义（WP-40.5）
//!
//! 本 demo 没有 TelemetryService 可接：固定注入 `FeedbackBundle::untrusted`
//! （FIX-24.R3：无真实遥测 → 显式未知 → CTL-010 fail-safe deny）——因此
//! 正常模式的授权链**必然拒绝**（untrusted 反馈）。这是诚实语义：demo
//! 没有可信输入就不假装成功；`--demo-trusted-feedback` 可显式启用
//! 演示用可信反馈（仅证据类别 DEMO_ONLY 下允许，产品链禁止）。
//!
//! 退出码：
//! - 0：端到端成功且 sink 写入命令（仅 `--demo-trusted-feedback` 路径）
//! - 1：装配/执行失败（LLM 加载、HiGHS 初始化、sink 打开、链路错误）
//! - 2：参数非法
//! - 3：Fatal 安全违规阻断（sink 零写入——P0-17 语义）

use eneros_ai_runner::cli::CliArgs;
use eneros_ai_runner::scenario::Overrides;
use eneros_ai_runner::{run_once, RunnerError};
use eneros_dual_brain::DecisionOutcome;
use eneros_dual_brain::DualBrainError;

/// 输出端到端证据 JSON（stdout；证据类别 DEMO_ONLY）。
fn print_evidence(
    args: &CliArgs,
    outcome: &eneros_ai_runner::RunnerOutcome,
) -> Result<(), RunnerError> {
    let (path_type, schedule, latency, outcome_label) = match &outcome.outcome {
        DecisionOutcome::Proposal(plan) => (
            format!("{:?}", plan.path),
            Some(&plan.schedule),
            &plan.latency,
            "proposal",
        ),
        DecisionOutcome::NoAction(reason) => (
            "none".into(),
            None,
            &eneros_dual_brain::LatencyBreakdown::default(),
            reason.as_str(),
        ),
        DecisionOutcome::Rejected(r) => (
            format!("{:?}", r.path),
            None,
            &eneros_dual_brain::LatencyBreakdown::default(),
            r.reason.as_str(),
        ),
    };
    let entry = schedule
        .and_then(|s| s.schedule.first())
        .map(|e| {
            serde_json::json!({
                "period": e.period,
                "charge_kw": e.charge_power_kw,
                "discharge_kw": e.discharge_power_kw,
                "net_kw": e.net_power_kw,
                "soc_pct": e.soc_pct,
            })
        })
        .unwrap_or(serde_json::Value::Null);
    let evidence = serde_json::json!({
        "schema": "eneros.ai-runner-demo.evidence.v1",
        // WP-40.5：host demo 明确不能产产品证据。
        "evidence_category": "DEMO_ONLY",
        "scenario": format!("{:?}", args.scenario),
        "scenario_description": args.scenario.description(),
        "now_ms": args.now_ms,
        "model": args.model,
        "sink": args.sink,
        "path_type": path_type,
        "outcome": outcome_label,
        "prompt": outcome.prompt,
        "llm_output": outcome.llm_output,
        "sink_written": outcome.sink_written,
        "schedule": {
            "first_entry": entry,
            "entries": schedule.map(|s| s.schedule.len()).unwrap_or(0),
            "total_revenue_yuan": schedule.map(|s| s.total_revenue_yuan).unwrap_or(0.0),
            "solve_status": schedule.map(|s| format!("{:?}", s.solve_status)).unwrap_or_else(|| "none".into()),
        },
        "latency_ms": {
            "perception": latency.perception_ms,
            "llm_inference": latency.llm_inference_ms,
            "intent_parse": latency.intent_parse_ms,
            "lp_build": latency.lp_build_ms,
            "lp_solve": latency.lp_solve_ms,
            "safety_validate": latency.safety_validate_ms,
            "command_dispatch": latency.command_dispatch_ms,
            "total": latency.total_ms,
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&evidence)
            .map_err(|e| RunnerError::Evidence(format!("{e}")))?
    );
    Ok(())
}

fn main() {
    // 32 AI-07：参数解析在 lib（cli 模块，host 可测）；缺密钥/非法
    // 参数 → exit 2（非零，fail-closed）。
    let args = match eneros_ai_runner::cli::parse_cli_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("eneros-ai-runner-demo: {msg}");
            std::process::exit(2);
        }
    };

    // FIX-24.R3 + WP-40.5：demo 无 TelemetryService 可接——默认注入显式
    // "未知"反馈（untrusted → CTL-010 fail-safe deny）。`--demo-trusted-feedback`
    // 显式启用演示用可信反馈（证据类别 DEMO_ONLY，产品链禁止）。
    let feedback_fn: fn(u64) -> eneros_control_gateway::chain::FeedbackBundle =
        if args.demo_trusted_feedback {
            // demo 可信反馈（grid_up=true, soc=50%, now）——仅 DEMO_ONLY。
            |now| eneros_control_gateway::chain::FeedbackBundle::fresh(true, 50.0, now)
        } else {
            eneros_control_gateway::chain::FeedbackBundle::untrusted
        };
    match run_once(
        args.scenario,
        Overrides {
            soc_min: args.soc_min,
        },
        &args.model,
        &args.sink,
        args.now_ms,
        feedback_fn,
        &args.chain_key,
    ) {
        Ok(outcome) => {
            // stdout 只输出证据 JSON（llama 日志在 stderr）——证据文件可
            // 直接被 `cargo xtask evidence` 类工具解析（A3）。
            if outcome.sink_written == 0 {
                // 链路成功但零命令：untrusted 反馈下授权链拒绝属诚实语义
                //（WP-40.5：无可信输入不假装成功）；--demo-trusted-feedback
                // 下则为异常。
                eprintln!(
                    "AI-RUNNER-DEMO: sink wrote 0 commands (feedback untrusted → chain denied; \
                     demo 无可信输入不假装成功。产品链用 svc-ai-worker + TelemetryService)"
                );
                std::process::exit(3);
            }
            if let Err(e) = print_evidence(&args, &outcome) {
                eprintln!("AI-RUNNER-DEMO evidence error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Err(RunnerError::DualBrain(DualBrainError::SafetyFatal(msg))) => {
            // Fatal 安全违规：阻断 sink（零写入），退出码 3（P0-17 语义）。
            eprintln!("AI-RUNNER-DEMO SAFETY FATAL: {msg}");
            std::process::exit(3);
        }
        Err(RunnerError::DualBrain(DualBrainError::DispatchError(msg))) => {
            // 授权链拒绝（untrusted 反馈/租约/策略）——demo 诚实失败，
            // 不是产品 PASS（证据类别 DEMO_ONLY）。
            eprintln!("AI-RUNNER-DEMO chain denied: {msg}");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("AI-RUNNER-DEMO error: {e}");
            std::process::exit(1);
        }
    }
}
