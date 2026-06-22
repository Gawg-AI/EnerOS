//! EnerOS 端到端测试框架
//!
//! 提供集群启动器和测试场景，用于验证 EnerOS 各组件在真实进程级
//! 部署下的端到端行为。
//!
//! ## 架构
//!
//! - [`cluster`] — 集群启动器，管理 API/Gateway/Broker 进程的生命周期
//! - [`scenarios`] — 端到端测试场景集合
//! - [`chaos_scenarios`] — 混沌工程测试场景集合
//!
//! ## 用法
//!
//! ```no_run
//! use eneros_e2e_tests::cluster::{TestCluster, ClusterConfig};
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let mut cluster = TestCluster::start(ClusterConfig::default()).await?;
//! // 执行测试...
//! cluster.shutdown().await;
//! # Ok::<(), anyhow::Error>(()) });
//! ```

pub mod chaos_scenarios;
pub mod cluster;
pub mod scenarios;
