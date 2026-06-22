//! EnerOS HA Daemon - 高可用守护进程
//!
//! 作为 HA 模块的运行时基座，加载 HaConfig 并运行：
//! - HeartbeatManager: 心跳检测
//! - SyncManager: 状态同步
//! - SharedStore: 共享状态存储（含持久化）
//! - FencingManager: 脑裂防护
//! - FailoverEngine: 热备切换引擎
//! - ClusterManager (可选): 多节点集群
//! - DrillScheduler (可选): 灾备演练
//!
//! 提供 IPC 控制通道（TCP 127.0.0.1:5402，JSON 行协议）供 enerosctl 查询状态。
//!
//! ## 启动流程
//! 1. 解析命令行参数（clap）
//! 2. 初始化 tracing
//! 3. 加载 HaConfig（支持 ENEROS_HA_CONFIG 环境变量覆盖）
//! 4. 创建 SharedStore（含持久化）并 load_from_disk
//! 5. 创建 HeartbeatManager + SyncManager + FencingManager + FailoverEngine
//! 6. （可选）创建 ClusterManager + DrillScheduler
//! 7. 启动心跳循环线程
//! 8. 启动同步循环线程
//! 9. 启动 IPC 控制通道（tokio::spawn）
//! 10. 等待 SIGTERM/SIGINT → 优雅关闭

use anyhow::{Context, Result};
use clap::Parser;
use eneros_os::ha::{
    ClusterManager, ConflictResolution, DrillScenario, DrillScheduler, FailoverEngine,
    FailoverState, FencingManager, HaConfig, HeartbeatManager, NodeState, NodeStateChange,
    SharedStore, SplitBrainConfig, StorageQuota, SyncManager, SyncStatus,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// IPC 控制通道监听端口
const IPC_PORT: u16 = 5402;

/// 默认配置文件路径
const DEFAULT_CONFIG_PATH: &str = "/etc/eneros/ha.toml";

/// 同步循环空闲时的睡眠时间（毫秒）
const SYNC_IDLE_SLEEP_MS: u64 = 10;

/// 同步循环错误后的退避时间（毫秒）
const SYNC_ERROR_BACKOFF_MS: u64 = 100;

// ============================================================================
// 命令行参数
// ============================================================================

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "eneros-ha", version, about = "EnerOS HA Daemon - 高可用守护进程")]
struct Args {
    /// 配置文件路径（默认 /etc/eneros/ha.toml，可通过 ENEROS_HA_CONFIG 环境变量覆盖）
    #[arg(long)]
    config: Option<String>,

    /// 启用详细日志（debug 级别）
    #[arg(long, short = 'v')]
    verbose: bool,
}

// ============================================================================
// IPC 协议
// ============================================================================

