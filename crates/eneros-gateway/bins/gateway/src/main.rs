//! EnerOS Gateway — 独立 SafetyGateway 进程（v0.16.0）
//!
//! 作为独立 OS 进程运行，通过 TCP IPC 暴露 `SafetyGateway` +
//! `ConstrainedDecisionPipeline` 服务给 Agent 进程。
//!
//! 用法：
//!   eneros-gateway [--bind 127.0.0.1:9870] [--max-history 100] [--log-level info]

use std::sync::Arc;

use clap::Parser;
use eneros_constraint::{ConstraintEngine, FeasibilityProjector};
use eneros_gateway::{
    client::LocalGatewayClient,
    constraint_validator::ConstraintAwareValidator,
    decision_pipeline::ConstrainedDecisionPipeline,
    gateway::SafetyGateway,
    server::GatewayServer,
};
use eneros_network::{NetworkSimulatorAdapter, PowerNetwork};
use tracing_subscriber::EnvFilter;

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "eneros-gateway", version, about = "EnerOS standalone SafetyGateway process")]
struct Args {
    /// IPC 服务端绑定地址
    #[arg(long, default_value = "127.0.0.1:9870")]
    bind: String,

    /// 命令历史最大长度
    #[arg(long, default_value_t = 100)]
    max_history: usize,

    /// 日志级别（trace, debug, info, warn, error）
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// 构建 Gateway 服务端栈：PowerNetwork → Simulator → Projector →
/// ConstraintEngine → SafetyGateway → Validator → Pipeline → Client → Server。
fn build_gateway_server(bind_addr: &str, max_history: usize) -> GatewayServer {
    let network = Arc::new(parking_lot::RwLock::new(PowerNetwork::from_ieee14()));
    let simulator = NetworkSimulatorAdapter::new(network);
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(simulator)));
    let constraint_engine = ConstraintEngine::new();
    let gateway = Arc::new(SafetyGateway::new(max_history));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        Arc::new(constraint_engine),
        gateway.clone(),
    ));
    let pipeline = ConstrainedDecisionPipeline::new(projector, validator, gateway.clone());
    let client = LocalGatewayClient::with_pipeline(gateway, Arc::new(pipeline));
    GatewayServer::new(client, bind_addr)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let filter = EnvFilter::new(&args.log_level);
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let server = build_gateway_server(&args.bind, args.max_history);

    tracing::info!("eneros-gateway starting on {}", args.bind);

    tokio::select! {
        result = server.run() => {
            if let Err(e) = result {
                tracing::error!("GatewayServer error: {}", e);
                return Err(e);
            }
        }
        _ = tokio::signal::ctrl_c() => {}
    }

    tracing::info!("eneros-gateway shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_args_default() {
        let args = Args::try_parse_from(["eneros-gateway"]).unwrap();
        assert_eq!(args.bind, "127.0.0.1:9870");
        assert_eq!(args.max_history, 100);
        assert_eq!(args.log_level, "info");
    }

    #[test]
    fn test_gateway_stack_construction() {
        let server = build_gateway_server("127.0.0.1:9870", 100);
        assert_eq!(server.addr(), "127.0.0.1:9870");
    }
}
