//! EnerOS EventBus Broker — 独立进程
//!
//! 作为系统服务运行，提供跨进程的事件发布/订阅服务。
//!
//! 用法：
//!   eneros-broker [--bind 127.0.0.1:9876] [--socket /var/run/eneros/broker.sock]

use clap::Parser;
use eneros_eventbus::{BrokerConfig, EventBusBroker};
use std::process::ExitCode;

/// EnerOS EventBus Broker 独立进程
#[derive(Parser, Debug)]
#[command(name = "eneros-broker", version, about = "EnerOS EventBus Broker")]
struct Args {
    /// TCP 绑定地址
    #[arg(long, default_value = "127.0.0.1:9876")]
    bind: String,

    /// Unix socket 路径（可选，仅 Unix 平台）
    #[arg(long)]
    socket: Option<String>,

    /// broadcast channel 容量
    #[arg(long, default_value_t = 1024)]
    channel_capacity: usize,

    /// 最大订阅者数
    #[arg(long, default_value_t = 256)]
    max_subscribers: usize,

    /// 详细日志
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    // 初始化日志
    let filter = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .init();

    tracing::info!("EnerOS EventBus Broker starting");
    tracing::info!("  TCP bind: {}", args.bind);
    #[cfg(unix)]
    if let Some(ref socket) = args.socket {
        tracing::info!("  Unix socket: {}", socket);
    }
    tracing::info!("  Channel capacity: {}", args.channel_capacity);
    tracing::info!("  Max subscribers: {}", args.max_subscribers);

    let config = BrokerConfig {
        tcp_addr: args.bind.clone(),
        #[cfg(unix)]
        unix_socket: args.socket.clone(),
        #[cfg(not(unix))]
        unix_socket: None,
        channel_capacity: args.channel_capacity,
        max_subscribers: args.max_subscribers,
    };

    let broker = EventBusBroker::new(config);

    // Ctrl+C 优雅关闭
    let broker_handle = tokio::spawn(async move {
        if let Err(e) = broker.run().await {
            tracing::error!("Broker error: {}", e);
        }
    });

    tokio::select! {
        _ = broker_handle => {
            tracing::info!("Broker task exited");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received, shutting down...");
        }
    }

    tracing::info!("EnerOS EventBus Broker stopped");
    ExitCode::SUCCESS
}