/// IPC 请求（JSON 行协议）
#[derive(Debug, Serialize, Deserialize)]
struct IpcRequest {
    command: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// IPC 响应（JSON 行协议）
#[derive(Debug, Serialize, Deserialize)]
struct IpcResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl IpcResponse {
    /// 成功响应
    fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    /// 错误响应
    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

// ============================================================================
// HA 运行时状态
// ============================================================================

/// HA 守护进程运行时状态
///
/// 持有所有 HA 组件的 Arc 引用，供心跳循环、同步循环、IPC 处理共享访问。
struct HaState {
    config: HaConfig,
    store: Arc<SharedStore>,
    heartbeat: Arc<HeartbeatManager>,
    sync: Arc<SyncManager>,
    #[allow(dead_code)]
    fencing: Arc<FencingManager>,
    failover: Arc<FailoverEngine>,
    #[allow(dead_code)]
    cluster: Option<Arc<ClusterManager>>,
    drill: Option<Arc<DrillScheduler>>,
}

/// 获取持久化路径（snapshot_path, wal_path）
///
/// Linux: /var/lib/eneros/ha/
/// 非 Linux: 临时目录/eneros-ha/
fn persistence_paths() -> (PathBuf, PathBuf) {
    #[cfg(target_os = "linux")]
    {
        let base = PathBuf::from("/var/lib/eneros/ha");
        (base.join("snapshot.json"), base.join("wal.log"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let base = std::env::temp_dir().join("eneros-ha");
        (base.join("snapshot.json"), base.join("wal.log"))
    }
}

/// 构建 HA 状态
///
/// 创建所有 HA 组件并加载持久化状态。组件创建顺序：
/// SharedStore → HeartbeatManager → SyncManager → FencingManager → FailoverEngine
/// → ClusterManager（可选）→ DrillScheduler（可选）
fn build_state(config: HaConfig) -> Result<HaState> {
    // 创建 SharedStore（含持久化）
    let (snapshot_path, wal_path) = persistence_paths();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建持久化目录失败: {:?}", parent))?;
    }
    let store = SharedStore::new(
        config.node_id.clone(),
        config.role,
        ConflictResolution::default(),
        StorageQuota::default(),
    )
    .with_persistence(&snapshot_path, &wal_path);

    // 从磁盘加载持久化状态（失败时以空状态继续）
    if let Err(e) = store.load_from_disk() {
        tracing::warn!(error = %e, "加载持久化状态失败（继续以空状态启动）");
    }
    let store = Arc::new(store);

    // 创建 HeartbeatManager
    let heartbeat = HeartbeatManager::new(config.clone()).context("创建 HeartbeatManager 失败")?;
    let heartbeat = Arc::new(heartbeat);

    // 创建 SyncManager
    let sync = SyncManager::new(config.clone(), Some(store.clone()))
        .context("创建 SyncManager 失败")?;
    let sync = Arc::new(sync);

    // 创建 FencingManager
    // SplitBrainConfig 从 HaConfig 派生：心跳超时取 dead_ms，仲裁节点取 cluster.witness
    let split_brain_config = SplitBrainConfig {
        heartbeat_timeout_ms: config.heartbeat_dead_ms,
        quorum_nodes: config
            .cluster
            .as_ref()
            .map(|c| c.witness.clone())
            .unwrap_or_default(),
        quorum_timeout_ms: 1000,
    };
    let fencing = FencingManager::new(
        config.fencing_strategy,
        config.node_id.clone(),
        config.role,
        split_brain_config,
    );
    let fencing = Arc::new(fencing);

    // 创建 FailoverEngine
    let failover_config = config.failover.clone().unwrap_or_default();
    let failover = FailoverEngine::new(config.clone(), store.clone(), failover_config)
        .with_sync_manager(sync.clone());
    let failover = Arc::new(failover);

    // 创建 ClusterManager（可选，依赖 config.cluster）
    let cluster = config
        .cluster
        .as_ref()
        .map(|c| Arc::new(ClusterManager::new(c.clone(), config.node_id.clone())));

    // 创建 DrillScheduler（可选，依赖 config.drill）
    let drill = config.drill.as_ref().map(|d| {
        Arc::new(DrillScheduler::new(
            d.clone(),
            failover.clone(),
            config.node_id.clone(),
        ))
    });

    Ok(HaState {
        config,
        store,
        heartbeat,
        sync,
        fencing,
        failover,
        cluster,
        drill,
    })
}

// ============================================================================
// 心跳循环
// ============================================================================

/// 心跳循环（在独立线程中运行）
///
/// 自定义循环（不使用 HeartbeatManager::run）以便捕获 NodeStateChange
/// 并传递给 FailoverEngine::on_node_state_change。
fn heartbeat_loop(
    heartbeat: Arc<HeartbeatManager>,
    failover: Arc<FailoverEngine>,
    shutdown: Arc<AtomicBool>,
    interval: std::time::Duration,
) {
    tracing::info!("心跳循环启动，间隔 {:?}", interval);
    while !shutdown.load(Ordering::SeqCst) {
        // 发送心跳（非 Linux 平台返回 UnsupportedPlatform，仅 debug 日志）
        if let Err(e) = heartbeat.send_heartbeat() {
            tracing::debug!(error = %e, "发送心跳失败");
        }
        // 接收心跳（非阻塞，循环接收所有待处理包）
        while let Ok(Some(_)) = heartbeat.receive_heartbeat() {}
        // 检查超时并通知 FailoverEngine
        let changes: Vec<NodeStateChange> = heartbeat.check_timeouts();
        for change in changes {
            tracing::info!(
                node_id = %change.node_id,
                old = ?change.old_state,
                new = ?change.new_state,
                "节点状态变更"
            );
            failover.on_node_state_change(&change);
        }
        std::thread::sleep(interval);
    }
    tracing::info!("心跳循环退出");
}

// ============================================================================
// 同步循环
// ============================================================================

/// 同步循环（在独立线程中运行）
///
/// 接收对端同步消息并处理，排空待发送队列。
/// 非 Linux 平台 receive_message 返回 UnsupportedPlatform，循环以退避策略继续。
fn sync_loop(sync: Arc<SyncManager>, shutdown: Arc<AtomicBool>) {
    tracing::info!("同步循环启动");
    while !shutdown.load(Ordering::SeqCst) {
        match sync.receive_message() {
            Ok(Some(msg)) => {
                if let Err(e) = sync.process_message(&msg) {
                    tracing::warn!(error = %e, "处理同步消息失败");
                }
            }
            Ok(None) => {
                // 无消息，短暂睡眠避免空转
                std::thread::sleep(std::time::Duration::from_millis(SYNC_IDLE_SLEEP_MS));
            }
            Err(e) => {
                // 非 Linux 平台返回 UnsupportedPlatform，以较长间隔退避
                tracing::debug!(error = %e, "接收同步消息失败");
                std::thread::sleep(std::time::Duration::from_millis(SYNC_ERROR_BACKOFF_MS));
            }
        }
        // 排空待发送队列（非 Linux 平台无实际网络发送，仅清理队列）
        let _ = sync.drain_pending();
    }
    tracing::info!("同步循环退出");
}

// ============================================================================
// IPC 控制通道
// ============================================================================

/// 处理单个 IPC 连接
///
/// JSON 行协议：每行一个 JSON 请求，返回一行 JSON 响应。
/// 连接保持打开，直到客户端断开或读取/写入失败。
async fn handle_ipc_connection(stream: tokio::net::TcpStream, state: Arc<HaState>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let peer = stream.peer_addr().ok();
    let (read_half, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF，客户端断开
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(error = %e, ?peer, "IPC 读取失败");
                break;
            }
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<IpcRequest>(line) {
            Ok(req) => {
                tracing::debug!(command = %req.command, ?peer, "IPC 请求");
                dispatch_command(&state, &req)
            }
            Err(e) => {
                tracing::warn!(error = %e, ?peer, "IPC 请求解析失败");
                IpcResponse::err(format!("请求解析失败: {}", e))
            }
        };

        let resp_line = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "序列化响应失败");
                break;
            }
        };
        if let Err(e) = writer.write_all(resp_line.as_bytes()).await {
            tracing::debug!(error = %e, ?peer, "IPC 写入失败");
            break;
        }
        if let Err(e) = writer.write_all(b"\n").await {
            tracing::debug!(error = %e, ?peer, "IPC 写入换行失败");
            break;
        }
    }
}

