//! EnerOS Trading Agent — 独立进程
//!
//! 交易 Agent，负责报价生成、风险评估、市场出清。
//! 作为独立 OS 进程运行，通过 EventBusBroker 与其他 Agent 通信，
//! 通过 GatewayServer 执行控制命令。
//!
//! 用法：
//!   eneros-trading-agent [--agent-id trading-1] [--eventbus-addr 127.0.0.1:9876] \
//!                        [--gateway-addr 127.0.0.1:9877] [--tick-interval-ms 1000] \
//!                        [--config path/to/config.json]

use async_trait::async_trait;
use clap::Parser;
use eneros_agent::{Agent, AgentConfig, AgentProcess, AgentType, TradingAgent};
use eneros_core::{AuthorityLevel, Jurisdiction};

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "eneros-trading-agent", version, about = "EnerOS Trading Agent process")]
struct Args {
    /// Agent ID
    #[arg(long, default_value = "trading-1")]
    agent_id: String,

    /// EventBus broker TCP 地址
    #[arg(long, default_value = "127.0.0.1:9876")]
    eventbus_addr: String,

    /// Gateway server TCP 地址
    #[arg(long, default_value = "127.0.0.1:9877")]
    gateway_addr: String,

    /// Tick 间隔（毫秒）
    #[arg(long, default_value_t = 1000)]
    tick_interval_ms: u64,

    /// 配置文件路径（JSON 格式，覆盖命令行参数）
    #[arg(long)]
    config: Option<String>,
}

/// TradingAgent 进程入口
struct TradingAgentProcess {
    agent_id: String,
}

#[async_trait]
impl AgentProcess for TradingAgentProcess {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Custom("Trading".to_string())
    }

    async fn create_agent(&self, config: &AgentConfig) -> anyhow::Result<Box<dyn Agent>> {
        // 使用默认参数构造 TradingAgent：
        // - markup_factor = 1.05
        // - risk_tolerance = 0.1
        // - gen_cost_curves 为空（运行时可通过事件注入）
        // 域算法（generate_bid、assess_risk）保持不变。
        let agent = TradingAgent::new(&config.agent_id, Vec::new());
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
            agent_type: AgentType::Custom("Trading".to_string()),
            authority: AuthorityLevel::Operator,
            jurisdiction: Jurisdiction::unrestricted(),
            tick_interval_ms: args.tick_interval_ms,
            eventbus_addr: args.eventbus_addr,
            gateway_addr: args.gateway_addr,
            ipc_socket_dir: "/var/run/eneros".to_string(),
        }
    };

    // Use config.agent_id so --config file takes precedence over CLI default.
    let process = TradingAgentProcess {
        agent_id: config.agent_id.clone(),
    };
    process.run(config).await?;

    Ok(())
}
