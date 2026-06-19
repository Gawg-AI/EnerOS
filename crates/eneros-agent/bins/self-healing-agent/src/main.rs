//! EnerOS Self-Healing Agent — 独立进程（RT 实时进程）
//!
//! 自愈 Agent，负责故障定位、隔离、恢复供电（FLISR）。
//! 作为独立 OS 进程运行，通过 EventBusBroker 与其他 Agent 通信，
//! 通过 GatewayServer 执行控制命令。
//!
//! # RT 调度说明
//!
//! NOTE: This agent should run as SCHED_FIFO real-time process.
//! The eneros-init will set RT scheduling policy via AgentScheduler.
//! No special code needed here - RT scheduling is applied externally.
//!
//! 用法：
//!   eneros-self-healing-agent [--agent-id self-healing-1] [--eventbus-addr 127.0.0.1:9876] \
//!                             [--gateway-addr 127.0.0.1:9877] [--tick-interval-ms 500] \
//!                             [--config path/to/config.json]

use async_trait::async_trait;
use clap::Parser;
use eneros_agent::{Agent, AgentConfig, AgentProcess, AgentType, SelfHealingAgent};
use eneros_core::{AuthorityLevel, Jurisdiction};

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "eneros-self-healing-agent", version, about = "EnerOS Self-Healing Agent process (RT)")]
struct Args {
    /// Agent ID
    #[arg(long, default_value = "self-healing-1")]
    agent_id: String,

    /// EventBus broker TCP 地址
    #[arg(long, default_value = "127.0.0.1:9876")]
    eventbus_addr: String,

    /// Gateway server TCP 地址
    #[arg(long, default_value = "127.0.0.1:9877")]
    gateway_addr: String,

    /// Tick 间隔（毫秒）— 自愈 Agent 默认 500ms 以保证快速响应
    #[arg(long, default_value_t = 500)]
    tick_interval_ms: u64,

    /// 配置文件路径（JSON 格式，覆盖命令行参数）
    #[arg(long)]
    config: Option<String>,
}

/// SelfHealingAgent 进程入口
struct SelfHealingAgentProcess {
    agent_id: String,
}

#[async_trait]
impl AgentProcess for SelfHealingAgentProcess {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Custom("SelfHealing".to_string())
    }

    async fn create_agent(&self, config: &AgentConfig) -> anyhow::Result<Box<dyn Agent>> {
        // 使用默认参数构造 SelfHealingAgent；interlocking_engine 与 device_states
        // 由构造器初始化为默认值。域算法（locate_fault_section、
        // generate_isolation_sequence、find_restoration_path）保持不变。
        let agent = SelfHealingAgent::new(&config.agent_id, "Self-Healing Agent", Vec::new());
        Ok(Box::new(agent))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let config = if let Some(path) = args.config {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)?
    } else {
        AgentConfig {
            agent_id: args.agent_id.clone(),
            agent_type: AgentType::Custom("SelfHealing".to_string()),
            authority: AuthorityLevel::Emergency,
            jurisdiction: Jurisdiction::unrestricted(),
            tick_interval_ms: args.tick_interval_ms,
            eventbus_addr: args.eventbus_addr,
            gateway_addr: args.gateway_addr,
            ipc_socket_dir: "/var/run/eneros".to_string(),
        }
    };

    // Use config.agent_id so --config file takes precedence over CLI default.
    let process = SelfHealingAgentProcess {
        agent_id: config.agent_id.clone(),
    };
    process.run(config).await?;

    Ok(())
}