/// 分发 IPC 命令
fn dispatch_command(state: &HaState, req: &IpcRequest) -> IpcResponse {
    match req.command.as_str() {
        "ha_status" => cmd_ha_status(state),
        "ha_nodes" => cmd_ha_nodes(state),
        "ha_sync_status" => cmd_ha_sync_status(state),
        "failover_status" => cmd_failover_status(state),
        "failover_trigger" => cmd_failover_trigger(state, req),
        "failover_history" => cmd_failover_history(state),
        "failover_drill" => cmd_failover_drill(state, req),
        other => IpcResponse::err(format!("未知命令: {}", other)),
    }
}

/// ha_status 命令：返回本节点 HA 概览状态
fn cmd_ha_status(state: &HaState) -> IpcResponse {
    let nodes = state.heartbeat.list_nodes();
    let sync_status = state.sync.status();
    let failover_status = state.failover.status();
    let data = serde_json::json!({
        "node_id": state.heartbeat.local_node_id(),
        "role": state.heartbeat.local_role().as_str(),
        "priority": state.config.priority,
        "peer_count": nodes.len(),
        "sync_connected": sync_status.is_connected,
        "failover_state": failover_status.current_state.as_str(),
        "is_readonly": state.store.is_readonly(),
    });
    IpcResponse::ok(data)
}

