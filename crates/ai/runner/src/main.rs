//! EnerOS AI 决策链 release 端到端入口（21-B A1）。
//!
//! 装配真实后端：LlamaCppEngine（llama.cpp）+ HighsSolver（HiGHS）+
//! SafetyValidator + FileCommandSink。生产路径零 mock。
//!
//! 用法：
//! ```text
//! eneros-ai-runner --model <path.gguf> --sink <path.jsonl> [--scenario normal|fatal-soc]
//!                 [--now-ms <u64>] [--soc-min <0.0-1.0>]
//! ```
//!
//! 退出码：
//! - 0：端到端成功且 sink 写入命令
//! - 1：装配/执行失败（LLM 加载、HiGHS 初始化、sink 打开、链路错误）
//! - 3：Fatal 安全违规阻断（sink 零写入——P0-17 语义）

use eneros_ai_runner::cli::CliArgs;
use eneros_ai_runner::scenario::Overrides;
use eneros_ai_runner::{run_once, RunnerError};
use eneros_dual_brain::DualBrainError;

/// 输出端到端证据 JSON（stdout；A3 落盘复用此结构）.
fn print_evidence(
    args: &CliArgs,
    outcome: &eneros_ai_runner::RunnerOutcome,
) -> Result<(), RunnerError> {
    let entry = outcome
        .result
        .schedule
        .schedule
        .first()
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
        "schema": "eneros.ai-runner.evidence.v1",
        "scenario": format!("{:?}", args.scenario),
        "scenario_description": args.scenario.description(),
        "now_ms": args.now_ms,
        "model": args.model,
        "sink": args.sink,
        "path_type": format!("{:?}", outcome.result.path_type),
        "prompt": outcome.prompt,
        "llm_output": outcome.llm_output,
        "sink_written": outcome.sink_written,
        "schedule": {
            "first_entry": entry,
            "entries": outcome.result.schedule.schedule.len(),
            "total_revenue_yuan": outcome.result.schedule.total_revenue_yuan,
            "solve_status": format!("{:?}", outcome.result.schedule.solve_status),
        },
        "latency_ms": {
            "perception": outcome.result.latency.perception_ms,
            "llm_inference": outcome.result.latency.llm_inference_ms,
            "intent_parse": outcome.result.latency.intent_parse_ms,
            "lp_build": outcome.result.latency.lp_build_ms,
            "lp_solve": outcome.result.latency.lp_solve_ms,
            "safety_validate": outcome.result.latency.safety_validate_ms,
            "command_dispatch": outcome.result.latency.command_dispatch_ms,
            "total": outcome.result.latency.total_ms,
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
            eprintln!("eneros-ai-runner: {msg}");
            std::process::exit(2);
        }
    };

    // FIX-24.R3：演示/证据路径无真实遥测可接 → 注入显式"未知"反馈语义
    // （untrusted → CTL-010 fail-safe deny）。偏差登记见蓝图/24 修订记录 R3；
    // 清偿条件：遥测源接入后注入真实 bundle。
    match run_once(
        args.scenario,
        Overrides {
            soc_min: args.soc_min,
        },
        &args.model,
        &args.sink,
        args.now_ms,
        eneros_control_gateway::chain::FeedbackBundle::untrusted,
        &args.chain_key,
    ) {
        Ok(outcome) => {
            // stdout 只输出证据 JSON（llama 日志在 stderr）——证据文件可
            // 直接被 `cargo xtask evidence` 类工具解析（A3）。
            if outcome.sink_written == 0 {
                // 链路成功但零命令：正常场景不应发生（命令下发是 D6 验收点）。
                eprintln!("AI-RUNNER FATAL: sink wrote 0 commands on normal path");
                std::process::exit(3);
            }
            if let Err(e) = print_evidence(&args, &outcome) {
                eprintln!("AI-RUNNER evidence error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Err(RunnerError::DualBrain(DualBrainError::SafetyFatal(msg))) => {
            // Fatal 安全违规：阻断 sink（零写入），退出码 3（P0-17 语义）。
            // stderr 输出 Fatal 证据 JSON（含 LLM 输出诊断，见 run_once）。
            eprintln!("AI-RUNNER SAFETY FATAL: {msg}");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("AI-RUNNER error: {e}");
            std::process::exit(1);
        }
    }
}
