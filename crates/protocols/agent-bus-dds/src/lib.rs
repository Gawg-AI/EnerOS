//! EnerOS v0.77.0 DDS 中间件集成与 Rust 封装（Phase 2 P2-A 路由层）.
//!
//! Agent Bus 三层总线之一的 DDS 发布/订阅抽象。提供统一的 [`node::DdsNode`] trait、
//! [`mock::MockDdsNode`]（默认可用，纯 Rust）与 [`cyclone_dds::CycloneDdsNode`]
//!（feature = "cyclone-dds"，封装 Eclipse Cyclone DDS C 库），并在 v0.76.0 引入
//! 语义层：[`topic::TopicSpec`] / [`topic::TopicCategory`] / [`topic::standard_topics`]
//! / [`registry::TopicRegistry`] / 扩展的 [`qos::QosPolicy`]（含 deadline/lifespan/priority）。
//! v0.77.0 在语义层之上引入路由层：[`router::MessageRouter`] / [`policy::RoutingPolicy`]
//! / [`policy::CapabilityVerifier`] / [`router::Subscription`] / [`policy::RouteDecision`]
//! / [`policy::DropReason`]，支持 topic 通配匹配（`*` 后缀）、能力校验与优先级路由。
//! 为后续 v0.78.0（Agent 调度器）、v0.89.0（联邦消息总线）、v0.92.0（VPP 聚合）提供路由基础。
//! v0.78.0 在路由层之上引入签名层：[`codec::CodecKind`] / [`signing::SignedEnvelope`]
//! / [`signing::EnvelopeHeader`] / [`signing::MessageSigner`] / [`signing::MockSigner`]
//! / [`signing::pack_and_sign`] / [`signing::unpack_and_verify`]，支持 SM2 签名（feature = "sm2"）
//! 与 5 秒防重放窗口。为后续 v0.98.0 mTLS、v0.117.0 审计哈希链提供签名基础。
//!
//! # 核心类型
//!
//! - [`node::DdsNode`] — DDS 节点统一 trait（D2 无 Send + Sync bound，D7 合并 reader/writer）
//! - [`mock::MockDdsNode`] — 默认可用的 Mock 实现（D3，纯 Rust）
//! - [`cyclone_dds::CycloneDdsNode`] — Cyclone DDS C 库实现（feature = "cyclone-dds"，D3）
//! - [`config::DdsConfig`] / [`config::DiscoveryPolicy`] — 节点配置与发现策略（D5 / D6）
//! - [`qos::QosPolicy`] / [`qos::Reliability`] / [`qos::Durability`] / [`qos::History`] — QoS 策略（D2/D3：History::KeepLast(u32) + deadline/lifespan/priority）
//! - [`topic::TopicSpec`] / [`topic::TopicCategory`] / [`topic::PayloadType`] / [`topic::TopicError`] / [`topic::standard_topics`] / [`topic::validate_topic_name`] — Topic 语义层（v0.76.0 新增）
//! - [`registry::TopicRegistry`] — Topic 注册表（D1：BTreeMap，D4：简化通配符匹配）（v0.76.0 新增）
//! - [`types::DdsSample`] / [`types::InstanceHandle`] / [`types::ParticipantId`] / [`types::ReaderId`] / [`types::WriterId`] — 数据样本与句柄（D4）
//! - [`error::DdsError`] — 错误类型（8 变体）
//! - [`policy::AgentId`] / [`policy::Permission`] / [`policy::DropReason`] / [`policy::RoutingPolicy`] / [`policy::RouteError`] / [`policy::RouteDecision`] / [`policy::CapabilityVerifier`] / [`policy::MockCapabilityVerifier`] — 路由策略层（v0.77.0 新增，D7/D10/D12）
//! - [`router::SubId`] / [`router::Subscription`] / [`router::RouterStats`] / [`router::pattern_matches`] / [`router::MessageRouter`] — 消息路由器（v0.77.0 新增，D5/D7/D8/D9/D11）
//! - [`codec::CodecKind`] / [`codec::CodecError`] — 编解码元数据 tag（v0.78.0 新增，D6 不实现实际编解码）
//! - [`signing::KeyId`] / [`signing::MsgId`] / [`signing::EnvelopeHeader`] / [`signing::SignedEnvelope`] / [`signing::SignError`] / [`signing::MessageSigner`] / [`signing::MockSigner`] / [`signing::pack_and_sign`] / [`signing::unpack_and_verify`] — 消息签名层（v0.78.0 新增，D7/D8/D9/D10/D11/D13/D14）
//!
//! # 偏差声明（D1~D14）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 扩展 v0.75.0 `eneros-agent-bus-dds` crate（不新建 crate）；签名层与 DDS 同属协议层（项目规则 §2.3.1，沿用 v0.77.0 D1） |
//! | **D2** | 文档位于 `docs/protocols/message-signing-design.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/message_signing.md`） |
//! | **D3** | 配置位于 `configs/signing_keys.toml`（项目规则 §2.3，非蓝图 `config/`） |
//! | **D4** | 测试内嵌 `src/lib.rs` T49~T63（沿用 v0.75.0~v0.77.0 模式，非蓝图 `tests/signing_verify.rs` / `tests/codec_bench.rs`） |
//! | **D5** | `KeyStore.keys: BTreeMap<KeyId, Sm2PublicKey>` 替代 `HashMap`（no_std 合规，v0.76.0 D1 先例） |
//! | **D6** | `CodecKind` 仅作为 header 元数据 tag，**不实现**实际 CDR/Bincode/JSON 编解码（避免引入 `cdr` / `bincode` / `serde_json` 三个 crate；扩展 v0.76.0 D6） |
//! | **D7** | `MessageCodec` / `MessageSigner` trait 无 `Send + Sync` bound（no_std 单线程，v0.59.0/v0.64.0/v0.72.0/v0.77.0 先例） |
//! | **D8** | `MsgId(pub u64)` newtype 替代 `Uuid::new_v4()`（无 `uuid` crate 依赖；Karpathy 简化） |
//! | **D9** | `verify()` / `pack_and_sign()` 显式接受 `now: u64` 参数，无 `current_timestamp()` 全局函数（no_std 无系统时钟） |
//! | **D10** | `pack_and_sign(payload: &[u8])` 接受已序列化字节，无 `impl Serialize` 泛型约束（避免 `serde` 依赖） |
//! | **D11** | 复用 `policy::AgentId(pub u64)`（v0.77.0 已定义），不重复定义 |
//! | **D12** | 不实现性能基准测试（CI 无法稳定验证 ≥1000 sig/s），仅保留正确性测试；性能延后到 v0.158.0 硬件加速 |
//! | **D13** | `KeyId(pub u64)` newtype（与 `AgentId` 解耦：密钥轮换允许 key_id ≠ agent_id） |
//! | **D14** | `MessageSigner` trait + `MockSigner` 默认实现；`Sm2Signer` + `KeyStore` 在 `sm2` feature 后（默认 build 不引入 `eneros-crypto`，保持 no_std 最小依赖） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，默认外部依赖仅 `slotmap`（`default-features = false`）。
//! 默认 feature 下不引入任何 `std::*`，不调用 `panic!` / `todo!` / `unimplemented!`，
//! 不含 `unsafe` 块（仅 `cyclone-dds` feature 启用时 `ffi` / `cyclone_dds` 模块含 `unsafe`）。
//! 启用 `sm2` feature 时引入 `eneros-crypto`（同样 no_std，纯 Rust 无 C FFI）。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod codec;
pub mod config;
pub mod error;
pub mod mock;
pub mod node;
pub mod policy;
pub mod qos;
pub mod registry;
pub mod router;
pub mod signing;
pub mod topic;
pub mod types;