/// ha_nodes 命令：返回集群节点列表
///
/// NodeInfo 含 Instant 字段（不可序列化），手动构造 JSON。
fn cmd_ha_nodes(state: &HaState) -> IpcResponse {
    let nodes = state.heartbeat.list_nodes();
    let arr: Vec<serde_json::Value> = nodes
        .iter()
        .map(|n| {
            serde_json::json!({
                "node_id": n.node_id,
                "role": n.role.as_str(),
                "state": match n.state {
                    NodeState::Alive => "alive",
                    NodeState::Suspect => "suspect",
                    NodeState::Dead => "dead",
                },
                "priority": n.priority,
                "last_seq": n.last_seq,
                "epoch": n.epoch,
            })
        })
        .collect();
    IpcResponse::ok(serde_json::Value::Array(arr))
}

/// ha_sync_status 命令：返回同步状态
fn cmd_ha_sync_status(state: &HaState) -> IpcResponse {
    let status: SyncStatus = state.sync.status();
    IpcResponse::ok(serde_json::to_value(status).unwrap_or(serde_json::Value::Null))
}

/// failover_status 命令：返回 failover 状态机状态
fn cmd_failover_status(state: &HaState) -> IpcResponse {
    let status = state.failover.status();
    IpcResponse::ok(serde_json::to_value(status).unwrap_or(serde_json::Value::Null))
}

/// failover_trigger 命令：手动触发 failover
fn cmd_failover_trigger(state: &HaState, req: &IpcRequest) -> IpcResponse {
    let reason = req
        .args
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("manual trigger");
    match state.failover.trigger_failover(reason) {
        Ok(record) => IpcResponse::ok(
            serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => IpcResponse::err(format!("failover 触发失败: {}", e)),
    }
}

/// failover_history 命令：返回切换历史
fn cmd_failover_history(state: &HaState) -> IpcResponse {
    let history = state.failover.history();
    IpcResponse::ok(serde_json::to_value(history).unwrap_or(serde_json::Value::Null))
}

/// failover_drill 命令：触发灾备演练
fn cmd_failover_drill(state: &HaState, req: &IpcRequest) -> IpcResponse {
    let drill = match &state.drill {
        Some(d) => d,
        None => return IpcResponse::err("灾备演练未配置（ha.toml 中 [drill] 段缺失）"),
    };
    let scenario_str = match req.args.get("scenario").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return IpcResponse::err("缺少 scenario 参数"),
    };
    let scenario = match scenario_str {
        "primary_down" => DrillScenario::PrimaryDown,
        "network_partition" => DrillScenario::NetworkPartition,
        "disk_failure" => DrillScenario::DiskFailure,
        other => return IpcResponse::err(format!("未知 scenario: {}", other)),
    };
    match drill.run_drill_manual(scenario) {
        Ok(result) => IpcResponse::ok(
            serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => IpcResponse::err(format!("演练失败: {}", e)),
    }
}

// ============================================================================
// 信号处理
// ============================================================================

/// 等待关闭信号（SIGTERM/SIGINT on Unix, Ctrl-C on Windows）
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("收到 SIGTERM，开始优雅关闭"),
            _ = sigint.recv() => tracing::info!("收到 SIGINT，开始优雅关闭"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("收到 Ctrl-C，开始优雅关闭");
    }
}

// ============================================================================
// 优雅关闭
// ============================================================================

