//! EnerOS Operation Agent — 独立进程
//!
//! 运维 Agent，负责故障诊断、设备健康评估、维护建议。
//! 作为独立 OS 进程运行，通过 EventBusBroker 与其他 Agent 通信，
//! 通过 GatewayServer 执行控制命令。
//!
//! 用法：
//!   eneros-operation-agent [--agent-id operation-1] [--eventbus-addr 127.0.0.1:9876] \
//!                          [--gateway-addr 127.0.0.1:9877] [--tick-interval-ms 1000] \
//!                          [--config path/to/config.json]

use async_trait::async_trait;
use clap::Parser;
use eneros_agent::{Agent, AgentConfig, AgentProcess, AgentType, OperationAgent};
use eneros_core::{AuthorityLevel, Jurisdiction};

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "eneros-operation-agent", version, about = "EnerOS Operation Agent process")]
struct Args {
    /// Agent ID
    #[arg(long, default_value = "operation-1")]
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

/// OperationAgent 进程入口
struct OperationAgentProcess {
    agent_id: String,
}

#[async_trait]
impl AgentProcess for OperationAgentProcess {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Operator
    }

    async fn create_agent(&self, config: &AgentConfig) -> anyhow::Result<Box<dyn Agent>> {
        // 使用默认参数构造 OperationAgent；内置故障模式库由构造器自动加载。
        // 域算法（故障诊断、设备健康评估）保持不变。
        let agent = OperationAgent::new(&config.agent_id, "Operation Agent", Vec::new());
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
            agent_type: AgentType::Operator,
            authority: AuthorityLevel::Operator,
            jurisdiction: Jurisdiction::unrestricted(),
            tick_interval_ms: args.tick_interval_ms,
            eventbus_addr: args.eventbus_addr,
            gateway_addr: args.gateway_addr,
            ipc_socket_dir: "/var/run/eneros".to_string(),
        }
    };

    // Use config.agent_id so --config file takes precedence over CLI default.
    let process = OperationAgentProcess {
        agent_id: config.agent_id.clone(),
    };
    process.run(config).await?;

    Ok(())
}