#[cfg(feature = "cyclone-dds")]
pub mod cyclone_dds;

#[cfg(feature = "cyclone-dds")]
pub mod ffi;

pub use codec::{CodecError, CodecKind};
pub use config::{DdsConfig, DiscoveryPolicy};
pub use error::DdsError;
pub use mock::MockDdsNode;
pub use node::{DdsNode, DdsNodeConfig};
pub use policy::{
    AgentId, CapabilityVerifier, DropReason, MockCapabilityVerifier, Permission, RouteDecision,
    RouteError, RoutingPolicy,
};
pub use qos::{Durability, History, QosPolicy, Reliability};
pub use registry::TopicRegistry;
pub use router::{pattern_matches, MessageRouter, RouterStats, SubId, Subscription};
pub use signing::{
    pack_and_sign, unpack_and_verify, EnvelopeHeader, KeyId, MessageSigner, MockSigner, MsgId,
    SignError, SignedEnvelope,
};
#[cfg(feature = "sm2")]
pub use signing::{KeyStore, Sm2Signer};
pub use topic::{
    standard_topics, validate_topic_name, PayloadType, TopicCategory, TopicError, TopicSpec,
};
pub use types::{DdsSample, InstanceHandle, ParticipantId, ReaderId, WriterId};

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T48（覆盖 D1~D13 偏差声明与 spec 验收场景）.
    //!
    //! - T1~T17：v0.75.0 通信底座（DdsNode/MockDdsNode/QosPolicy 基础）
    //! - T18~T31：v0.76.0 语义层（TopicSpec/TopicRegistry/QoS 扩展）
    //! - T32~T48：v0.77.0 路由层（RoutingPolicy/CapabilityVerifier/MessageRouter）
    //!
    //! 全部使用 `MockDdsNode`（`CycloneDdsNode` 受 feature-gated，需 C 库链接）。

    use super::*;
    use crate::config::{DdsConfig, DiscoveryPolicy};
    use crate::error::DdsError;
    use crate::mock::MockDdsNode;
    use crate::node::{DdsNode, DdsNodeConfig};
    use crate::qos::{Durability, History, QosPolicy, Reliability};
    use crate::types::ParticipantId;

    // ===== T1：DdsError 各变体构造 + Display 输出非空 =====
    #[test]
    fn test_t1_dds_error_variants_display() {
        let errors = [
            DdsError::Ffi(-1),
            DdsError::InvalidHandle,
            DdsError::Closed,
            DdsError::InconsistentQos(alloc::string::String::from("reliability mismatch")),
            DdsError::Serialization(alloc::string::String::from("cdr decode failed")),
            DdsError::TopicNotFound(alloc::string::String::from("foo")),
            DdsError::ParticipantNotFound,
            DdsError::Timeout,
        ];
        for err in &errors {
            let s = alloc::format!("{}", err);
            assert!(!s.is_empty(), "Display 输出不应为空: {:?}", err);
        }
    }

    // ===== T2：DdsConfig::default() 字段验证（domain_id=0, Multicast, None）=====
    #[test]
    fn test_t2_dds_config_default() {
        let cfg = DdsConfig::default();
        assert_eq!(cfg.domain_id, 0);
        assert_eq!(cfg.discovery, DiscoveryPolicy::Multicast);
        assert!(cfg.interface.is_none());
    }

    // ===== T3：DdsConfig::new(42, Unicast) + interface 设置 =====
    #[test]
    fn test_t3_dds_config_new_unicast() {
        let mut cfg = DdsConfig::new(42, DiscoveryPolicy::Unicast);
        assert_eq!(cfg.domain_id, 42);
        assert_eq!(cfg.discovery, DiscoveryPolicy::Unicast);
        assert!(cfg.interface.is_none());

        cfg.interface = Some(alloc::string::String::from("eth0"));
        assert_eq!(cfg.interface.as_deref(), Some("eth0"));
    }

    // ===== T4：QosPolicy::default() 字段验证（Reliable/Volatile/KeepLast(10) + deadline/lifespan/priority）=====
    #[test]
    fn test_t4_qos_policy_default() {
        let qos = QosPolicy::default();
        assert_eq!(qos.reliability, Reliability::Reliable);
        assert_eq!(qos.durability, Durability::Volatile);
        assert_eq!(qos.history, History::KeepLast(10));
        assert_eq!(qos.deadline, None);
        assert_eq!(qos.lifespan, None);
        assert_eq!(qos.priority, 0);
    }

    // ===== T5：QosPolicy::state_default() 字段验证（BestEffort/Volatile/KeepLast(1) + lifespan=5s）=====
    #[test]
    fn test_t5_qos_policy_state_default() {
        let qos = QosPolicy::state_default();
        assert_eq!(qos.reliability, Reliability::BestEffort);
        assert_eq!(qos.durability, Durability::Volatile);
        assert_eq!(qos.history, History::KeepLast(1));
        assert_eq!(qos.deadline, None);
        assert_eq!(qos.lifespan, Some(core::time::Duration::from_secs(5)));
        assert_eq!(qos.priority, 0);
    }

    // ===== T6：MockDdsNode::new_default() + is_shutdown == false =====
    #[test]
    fn test_t6_mock_node_new_default_not_shutdown() {
        let node = MockDdsNode::new_default();
        assert!(!node.is_shutdown());
        assert_eq!(node.config().domain_id, 0);
    }

    // ===== T7：MockDdsNode create_participant 返回 Ok =====
    #[test]
    fn test_t7_create_participant_ok() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant();
        assert!(p.is_ok(), "create_participant 应返回 Ok");
    }

    // ===== T8：MockDdsNode create_writer + create_reader 句柄分配 =====
    #[test]
    fn test_t8_create_writer_and_reader() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node.create_writer(p, "topic1", QosPolicy::default());
        let r = node.create_reader(p, "topic1", QosPolicy::default());
        assert!(w.is_ok(), "create_writer 应返回 Ok");
        assert!(r.is_ok(), "create_reader 应返回 Ok");
    }

    // ===== T9：单节点往返：write [1,2,3] → take(10) 返回 1 条样本 =====
    #[test]
    fn test_t9_single_node_roundtrip() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let _w = node
            .create_writer(p, "topic1", QosPolicy::default())
            .expect("writer");
        let r = node
            .create_reader(p, "topic1", QosPolicy::default())
            .expect("reader");

        node.write(_w, &[0x01, 0x02, 0x03]).expect("write");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].payload, alloc::vec![0x01, 0x02, 0x03]);
    }

    // ===== T10：read 不清空：write → read(10) 返回 1 条 → read(10) 再次返回 1 条 =====
    #[test]
    fn test_t10_read_does_not_consume() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "t", QosPolicy::default())
            .expect("writer");
        let r = node
            .create_reader(p, "t", QosPolicy::default())
            .expect("reader");

        node.write(w, &[0xAA]).expect("write");
        let first = node.read(r, 10).expect("read 1");
        assert_eq!(first.len(), 1);
        let second = node.read(r, 10).expect("read 2");
        assert_eq!(second.len(), 1, "read 不应清空 buffer");
    }

    // ===== T11：跨 topic 隔离：write topic1 → reader(topic2).take(10) 返回空 =====
    #[test]
    fn test_t11_cross_topic_isolation() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "topic1", QosPolicy::default())
            .expect("writer");
        let r = node
            .create_reader(p, "topic2", QosPolicy::default())
            .expect("reader");

        node.write(w, &[0x01]).expect("write");
        let samples = node.take(r, 10).expect("take");
        assert!(samples.is_empty(), "跨 topic 不应收到消息");
    }

    // ===== T12：多 reader 广播：2 个 reader 监听同 topic → write 1 条 → 每个 take(1) 各返回 1 条 =====
    #[test]
    fn test_t12_multi_reader_broadcast() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "t", QosPolicy::default())
            .expect("writer");
        let r1 = node
            .create_reader(p, "t", QosPolicy::default())
            .expect("reader1");
        let r2 = node
            .create_reader(p, "t", QosPolicy::default())
            .expect("reader2");

        node.write(w, &[0x42]).expect("write");
        let s1 = node.take(r1, 1).expect("take r1");
        let s2 = node.take(r2, 1).expect("take r2");
        assert_eq!(s1.len(), 1, "reader1 应收到 1 条");
        assert_eq!(s2.len(), 1, "reader2 应收到 1 条（广播）");
        assert_eq!(s1[0].payload, alloc::vec![0x42]);
        assert_eq!(s2[0].payload, alloc::vec![0x42]);
    }

    // ===== T13：KeepLast 截断：reader qos KeepLast(2) → write 3 条 → take(10) 最多返回 2 条 =====
    #[test]
    fn test_t13_keep_last_truncation() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "t", QosPolicy::default())
            .expect("writer");
        let qos = QosPolicy {
            reliability: Reliability::Reliable,
            durability: Durability::Volatile,
            history: History::KeepLast(2),
            deadline: None,
            lifespan: None,
            priority: 0,
        };
        let r = node.create_reader(p, "t", qos).expect("reader");

        node.write(w, &[1]).expect("write 1");
        node.write(w, &[2]).expect("write 2");
        node.write(w, &[3]).expect("write 3");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 2, "KeepLast(2) 应截断为 2 条");
        // 截断最旧的，保留 [2, 3]
        assert_eq!(samples[0].payload, alloc::vec![2]);
        assert_eq!(samples[1].payload, alloc::vec![3]);
    }

    // ===== T14：shutdown 后 create_participant 返回 Err(Closed) =====
    #[test]
    fn test_t14_shutdown_blocks_create() {
        let mut node = MockDdsNode::new_default();
        assert!(!node.is_shutdown());
        node.shutdown().expect("shutdown");
        assert!(node.is_shutdown());
        let r = node.create_participant();
        assert!(matches!(r, Err(DdsError::Closed)));
    }

    // ===== T15：InvalidHandle：用无效 ParticipantId 创建 reader 返回 Err(InvalidHandle) =====
    #[test]
    fn test_t15_invalid_participant_handle() {
        let mut node = MockDdsNode::new_default();
        // ParticipantId::default() 返回无效 key（version=0，永不匹配）
        let invalid_pid = ParticipantId::default();
        let r = node.create_reader(invalid_pid, "t", QosPolicy::default());
        assert!(matches!(r, Err(DdsError::InvalidHandle)));
    }

    // ===== T16：set_now_ns：设置 now_ns=12345 → write → take → source_timestamp == 12345 =====
    #[test]
    fn test_t16_set_now_ns_timestamp() {
        let mut node = MockDdsNode::new_default();
        node.set_now_ns(12345);
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "t", QosPolicy::default())
            .expect("writer");
        let r = node
            .create_reader(p, "t", QosPolicy::default())
            .expect("reader");

        node.write(w, &[0x01]).expect("write");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].source_timestamp, 12345);
    }

    // ===== T17：feature 验证：默认 feature 下 CycloneDdsNode 不存在 =====
    #[cfg(not(feature = "cyclone-dds"))]
    #[test]
    fn test_t17_cyclone_dds_not_available_by_default() {
        // 默认 feature 下，ffi 模块和 cyclone_dds 模块不参与编译
        // （#[cfg(feature = "cyclone-dds")] 门控）。
        // 此测试通过编译即证明 feature 门控生效：
        // 若门控失效，将因引用不存在的 `crate::cyclone_dds::CycloneDdsNode`
        // 而编译失败。
        // 此处不引用任何 cyclone_dds 类型，仅验证 feature 默认关闭。
    }

    // ===== T18：validate_topic_name 合法 topic 名 =====
    #[test]
    fn test_t18_validate_topic_name_valid() {
        assert!(validate_topic_name("/power/state/battery/1").is_ok());
        assert!(validate_topic_name("/power/state/battery/{id}").is_ok());
        assert!(validate_topic_name("/power/command/internal").is_ok());
    }

    // ===== T19：validate_topic_name 非法 topic 名 =====
    #[test]
    fn test_t19_validate_topic_name_invalid() {
        assert!(matches!(
            validate_topic_name("power/state"),
            Err(TopicError::InvalidName(_))
        ));
        assert!(matches!(
            validate_topic_name("/power/state battery"),
            Err(TopicError::InvalidName(_))
        ));
        assert!(matches!(
            validate_topic_name("/power/state;drop"),
            Err(TopicError::InvalidName(_))
        ));
    }

    // ===== T20：QosPolicy::command_default() =====
    #[test]
    fn test_t20_qos_policy_command_default() {
        let qos = QosPolicy::command_default();
        assert_eq!(qos.reliability, Reliability::Reliable);
        assert_eq!(qos.durability, Durability::TransientLocal);
        assert_eq!(qos.history, History::KeepAll);
        assert_eq!(qos.deadline, Some(core::time::Duration::from_secs(2)));
        assert_eq!(qos.lifespan, Some(core::time::Duration::from_secs(10)));
        assert_eq!(qos.priority, 6);
    }

    // ===== T21：QosPolicy::alert_default() =====
    #[test]
    fn test_t21_qos_policy_alert_default() {
        let qos = QosPolicy::alert_default();
        assert_eq!(qos.reliability, Reliability::Reliable);
        assert_eq!(qos.durability, Durability::TransientLocal);
        assert_eq!(qos.history, History::KeepLast(10));
        assert_eq!(qos.deadline, None);
        assert_eq!(qos.lifespan, None);
        assert_eq!(qos.priority, 7);
    }

    // ===== T22：standard_topics() 返回 8 个标准 Topic =====
    #[test]
    fn test_t22_standard_topics_count() {
        let topics = standard_topics();
        assert_eq!(topics.len(), 8);
        let names: Vec<&str> = topics.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"/power/state/battery/{id}"));
        assert!(names.contains(&"/power/state/pv/{id}"));
        assert!(names.contains(&"/power/state/grid"));
        assert!(names.contains(&"/power/market/price"));
        assert!(names.contains(&"/power/market/signal"));
        assert!(names.contains(&"/power/command/internal"));
        assert!(names.contains(&"/power/alert/fault"));
        assert!(names.contains(&"/power/twin/update"));
    }

    // ===== T23：TopicRegistry::with_standards() =====
    #[test]
    fn test_t23_registry_with_standards() {
        let registry = TopicRegistry::with_standards();
        assert!(registry.lookup("/power/state/battery/{id}").is_some());
        assert!(registry.lookup("/power/alert/fault").is_some());
    }

    // ===== T24：register() 注册新 Topic 成功 =====
    #[test]
    fn test_t24_register_new_topic() {
        let mut registry = TopicRegistry::new();
        let spec = TopicSpec {
            name: String::from("/power/custom/topic"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(core::time::Duration::from_secs(5)),
        };
        assert!(registry.register(spec).is_ok());
        assert!(registry.lookup("/power/custom/topic").is_some());
    }

    // ===== T25：register() 重复注册同名且 QoS 一致 → Ok（幂等）=====
    #[test]
    fn test_t25_register_duplicate_same_qos() {
        use alloc::string::ToString;
        let mut registry = TopicRegistry::new();
        let spec1 = TopicSpec {
            name: "/power/test".to_string(),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: None,
        };
        let spec2 = spec1.clone();
        assert!(registry.register(spec1).is_ok());
        assert!(registry.register(spec2).is_ok()); // 幂等
    }

    // ===== T26：register() 重复注册同名且 QoS 不一致 → Err(Conflict) =====
    #[test]
    fn test_t26_register_duplicate_different_qos() {
        use alloc::string::ToString;
        let mut registry = TopicRegistry::new();
        let spec1 = TopicSpec {
            name: "/power/test".to_string(),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: None,
        };
        let spec2 = TopicSpec {
            name: "/power/test".to_string(),
            category: TopicCategory::Command,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::command_default(),
            ttl: None,
        };
        assert!(registry.register(spec1).is_ok());
        assert!(matches!(
            registry.register(spec2),
            Err(TopicError::Conflict { .. })
        ));
    }

    // ===== T27：register() 非法 topic 名 → Err(InvalidName) =====
    #[test]
    fn test_t27_register_invalid_name() {
        use alloc::string::ToString;
        let mut registry = TopicRegistry::new();
        let spec = TopicSpec {
            name: "invalid_no_slash".to_string(),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: None,
        };
        assert!(matches!(
            registry.register(spec),
            Err(TopicError::InvalidName(_))
        ));
    }

    // ===== T28：lookup() 查询已注册 Topic =====
    #[test]
    fn test_t28_lookup_registered() {
        let registry = TopicRegistry::with_standards();
        let spec = registry.lookup("/power/state/grid");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().category, TopicCategory::State);
    }

    // ===== T29：lookup() 查询未注册 Topic → None =====
    #[test]
    fn test_t29_lookup_unregistered() {
        let registry = TopicRegistry::with_standards();
        assert!(registry.lookup("/unknown/topic").is_none());
    }

    // ===== T30：match_pattern() 通配符匹配 =====
    #[test]
    fn test_t30_match_pattern() {
        let mut registry = TopicRegistry::with_standards();
        // 注册具体 topic 用于通配匹配
        let battery = TopicSpec {
            name: alloc::string::String::from("/power/state/battery/1"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(core::time::Duration::from_secs(5)),
        };
        let pv = TopicSpec {
            name: alloc::string::String::from("/power/state/pv/1"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(core::time::Duration::from_secs(5)),
        };
        registry.register(battery).unwrap();
        registry.register(pv).unwrap();

        let matches = registry.match_pattern("/power/state/*");
        assert!(
            matches.len() >= 2,
            "应匹配至少 2 个 /power/state/ 开头的 topic"
        );
    }

    // ===== T31：MockDdsNode with KeepAll — write 3 条不截断 =====
    #[test]
    fn test_t31_keep_all_no_truncation() {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "t", QosPolicy::default())
            .expect("writer");
        let qos = QosPolicy {
            reliability: Reliability::Reliable,
            durability: Durability::Volatile,
            history: History::KeepAll,
            deadline: None,
            lifespan: None,
            priority: 0,
        };
        let r = node.create_reader(p, "t", qos).expect("reader");

        node.write(w, &[1]).expect("write 1");
        node.write(w, &[2]).expect("write 2");
        node.write(w, &[3]).expect("write 3");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 3, "KeepAll 不应截断");
    }

    // ===== T32：Permission 枚举变体 =====
    #[test]
    fn test_t32_permission_variants() {
        assert_eq!(Permission::Publish, Permission::Publish);
        assert_eq!(Permission::Subscribe, Permission::Subscribe);
        assert_ne!(Permission::Publish, Permission::Subscribe);
    }

    // ===== T33：DropReason::reason_name() 返回正确字符串 =====
    #[test]
    fn test_t33_drop_reason_name() {
        assert_eq!(DropReason::Unauthorized.reason_name(), "Unauthorized");
        assert_eq!(DropReason::RateLimited.reason_name(), "RateLimited");
        assert_eq!(DropReason::InvalidTopic.reason_name(), "InvalidTopic");
        assert_eq!(DropReason::TokenExpired.reason_name(), "TokenExpired");
    }

    // ===== T34：RoutingPolicy::default() 全 false / None =====
    #[test]
    fn test_t34_routing_policy_default() {
        let p = RoutingPolicy::default();
        assert!(!p.require_publish_token);
        assert!(!p.require_subscribe_token);
        assert!(!p.priority_preempt);
        assert_eq!(p.rate_limit_per_agent, None);
    }

    // ===== T35：RoutingPolicy::strict() 全 true / Some(100) =====
    #[test]
    fn test_t35_routing_policy_strict() {
        let p = RoutingPolicy::strict();
        assert!(p.require_publish_token);
        assert!(p.require_subscribe_token);
        assert!(p.priority_preempt);
        assert_eq!(p.rate_limit_per_agent, Some(100));
    }

    // ===== T36：RouteError::Display 输出非空 =====
    #[test]
    fn test_t36_route_error_display() {
        let errors = [
            RouteError::InvalidPattern(alloc::string::String::from("/bad pattern")),
            RouteError::Dropped(DropReason::Unauthorized),
            RouteError::InvalidTopic(alloc::string::String::from("/unknown")),
        ];
        for err in &errors {
            let s = alloc::format!("{}", err);
            assert!(!s.is_empty(), "Display 输出不应为空: {:?}", err);
        }
    }

    // ===== T37：MockCapabilityVerifier::verify() 返回 Ok =====
    #[test]
    fn test_t37_mock_verifier_always_ok() {
        let v = MockCapabilityVerifier;
        let agent = AgentId(1);
        assert!(v.verify(Permission::Publish, agent, "/test").is_ok());
        assert!(v.verify(Permission::Subscribe, agent, "/test/*").is_ok());
    }

    // ===== T38：pattern_matches 精确匹配 =====
    #[test]
    fn test_t38_pattern_matches_exact() {
        assert!(pattern_matches(
            "/power/state/battery",
            "/power/state/battery"
        ));
        assert!(!pattern_matches("/power/state/battery", "/power/state/pv"));
    }

    // ===== T39：pattern_matches * 后缀通配 =====
    #[test]
    fn test_t39_pattern_matches_wildcard() {
        assert!(pattern_matches("/power/state/*", "/power/state/battery"));
        assert!(pattern_matches("/power/state/*", "/power/state/pv"));
        assert!(pattern_matches("/power/state/*", "/power/state/"));
        assert!(!pattern_matches("/power/state/*", "/market/price"));
    }

    // ===== T40：pattern_matches 不匹配 =====
    #[test]
    fn test_t40_pattern_matches_no_match() {
        assert!(!pattern_matches("/market/*", "/power/state/battery"));
        assert!(!pattern_matches(
            "/power/state/battery",
            "/power/state/battery/extra"
        ));
    }

    // ===== T41：MessageRouter::new() 默认状态（stats 全 0）=====
    #[test]
    fn test_t41_router_new_default_stats() {
        let router = MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        assert_eq!(router.stats().total_routed, 0);
        assert_eq!(router.stats().total_dropped, 0);
        assert!(router.stats().dropped_by_reason.is_empty());
    }

    // ===== T42：subscribe() 成功返回 SubId 递增 =====
    #[test]
    fn test_t42_subscribe_returns_incrementing_subid() {
        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        let cb: Box<dyn Fn(&DdsSample)> = Box::new(|_s| {});
        let id1 = router
            .subscribe("/power/state/*", AgentId(1), cb)
            .expect("sub1");
        let id2 = router
            .subscribe("/market/*", AgentId(2), Box::new(|_s| {}))
            .expect("sub2");
        let id3 = router
            .subscribe("/command/internal", AgentId(3), Box::new(|_s| {}))
            .expect("sub3");
        assert_eq!(id1, SubId(1));
        assert_eq!(id2, SubId(2));
        assert_eq!(id3, SubId(3));
    }

    // ===== T43：subscribe() 非法 pattern 返回 Err(InvalidPattern) =====
    #[test]
    fn test_t43_subscribe_invalid_pattern() {
        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        // Pattern must start with '/' and contain only [a-zA-Z0-9_/{]
        let result = router.subscribe("invalid-no-slash", AgentId(1), Box::new(|_s| {}));
        assert!(matches!(result, Err(RouteError::InvalidPattern(_))));

        let result2 = router.subscribe("/bad pattern", AgentId(1), Box::new(|_s| {}));
        assert!(matches!(result2, Err(RouteError::InvalidPattern(_))));
    }

    // ===== T44：subscribe() require_subscribe_token=true + Mock 放行 =====
    #[test]
    fn test_t44_subscribe_with_token_required_mock_passes() {
        let policy = RoutingPolicy {
            require_subscribe_token: true,
            ..RoutingPolicy::default()
        };
        let mut router = MessageRouter::new(TopicRegistry::with_standards(), policy);
        // MockCapabilityVerifier always returns Ok, so subscribe should succeed
        let result = router.subscribe("/power/state/*", AgentId(1), Box::new(|_s| {}));
        assert!(result.is_ok());
    }

    // ===== T45：dispatch() 精确匹配 topic 派发到 1 个订阅 =====
    #[test]
    fn test_t45_dispatch_exact_match_one_subscriber() {
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let cb: Box<dyn Fn(&DdsSample)> = Box::new(move |_s| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        router
            .subscribe("/power/state/battery", AgentId(1), cb)
            .expect("sub");

        let sample = DdsSample {
            payload: alloc::vec![0x01],
            instance_handle: 0,
            source_timestamp: 100,
        };
        let count = router
            .dispatch("/power/state/battery", &sample)
            .expect("dispatch");
        assert_eq!(count, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ===== T46：dispatch() 通配匹配派发到多个订阅 =====
    #[test]
    fn test_t46_dispatch_wildcard_match_multiple_subscribers() {
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));

        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        // Two subscriptions matching the same topic
        let c1 = counter.clone();
        router
            .subscribe(
                "/power/state/*",
                AgentId(1),
                Box::new(move |_s| {
                    c1.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .expect("sub1");
        let c2 = counter.clone();
        router
            .subscribe(
                "/power/state/battery",
                AgentId(2),
                Box::new(move |_s| {
                    c2.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .expect("sub2");
        // Non-matching subscription
        router
            .subscribe("/market/*", AgentId(3), Box::new(|_s| {}))
            .expect("sub3");

        let sample = DdsSample {
            payload: alloc::vec![0x42],
            instance_handle: 0,
            source_timestamp: 200,
        };
        let count = router
            .dispatch("/power/state/battery", &sample)
            .expect("dispatch");
        assert_eq!(count, 2, "两个订阅应收到消息");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    // ===== T47：dispatch() 不匹配 topic 返回 Ok(0) =====
    #[test]
    fn test_t47_dispatch_no_match_returns_zero() {
        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        router
            .subscribe("/market/*", AgentId(1), Box::new(|_s| {}))
            .expect("sub");

        let sample = DdsSample {
            payload: alloc::vec![0x01],
            instance_handle: 0,
            source_timestamp: 300,
        };
        let count = router
            .dispatch("/power/state/battery", &sample)
            .expect("dispatch");
        assert_eq!(count, 0, "无匹配订阅应返回 0");
    }

    // ===== T48：dispatch() 未注册 topic 仍 Deliver priority=0 =====
    #[test]
    fn test_t48_dispatch_unregistered_topic_delivers_priority_zero() {
        let mut router =
            MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default());
        router
            .subscribe("/unknown/*", AgentId(1), Box::new(|_s| {}))
            .expect("sub");

        let sample = DdsSample {
            payload: alloc::vec![0x01],
            instance_handle: 0,
            source_timestamp: 400,
        };
        // Unregistered topic should still deliver (priority=0)
        let decision = router.route("/unknown/topic", &sample);
        assert!(matches!(decision, RouteDecision::Deliver { priority: 0 }));

        let count = router
            .dispatch("/unknown/topic", &sample)
            .expect("dispatch");
        assert_eq!(count, 1, "通配匹配应派发");
    }
}

#[cfg(test)]
mod tests_v0780 {
    //! v0.78.0 签名层测试 T49~T63（覆盖 D6~D14 偏差声明与 spec 验收场景）.
    //!
    //! - T49~T50：编解码元数据 tag（CodecKind / CodecError）
    //! - T51~T55：签名层基础类型（KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError）
    //! - T56~T58：MockSigner 签名/验签/防重放
    //! - T59~T63：pack_and_sign / unpack_and_verify 端到端流程

    use super::*;
    use crate::policy::AgentId;

    // T49: CodecKind 变体与 as_u8/from_u8 往返
    #[test]
    fn test_t49_codec_kind_round_trip() {
        assert_eq!(CodecKind::Cdr.as_u8(), 0);
        assert_eq!(CodecKind::Bincode.as_u8(), 1);
        assert_eq!(CodecKind::Json.as_u8(), 2);
        assert_eq!(CodecKind::from_u8(0), Some(CodecKind::Cdr));
        assert_eq!(CodecKind::from_u8(1), Some(CodecKind::Bincode));
        assert_eq!(CodecKind::from_u8(2), Some(CodecKind::Json));
        assert_eq!(CodecKind::from_u8(3), None);
    }

    // T50: CodecError::Display 输出非空（3 变体）
    #[test]
    fn test_t50_codec_error_display() {
        let s1 = format!("{}", CodecError::Unsupported(CodecKind::Cdr));
        let s2 = format!("{}", CodecError::InvalidData);
        let s3 = format!("{}", CodecError::BufferTooShort);
        assert!(!s1.is_empty());
        assert!(!s2.is_empty());
        assert!(!s3.is_empty());
    }

    // T51: KeyId newtype 基本访问
    #[test]
    fn test_t51_key_id_access() {
        let id = KeyId(42);
        assert_eq!(id.0, 42);
        let id2 = KeyId(42);
        assert_eq!(id, id2);
    }

    // T52: MsgId newtype 基本访问
    #[test]
    fn test_t52_msg_id_access() {
        let id = MsgId(99);
        assert_eq!(id.0, 99);
    }

    // T53: EnvelopeHeader 构造与字段访问
    #[test]
    fn test_t53_envelope_header_construction() {
        let h = EnvelopeHeader {
            msg_id: MsgId(1),
            timestamp: 1000,
            source: AgentId(7),
            topic: String::from("/power/state"),
            qos: 5,
            codec: CodecKind::Bincode,
            key_id: KeyId(10),
        };
        assert_eq!(h.msg_id, MsgId(1));
        assert_eq!(h.timestamp, 1000);
        assert_eq!(h.source, AgentId(7));
        assert_eq!(h.topic, "/power/state");
        assert_eq!(h.qos, 5);
        assert_eq!(h.codec, CodecKind::Bincode);
        assert_eq!(h.key_id, KeyId(10));
    }

    // T54: SignedEnvelope 字段访问
    #[test]
    fn test_t54_signed_envelope_access() {
        let h = EnvelopeHeader {
            msg_id: MsgId(1),
            timestamp: 1000,
            source: AgentId(7),
            topic: String::from("/t"),
            qos: 0,
            codec: CodecKind::Cdr,
            key_id: KeyId(1),
        };
        let env = SignedEnvelope {
            header: h.clone(),
            payload: vec![0xDE, 0xAD],
            signature: [0u8; 64],
        };
        assert_eq!(env.header, h);
        assert_eq!(env.payload, vec![0xDE, 0xAD]);
        assert_eq!(env.signature.len(), 64);
    }

    // T55: SignError::Display 输出非空（6 变体）
    #[test]
    fn test_t55_sign_error_display() {
        let cases = [
            format!("{}", SignError::EncodeFailed),
            format!("{}", SignError::UnknownKey(KeyId(1))),
            format!("{}", SignError::StaleTimestamp),
            format!("{}", SignError::SigningFailed),
            format!("{}", SignError::VerifyFailed),
            format!("{}", SignError::MockError),
        ];
        for s in cases.iter() {
            assert!(!s.is_empty());
        }
    }

    // T56: MockSigner::sign() 返回 Ok(64 字节)
    #[test]
    fn test_t56_mock_signer_sign() {
        let signer = MockSigner;
        let h = EnvelopeHeader {
            msg_id: MsgId(1),
            timestamp: 5000,
            source: AgentId(1),
            topic: String::from("/t"),
            qos: 0,
            codec: CodecKind::Bincode,
            key_id: KeyId(1),
        };
        let sig = signer.sign(&h, b"payload").unwrap();
        assert_eq!(sig.len(), 64);
        // 前 8 字节 = timestamp.to_be_bytes()
        assert_eq!(&sig[..8], &5000u64.to_be_bytes());
        // 第 9 字节 = payload.len() & 0xFF = 7
        assert_eq!(sig[8], 7);
    }

    // T57: MockSigner::verify() 匹配签名返回 Ok(true)
    #[test]
    fn test_t57_mock_signer_verify_match() {
        let signer = MockSigner;
        let h = EnvelopeHeader {
            msg_id: MsgId(1),
            timestamp: 5000,
            source: AgentId(1),
            topic: String::from("/t"),
            qos: 0,
            codec: CodecKind::Bincode,
            key_id: KeyId(1),
        };
        let sig = signer.sign(&h, b"payload").unwrap();
        let ok = signer.verify(&h, b"payload", &sig, 5000).unwrap();
        assert!(ok);
    }

    // T58: MockSigner::verify() 时间戳过期返回 Err(StaleTimestamp)
    #[test]
    fn test_t58_mock_signer_stale_timestamp() {
        let signer = MockSigner;
        let h = EnvelopeHeader {
            msg_id: MsgId(1),
            timestamp: 1000,
            source: AgentId(1),
            topic: String::from("/t"),
            qos: 0,
            codec: CodecKind::Bincode,
            key_id: KeyId(1),
        };
        let sig = signer.sign(&h, b"payload").unwrap();
        // now = 1000 + 5001 = 6001，差 5001 > 5000
        let res = signer.verify(&h, b"payload", &sig, 6001);
        assert_eq!(res, Err(SignError::StaleTimestamp));
    }

    // T59: pack_and_sign() + MockSigner 成功构造 SignedEnvelope
    #[test]
    fn test_t59_pack_and_sign() {
        let signer = MockSigner;
        let env = pack_and_sign(
            &signer,
            b"hello",
            AgentId(1),
            "/topic",
            3,
            CodecKind::Bincode,
            KeyId(1),
            MsgId(42),
            1000,
        )
        .unwrap();
        assert_eq!(env.header.msg_id, MsgId(42));
        assert_eq!(env.header.timestamp, 1000);
        assert_eq!(env.header.source, AgentId(1));
        assert_eq!(env.header.topic, "/topic");
        assert_eq!(env.header.qos, 3);
        assert_eq!(env.header.codec, CodecKind::Bincode);
        assert_eq!(env.header.key_id, KeyId(1));
        assert_eq!(env.payload, b"hello");
        assert_eq!(env.signature.len(), 64);
    }

    // T60: unpack_and_verify() + MockSigner 匹配返回 Ok(true)
    #[test]
    fn test_t60_unpack_and_verify_match() {
        let signer = MockSigner;
        let env = pack_and_sign(
            &signer,
            b"payload",
            AgentId(1),
            "/t",
            0,
            CodecKind::Bincode,
            KeyId(1),
            MsgId(1),
            5000,
        )
        .unwrap();
        let ok = unpack_and_verify(&signer, &env, 5000).unwrap();
        assert!(ok);
    }

    // T61: unpack_and_verify() 篡改 payload 返回 Ok(false)
    #[test]
    fn test_t61_unpack_and_verify_tampered_payload() {
        let signer = MockSigner;
        let mut env = pack_and_sign(
            &signer,
            b"original",
            AgentId(1),
            "/t",
            0,
            CodecKind::Bincode,
            KeyId(1),
            MsgId(1),
            5000,
        )
        .unwrap();
        env.payload[0] ^= 0xFF;
        let ok = unpack_and_verify(&signer, &env, 5000).unwrap();
        assert!(!ok);
    }

    // T62: unpack_and_verify() 篡改 signature 返回 Ok(false)
    #[test]
    fn test_t62_unpack_and_verify_tampered_signature() {
        let signer = MockSigner;
        let mut env = pack_and_sign(
            &signer,
            b"payload",
            AgentId(1),
            "/t",
            0,
            CodecKind::Bincode,
            KeyId(1),
            MsgId(1),
            5000,
        )
        .unwrap();
        env.signature[0] ^= 0xFF;
        let ok = unpack_and_verify(&signer, &env, 5000).unwrap();
        assert!(!ok);
    }

    // T63: unpack_and_verify() 时间戳过期返回 Err(StaleTimestamp)
    #[test]
    fn test_t63_unpack_and_verify_stale_timestamp() {
        let signer = MockSigner;
        let env = pack_and_sign(
            &signer,
            b"payload",
            AgentId(1),
            "/t",
            0,
            CodecKind::Bincode,
            KeyId(1),
            MsgId(1),
            1000, // header.timestamp
        )
        .unwrap();
        // now = 1000 + 5001 = 6001，过期
        let res = unpack_and_verify(&signer, &env, 6001);
        assert_eq!(res, Err(SignError::StaleTimestamp));
    }
}