/// 优雅关闭：持久化快照 → 释放 VIP（如果是主节点）
fn graceful_shutdown(state: &HaState) {
    tracing::info!("开始优雅关闭");

    // 1. 持久化 SharedStore 快照
    if let Err(e) = state.store.snapshot() {
        tracing::warn!(error = %e, "持久化快照失败");
    } else {
        tracing::info!("SharedStore 快照已持久化");
    }

    // 2. 如果是主节点（Active 状态），释放 VIP
    if state.failover.current_state() == FailoverState::Active {
        if let Err(e) = state.failover.release_vip() {
            tracing::warn!(error = %e, "释放 VIP 失败");
        } else {
            tracing::info!("VIP 已释放");
        }
    }

    tracing::info!("优雅关闭完成");
}

// ============================================================================
// 主函数
// ============================================================================

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    // 初始化 tracing
    let filter = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .init();

    tracing::info!("EnerOS HA Daemon 启动 (pid={})", std::process::id());

    // 解析配置路径：CLI --config > ENEROS_HA_CONFIG 环境变量 > 默认路径
    let config_path = args
        .config
        .or_else(|| std::env::var("ENEROS_HA_CONFIG").ok())
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
    tracing::info!("加载 HA 配置: {}", config_path);

    let config = match HaConfig::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, path = %config_path, "加载 HA 配置失败");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        node_id = %config.node_id,
        role = ?config.role,
        heartbeat_interval_ms = config.heartbeat_interval_ms,
        "HA 配置加载完成"
    );

    // 构建 HA 状态
    let state = match build_state(config) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "构建 HA 状态失败");
            return ExitCode::FAILURE;
        }
    };

    // 创建 shutdown flag（共享给心跳/同步/IPC）
    let shutdown = Arc::new(AtomicBool::new(false));

    // 启动心跳循环线程（克隆 Arc 引用，不移动 state）
    let heartbeat_interval = state.config.heartbeat_interval();
    let heartbeat_handle = {
        let hb = state.heartbeat.clone();
        let fo = state.failover.clone();
        let sd = shutdown.clone();
        std::thread::spawn(move || heartbeat_loop(hb, fo, sd, heartbeat_interval))
    };

    // 启动同步循环线程
    let sync_handle = {
        let sync = state.sync.clone();
        let sd = shutdown.clone();
        std::thread::spawn(move || sync_loop(sync, sd))
    };

    // 启动 IPC 控制通道
    let ipc_addr = format!("127.0.0.1:{}", IPC_PORT);
    let listener = match tokio::net::TcpListener::bind(&ipc_addr).await {
        Ok(l) => {
            tracing::info!("IPC 控制通道监听: {}", ipc_addr);
            l
        }
        Err(e) => {
            tracing::error!(error = %e, addr = %ipc_addr, "IPC 监听失败");
            shutdown.store(true, Ordering::SeqCst);
            let _ = heartbeat_handle.join();
            let _ = sync_handle.join();
            return ExitCode::FAILURE;
        }
    };

    // 将 state 包入 Arc 供 IPC 任务和主线程共享
    let state = Arc::new(state);
    let ipc_state = state.clone();
    let ipc_task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let st = ipc_state.clone();
                    tokio::spawn(async move {
                        handle_ipc_connection(stream, st).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "IPC accept 失败");
                    break;
                }
            }
        }
    });

    // 等待关闭信号
    wait_for_signal().await;

    // 优雅关闭流程：
    // 1. 设置 shutdown flag（通知心跳/同步循环退出）
    // 2. 停止 IPC 任务（不再接受新请求）
    // 3. 等待心跳/同步线程退出
    // 4. 持久化快照 + 释放 VIP
    shutdown.store(true, Ordering::SeqCst);
    ipc_task.abort();
    let _ = heartbeat_handle.join();
    let _ = sync_handle.join();
    graceful_shutdown(&state);

    tracing::info!("EnerOS HA Daemon 已退出");
    ExitCode::SUCCESS
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_os::ha::{DrillConfig, FencingStrategy, NodeRole, SyncScope};

    /// 构造测试用 HaConfig（非生产环境，fencing_strategy = None）
    fn make_test_config() -> HaConfig {
        HaConfig {
            node_id: "test-node".to_string(),
            role: NodeRole::Primary,
            heartbeat_interval_ms: 100,
            heartbeat_suspect_ms: 100,
            heartbeat_dead_ms: 300,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 5401,
            interfaces: vec![],
            priority: 100,
            fencing_strategy: FencingStrategy::None,
            sync_scope: SyncScope::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        }
    }

    /// 构造测试用 HaState（无 cluster/drill 配置）
    fn make_test_state() -> HaState {
        let config = make_test_config();
        let store = Arc::new(SharedStore::new(
            config.node_id.clone(),
            config.role,
            ConflictResolution::default(),
            StorageQuota::default(),
        ));
        let heartbeat = Arc::new(HeartbeatManager::new(config.clone()).unwrap());
        let sync = Arc::new(SyncManager::new(config.clone(), Some(store.clone())).unwrap());
        let fencing = Arc::new(FencingManager::new(
            config.fencing_strategy,
            config.node_id.clone(),
            config.role,
            SplitBrainConfig::default(),
        ));
        let failover = Arc::new(
            FailoverEngine::new(config.clone(), store.clone(), Default::default())
                .with_sync_manager(sync.clone()),
        );
        HaState {
            config,
            store,
            heartbeat,
            sync,
            fencing,
            failover,
            cluster: None,
            drill: None,
        }
    }

    // --- IPC 请求解析测试 ---

    #[test]
    fn test_ipc_request_parse_with_args() {
        let json = r#"{"command":"ha_status","args":{}}"#;
        let req: IpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.command, "ha_status");
    }

    #[test]
    fn test_ipc_request_parse_without_args() {
        // 缺少 args 字段时使用 serde default（Null）
        let json = r#"{"command":"ha_nodes"}"#;
        let req: IpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.command, "ha_nodes");
        assert!(req.args.is_null());
    }

    #[test]
    fn test_ipc_request_parse_failover_trigger() {
        let json = r#"{"command":"failover_trigger","args":{"reason":"manual"}}"#;
        let req: IpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.command, "failover_trigger");
        assert_eq!(req.args["reason"], "manual");
    }

    #[test]
    fn test_ipc_request_parse_invalid_json() {
        let json = r#"not a json"#;
        let result: Result<IpcRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // --- IPC 响应构造测试 ---

    #[test]
    fn test_ipc_response_ok_serialize() {
        let resp = IpcResponse::ok(serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"data\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_ipc_response_err_serialize() {
        let resp = IpcResponse::err("测试错误");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"error\":\"测试错误\""));
        assert!(!json.contains("\"data\""));
    }

    #[test]
    fn test_ipc_response_roundtrip() {
        let resp = IpcResponse::ok(serde_json::json!({"state": "active"}));
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: IpcResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.data.unwrap()["state"], "active");
    }

    // --- 命令分发测试 ---

    #[test]
    fn test_dispatch_unknown_command() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "unknown_cmd".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("未知命令"));
    }

    #[test]
    fn test_dispatch_ha_status() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "ha_status".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert_eq!(data["node_id"], "test-node");
        assert_eq!(data["role"], "primary");
        assert_eq!(data["failover_state"], "standby");
    }

    #[test]
    fn test_dispatch_ha_nodes_empty() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "ha_nodes".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert!(data.is_array());
        assert_eq!(data.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_dispatch_ha_sync_status() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "ha_sync_status".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert!(data["is_connected"].is_boolean());
    }

    #[test]
    fn test_dispatch_failover_status() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "failover_status".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert_eq!(data["current_state"], "standby");
    }

    #[test]
    fn test_dispatch_failover_history_empty() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "failover_history".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert!(data.is_array());
        assert_eq!(data.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_dispatch_failover_drill_no_config() {
        let state = make_test_state();
        let req = IpcRequest {
            command: "failover_drill".to_string(),
            args: serde_json::json!({"scenario": "primary_down"}),
        };
        let resp = dispatch_command(&state, &req);
        // 无 drill 配置时应返回错误
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("灾备演练未配置"));
    }

    #[test]
    fn test_dispatch_failover_drill_missing_scenario() {
        // 构造带 drill 配置的 state
        let mut config = make_test_config();
        config.drill = Some(DrillConfig::default());
        let store = Arc::new(SharedStore::new(
            config.node_id.clone(),
            config.role,
            ConflictResolution::default(),
            StorageQuota::default(),
        ));
        let heartbeat = Arc::new(HeartbeatManager::new(config.clone()).unwrap());
        let sync = Arc::new(SyncManager::new(config.clone(), Some(store.clone())).unwrap());
        let fencing = Arc::new(FencingManager::new(
            config.fencing_strategy,
            config.node_id.clone(),
            config.role,
            SplitBrainConfig::default(),
        ));
        let failover = Arc::new(
            FailoverEngine::new(config.clone(), store.clone(), Default::default())
                .with_sync_manager(sync.clone()),
        );
        let drill = Arc::new(DrillScheduler::new(
            config.drill.clone().unwrap(),
            failover.clone(),
            config.node_id.clone(),
        ));
        let state = HaState {
            config,
            store,
            heartbeat,
            sync,
            fencing,
            failover,
            cluster: None,
            drill: Some(drill),
        };

        let req = IpcRequest {
            command: "failover_drill".to_string(),
            args: serde_json::Value::Null,
        };
        let resp = dispatch_command(&state, &req);
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("缺少 scenario"));
    }

    #[test]
    fn test_dispatch_failover_drill_unknown_scenario() {
        let mut config = make_test_config();
        config.drill = Some(DrillConfig::default());
        let store = Arc::new(SharedStore::new(
            config.node_id.clone(),
            config.role,
            ConflictResolution::default(),
            StorageQuota::default(),
        ));
        let heartbeat = Arc::new(HeartbeatManager::new(config.clone()).unwrap());
        let sync = Arc::new(SyncManager::new(config.clone(), Some(store.clone())).unwrap());
        let fencing = Arc::new(FencingManager::new(
            config.fencing_strategy,
            config.node_id.clone(),
            config.role,
            SplitBrainConfig::default(),
        ));
        let failover = Arc::new(
            FailoverEngine::new(config.clone(), store.clone(), Default::default())
                .with_sync_manager(sync.clone()),
        );
        let drill = Arc::new(DrillScheduler::new(
            config.drill.clone().unwrap(),
            failover.clone(),
            config.node_id.clone(),
        ));
        let state = HaState {
            config,
            store,
            heartbeat,
            sync,
            fencing,
            failover,
            cluster: None,
            drill: Some(drill),
        };

        let req = IpcRequest {
            command: "failover_drill".to_string(),
            args: serde_json::json!({"scenario": "unknown_scenario"}),
        };
        let resp = dispatch_command(&state, &req);
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("未知 scenario"));
    }

    // --- 配置加载错误测试 ---

    #[test]
    fn test_config_load_nonexistent_file() {
        let result = HaConfig::load("/nonexistent/path/ha.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_load_invalid_toml() {
        let tmp = std::env::temp_dir().join("eneros-ha-test-invalid.toml");
        std::fs::write(&tmp, "invalid toml content = = =").unwrap();
        let result = HaConfig::load(&tmp);
        assert!(result.is_err());
        let _ = std::fs::remove_file(&tmp);
    }

    // --- 持久化路径测试 ---

    #[test]
    fn test_persistence_paths() {
        let (snapshot, wal) = persistence_paths();
        assert!(snapshot.to_string_lossy().contains("snapshot.json"));
        assert!(wal.to_string_lossy().contains("wal.log"));
    }

    // --- build_state 测试 ---

    #[test]
    fn test_build_state_success() {
        let config = make_test_config();
        let result = build_state(config);
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.heartbeat.local_node_id(), "test-node");
        assert_eq!(state.heartbeat.local_role(), NodeRole::Primary);
    }

    #[test]
    fn test_build_state_with_failover_config() {
        let mut config = make_test_config();
        config.failover = Some(eneros_os::ha::FailoverConfig::default());
        let result = build_state(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_state_with_drill_config() {
        let mut config = make_test_config();
        config.drill = Some(DrillConfig::default());
        let result = build_state(config);
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.drill.is_some());
    }
}
