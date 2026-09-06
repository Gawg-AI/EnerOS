//! 各 ACTIVE 配置的 canonical 字段 id（单一事实；host 编译面与 target
//! 解码面共用——禁止两侧各自定义漂移）。
//!
//! 规则：field id 从 1 起连续分配；0 保留。新增字段 = 追加 id（schema
//! minor 升版）；删除/重定义字段 = 升 major 并拒绝旧 id。

/// `control-write`（consumer: svc-control-dispatch；schema v1）
pub mod control_write {
    pub const CONFIG_ID: &str = "control-write";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 4;
    /// 1：租约 ID，16B 二进制（两侧同源，产线烧录）。
    pub const FIELD_LEASE_ID: u8 = 1;
    /// 2：legacy_input 策略，u8（1=DenyAndLog；其余拒绝）。
    pub const FIELD_LEGACY_INPUT: u8 = 2;
    /// 3：handshake_retries，u8（≤5）。
    pub const FIELD_HANDSHAKE_RETRIES: u8 = 3;
    /// 4（47 WP-47.1 速率可调）：dispatch rate gate 每小时配额 u32
    ///（缺省 = 产品默认 60/h；qemu-test 装配经 bundle 注入放宽值）。
    pub const FIELD_RATE_PER_HOUR: u8 = 4;
    pub const LEGACY_INPUT_DENY_AND_LOG: u8 = 1;
    pub const LEASE_ID_LEN: usize = 16;
    pub const HANDSHAKE_RETRIES_MAX: u8 = 5;
}

/// `ota`（consumer: svc-ota；schema v1）
pub mod ota {
    pub const CONFIG_ID: &str = "ota";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 12;
    pub const FIELD_META_BLOCK: u8 = 1; // [partition] u64
    pub const FIELD_SLOT_A_BASE: u8 = 2; // u64
    pub const FIELD_PARTITION_BLOCKS: u8 = 3; // u64
    pub const FIELD_SLOT_B_BASE: u8 = 4; // u64
    pub const FIELD_CHUNK_SIZE: u8 = 5; // [upgrade] u32
    pub const FIELD_MAX_BOOT_ATTEMPTS: u8 = 6; // u32
    pub const FIELD_DOWNLOAD_TIMEOUT_SECS: u8 = 7; // u32
    pub const FIELD_DOWNLOAD_RETRIES: u8 = 8; // u32
    pub const FIELD_TOTAL_DOWNLOAD_TIMEOUT_SECS: u8 = 9; // u32
    pub const FIELD_ROOT_KEY: u8 = 10; // [security] 64B SM2 未压缩点（构建期注入）
    pub const FIELD_SIGNATURE_MANDATORY: u8 = 11; // u8（必须为 1）
    pub const FIELD_SM3_CONSTANT_TIME: u8 = 12; // u8（必须为 1；常时比较无旁路）
    pub const ROOT_KEY_LEN: usize = 64;
}

/// `cloud-sync`（consumer: svc-cloud-sync；schema v1）
pub mod cloud_sync {
    pub const CONFIG_ID: &str = "cloud-sync";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 6;
    pub const FIELD_MAX_EVENTS: u8 = 1; // [event_store] u32
    pub const FIELD_BATCH_SIZE: u8 = 2; // [delta_sync] u32
    pub const FIELD_COMPRESSION: u8 = 3; // u8（1=None）
    pub const FIELD_MAX_RETRIES: u8 = 4; // u32
    pub const FIELD_BACKOFF_BASE_MS: u8 = 5; // u64
    pub const FIELD_DEAD_LETTER_CAPACITY: u8 = 6; // u32
    pub const COMPRESSION_NONE: u8 = 1;
}

/// `metrics`（consumer: svc-observability；schema v1）
pub mod metrics {
    pub const CONFIG_ID: &str = "metrics";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 5;
    pub const FIELD_FLUSH_INTERVAL_MS: u8 = 1; // [collector] u64
    pub const FIELD_BUFFER_CAPACITY: u8 = 2; // u32
    pub const FIELD_PER_METRIC_CAPACITY: u8 = 3; // [store] u32
    pub const FIELD_RETENTION: u8 = 4; // u8（1=drop_oldest；2=reject_new）
    pub const FIELD_DEFAULTS_MASK: u8 = 5; // [defaults] u32 bitmask（bit0..bit6 顺序见下）
    pub const RETENTION_DROP_OLDEST: u8 = 1;
    pub const RETENTION_REJECT_NEW: u8 = 2;
    pub const DEFAULTS_KEYS: [&str; 7] = [
        "system_cpu_usage",
        "system_mem_usage",
        "system_temperature",
        "agent_active_count",
        "agent_request_latency",
        "agent_error_count",
        "business_control_commands",
        // business_optimization_calls = bit7
    ];
}

/// `logging`（consumer: svc-observability；schema v1）
pub mod logging {
    pub const CONFIG_ID: &str = "logging";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 5;
    pub const FIELD_MIN_LEVEL: u8 = 1; // [logger] u8（0=ERROR..4=TRACE）
    pub const FIELD_BUFFER_CAPACITY: u8 = 2; // u32
    pub const FIELD_STORAGE_CAPACITY: u8 = 3; // [storage] u32
    pub const FIELD_ROTATION: u8 = 4; // u8（1=drop_oldest；2=reject_new）
    pub const FIELD_MAX_CUSTOM_FIELDS: u8 = 5; // [fields] u32
    pub const LEVEL_ERROR: u8 = 0;
    pub const LEVEL_WARN: u8 = 1;
    pub const LEVEL_INFO: u8 = 2;
    pub const LEVEL_DEBUG: u8 = 3;
    pub const LEVEL_TRACE: u8 = 4;
    pub const ROTATION_DROP_OLDEST: u8 = 1;
    pub const ROTATION_REJECT_NEW: u8 = 2;
}

/// `points`（consumer: svc-telemetry + 7 协议服务；schema v1）——蓝图 45
/// 点表：协议↔遥测唯一映射事实源。
///
/// 蓝图 61 v0.314.27 起字段帧扩展（`MAX_FIELD=3`）：
/// - `FIELD_POINTS`（id=1）：v1 点表（无身份绑定；N-1 读窗口保留——
///   新解码面遇旧 bundle 回退本字段，identity 全 UNKNOWN）；
/// - `FIELD_SOURCES`（id=2）：来源注册表（[[source]] 编译产物）；
/// - `FIELD_POINTS_V2`（id=3）：v2 点表（每条记录尾部追加 24B 身份块）。
///
/// 编码面（新 bundle）只写 id=2+3；旧读方面对未知 id → `UnknownField`
/// 显式拒绝（§4.4：不把新字节误解为旧状态）。新读方面对旧 bundle →
/// 回退 id=1（identity 全 UNKNOWN，不满足控制快照资格）。
///
/// v1/v2 点表子格式（v2 = 每条记录追加身份块）：
///
/// ```text
/// u32 LE: point count（≤ MAX_POINTS）
/// 每条点记录：
///   u32 LE  point_id           （全表唯一，fail-closed）
///   u16 LE  device_id
///   u8      protocol 判别码    （PROTO_*；插件路由，非点模型）
///   u8      data_type 序号     （upa-model PointType：0..=4）
///   f64 LE  scale              （禁 0）
///   f64 LE  offset
///   u32 LE  period_ms          （1..=MAX_PERIOD_MS）
///   f64 LE  deadband
///   u8      range 标志（0/1）
///   f64 LE  min（标志=1）
///   f64 LE  max（标志=1）      （min ≤ max）
///   u16 LE  addr_len（≤ MAX_ADDR_LEN）
///   addr 字节（按协议编码见下）
///   [v2 only] 身份块 24B：
///     u16 LE unit_code（UnitCatalog；0=未知）
///     u32 LE schema_id（0=未注册）
///     u16 LE schema_version
///     u64 LE asset_id（0=未绑定）
///     u64 LE mapping_generation（0=未定代；consumer apply 期以 bundle
///            generation 覆盖注入）
/// ```
///
/// 地址编码（与 `ProtocolAddress` 直映；goose/sv 为订阅绑定，addr_len=0）：
/// - modbus-rtu/tcp：`[slave u8][reg u16 LE][func u8]`（4B）
/// - iec104：`[common u16 LE][ioa u16 LE][type u8]`（5B）
/// - mms：object_ref UTF-8 字节
/// - mqtt：topic UTF-8 字节
///
/// 来源注册表子格式（`FIELD_SOURCES`；蓝图 61 [02] §4.2）：
///
/// ```text
/// u32 LE: source count（≤ MAX_SOURCES）
/// 每条来源记录：
///   u32 LE  source_id（≠0，全表唯一）
///   u32 LE  tenant_id
///   u32 LE  site_id
///   u8      protocol 判别码（PROTO_*）
///   u8      flags（bit0 allow_control，bit1 require_integrity）
///   u32 LE  point_min（0=不设下界）
///   u32 LE  point_max（0=不设上界）
///   u32 LE  max_events_per_s（[1, 100000]）
/// ```
pub mod points {
    pub const CONFIG_ID: &str = "points";
    pub const SCHEMA: u16 = 1;
    /// 蓝图 61：3 个字段帧（v1 点表/来源注册表/v2 点表）。
    pub const MAX_FIELD: u8 = 3;
    /// 1：点表 v1（自描述子格式，见模块注释；N-1 读窗口）。
    pub const FIELD_POINTS: u8 = 1;
    /// 2：来源注册表（蓝图 61 v0.314.27）。
    pub const FIELD_SOURCES: u8 = 2;
    /// 3：点表 v2（记录尾随 24B 身份块；生产编码面唯一输出）。
    pub const FIELD_POINTS_V2: u8 = 3;
    /// 点表最大点数（防御性上界）。
    pub const MAX_POINTS: u32 = 1024;
    /// 来源注册表最大条数（防御性上界）。
    pub const MAX_SOURCES: u32 = 64;
    /// 采集周期上限（毫秒；默认 1 小时）。
    pub const MAX_PERIOD_MS: u32 = 3_600_000;
    /// 采集周期默认值（毫秒）。
    pub const DEFAULT_PERIOD_MS: u32 = 1_000;
    /// 地址最大长度（MQTT topic / MMS object_ref 上限）。
    pub const MAX_ADDR_LEN: usize = 256;
    /// 事件率上限（事件/秒）。
    pub const MAX_EVENTS_PER_S: u32 = 100_000;
    /// 来源 flags：允许进入控制快照。
    pub const SOURCE_FLAG_ALLOW_CONTROL: u8 = 1 << 0;
    /// 来源 flags：要求完整性认证。
    pub const SOURCE_FLAG_REQUIRE_INTEGRITY: u8 = 1 << 1;
    // 协议路由判别码（对应 points.toml `protocol` 枚举；插件路由，非点模型）。
    pub const PROTO_MODBUS_RTU: u8 = 1;
    pub const PROTO_MODBUS_TCP: u8 = 2;
    pub const PROTO_IEC104: u8 = 3;
    pub const PROTO_MMS: u8 = 4;
    pub const PROTO_GOOSE: u8 = 5;
    pub const PROTO_SV: u8 = 6;
    pub const PROTO_MQTT: u8 = 7;
}

/// `ai-peer`（consumer: svc-ai-worker；schema v1；49 WP-49.2 远程 dual-brain
/// 状态机参数——全部必配 fail-closed，禁凭记忆缺省）
pub mod ai_peer {
    pub const CONFIG_ID: &str = "ai-peer";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 8;
    /// 1：在途请求超时毫秒 u64（值域 [100, 60000]）。
    pub const FIELD_TIMEOUT_MS: u8 = 1;
    /// 2：连续超时→Unhealthy 阈值 u32（[1, 10]）。
    pub const FIELD_UNHEALTHY_AFTER: u8 = 2;
    /// 3：Unhealthy 探测周期 tick 数 u32（[1, 100]）。
    pub const FIELD_PROBE_EVERY: u8 = 3;
    /// 4：功率幅值预算 kW f64 LE bits（(0, 1000]）。
    pub const FIELD_MAX_POWER_KW: u8 = 4;
    /// 5：提案 TTL 上限秒 u32（[1, 3600]）。
    pub const FIELD_TTL_SECS: u8 = 5;
    /// 6：有界学习闸（SPEC-E WP-E.3）u8 0/1（EAQ-2：dev/qemu=true、
    /// release 归 owner 配置裁决——值必配，无代码缺省）。
    pub const FIELD_LEARN_ENABLED: u8 = 6;
    /// 7：D3 降级 lease HMAC 键 32B（蓝图 68.5 WP-68.5.3；**不落 TOML**
    /// ——compile_security 同款注入链：`--degrade-lease-key-hex` >
    /// `ENEROS_DEGRADE_LEASE_KEY` > `$ENEROS_KMS_DIR/degrade-lease.key`，
    /// 全缺 fail-closed；开发档模板 configs/kms-dev/degrade-lease.key）。
    pub const FIELD_DEGRADE_LEASE_KEY: u8 = 7;
    /// 8：D3 降级 lease 有效时长秒 u32（[60, 86400]；toml 可载非密）。
    pub const FIELD_DEGRADE_LEASE_TTL_SECS: u8 = 8;

    pub const TIMEOUT_MS_MIN: u64 = 100;
    pub const TIMEOUT_MS_MAX: u64 = 60_000;
    pub const UNHEALTHY_AFTER_MIN: u32 = 1;
    pub const UNHEALTHY_AFTER_MAX: u32 = 10;
    pub const PROBE_EVERY_MIN: u32 = 1;
    pub const PROBE_EVERY_MAX: u32 = 100;
    pub const MAX_POWER_KW_MAX: f64 = 1000.0;
    pub const TTL_SECS_MAX: u32 = 3600;
    pub const DEGRADE_LEASE_TTL_SECS_MIN: u32 = 60;
    pub const DEGRADE_LEASE_TTL_SECS_MAX: u32 = 86_400;
}

/// `security`（consumer: svc-control-dispatch / svc-device-agent；
/// SPEC-SERIES-2026-08-27/A WP-A.1；schema v1）
pub mod security {
    /// 配置 ID。
    pub const CONFIG_ID: &str = "security";
    /// payload schema 版本。
    pub const SCHEMA: u16 = 1;
    /// 允许的最大字段 id。
    pub const MAX_FIELD: u8 = 1;
    /// 控制链信封 HMAC-SM3 密钥（32B；构建期 KMS 注入链取得，文件不落明文）。
    pub const FIELD_CONTROL_CHAIN_KEY: u8 = 1;
    /// 信封密钥长度。
    pub const CONTROL_CHAIN_KEY_LEN: usize = 32;
}

/// `time_gate`（SPEC-SERIES-2026-08-27/B WP-B.1，BAQ-2）：墙钟 epoch 合理性
/// 闸门常量与判定——root spawn/restart 在把 RTC 读数经 X6 下发 EL0 服务前
/// 必须过此闸门；垃圾时间（电池失效全零、出厂前旧钟、溢出级未来值）不得
/// 污染审计/SOE 时间戳。阈值常量集中于此（owner 可调面），不散落 root-task。
pub mod time_gate {
    /// 下限：2026-01-01T00:00:00Z。低于它 = 出厂失效/严重过期（含 PL031
    /// 电池失效回退的 epoch(0)），判不可信。
    pub const WALL_EPOCH_MIN_SEC: u64 = 1_767_225_600;
    /// 上限：2100-01-01T00:00:00Z。
    ///
    /// BAQ-2 原建议"服务构建时刻+30 天"为浮动基准——把构建墙钟嵌进镜像会
    /// 复活 DEF-52-03（镜像非确定构建）P1，且部署超期产品被误杀；改用
    /// 固定远期天花板实现同一意图（拦截溢出级/荒谬跳变），确定性可复现。
    pub const WALL_EPOCH_MAX_SEC: u64 = 4_102_444_800;

    /// 闸门四臂裁决（C1 判据：合法/过旧/超未来/零）。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WallEpochVerdict {
        /// 在合理窗内，可注入墙钟偏移。
        Accept,
        /// 早于下限（含出厂钟未校时）——按缺失降级。
        Stale,
        /// 晚于上限（跳变/溢出）——按缺失降级。
        Future,
        /// 零 = RTC 缺失或读数全零——显式降级 `TIME-EPOCH-MISSING`。
        Missing,
    }

    /// 对 epoch 秒做四臂裁决。任何非 `Accept` 分支 root 一律以 X6=0 下发，
    /// 服务侧显式打印降级行——绝不静默用 0 冒充可信时间。
    pub fn gate(epoch_sec: u64) -> WallEpochVerdict {
        match epoch_sec {
            0 => WallEpochVerdict::Missing,
            s if s < WALL_EPOCH_MIN_SEC => WallEpochVerdict::Stale,
            s if s > WALL_EPOCH_MAX_SEC => WallEpochVerdict::Future,
            _ => WallEpochVerdict::Accept,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn time_gate_four_verdicts() {
            assert_eq!(gate(0), WallEpochVerdict::Missing);
            assert_eq!(gate(1), WallEpochVerdict::Stale);
            // 1970..=2025 全区间 stale（抽两端）。
            assert_eq!(gate(WALL_EPOCH_MIN_SEC - 1), WallEpochVerdict::Stale);
            assert_eq!(gate(WALL_EPOCH_MIN_SEC), WallEpochVerdict::Accept);
            assert_eq!(gate(WALL_EPOCH_MAX_SEC), WallEpochVerdict::Accept);
            assert_eq!(gate(WALL_EPOCH_MAX_SEC + 1), WallEpochVerdict::Future);
            // u64 溢出级荒谬值落 Future。
            assert_eq!(gate(u64::MAX), WallEpochVerdict::Future);
        }
    }
}

/// `catalog`（蓝图 61 v0.314.33 SemanticCatalog 编译源；consumer:
/// svc-telemetry / svc-control-dispatch / svc-device-agent；schema v1）
///
/// 人类维护的工程 TOML 在 host 编译为封闭二进制 payload（单一事实）；
/// target 只读编译产物，不解析任意企业文档。字段语义与 `points` 同纪律：
/// field id 从 1 连续分配、0 保留；各表为 `count u32 LE ‖ 定长记录 × n`
/// 子格式，记录内子字段固定顺序小端（见 `toml_compile::compile_catalog`
/// 编码序——禁止依赖 Rust 内存布局）。
pub mod catalog {
    /// 配置 ID。
    pub const CONFIG_ID: &str = "catalog";
    /// payload schema 版本。
    pub const SCHEMA: u16 = 1;
    /// 允许的最大字段 id。
    pub const MAX_FIELD: u8 = 13;

    /// 1：目录头（64B 定长：generation u64 ‖ valid_from_ns u64 ‖
    /// valid_until_ns u64 ‖ mapping_version u64 ‖ topology_version u64 ‖
    /// asset_version u64 ‖ contract_version u64 ‖ safety_version u64）。
    pub const FIELD_HEADER: u8 = 1;
    /// 2：schema 注册表；记录 = schema_id u32 ‖ version u16 ‖ flags u16（8B）。
    pub const FIELD_SCHEMAS: u8 = 2;
    /// 3：单位受控换算图；记录 = from u16 ‖ to u16 ‖ factor f64（12B；
    /// 量纲一致性由 decoder 经 UnitCatalog 强制）。
    pub const FIELD_UNIT_CONVERSIONS: u8 = 3;
    /// 4：资产目录；记录 = asset_id u64 ‖ site_id u32 ‖ tenant_id u32 ‖
    /// kind u16 ‖ device_id u16 ‖ topology_node u64 ‖ flags u16 ‖ pad u16
    /// （32B；flags 位 0 = tombstone 不可用）。
    pub const FIELD_ASSETS: u8 = 4;
    /// 5：拓扑节点；记录 = id u64 ‖ kind u16 ‖ flags u16（12B）。
    pub const FIELD_TOPO_NODES: u8 = 5;
    /// 6：拓扑边；记录 = from u64 ‖ to u64 ‖ kind u16 ‖ switch_point u32 ‖
    /// capacity_kw f64 ‖ loss f64 ‖ valid_from_ns u64 ‖ valid_until_ns u64
    /// ‖ flags u16（56B；kind=switch 必须带 switch_point 观测点）。
    pub const FIELD_TOPO_EDGES: u8 = 6;
    /// 7：点语义目录；记录 = point_id u32 ‖ asset_id u64 ‖ schema_id u32 ‖
    /// schema_version u16 ‖ unit_code u16 ‖ role u8 ‖ readback_point u32 ‖
    /// flags u8（26B；role=control 必须带 readback_point≠0）。
    pub const FIELD_POINT_SEMANTICS: u8 = 7;
    /// 8：反馈槽位（与生产 TrustedFeedbackBuilder 槽序对齐）；记录 =
    /// slot u8 ‖ point_id u32 ‖ pad u8（6B；slot 码见 [`FEEDBACK_SLOT_*`]）。
    pub const FIELD_FEEDBACK_SLOTS: u8 = 8;
    /// 9：安全覆盖；记录 = point_id u32 ‖ min f64 ‖ max f64 ‖
    /// period_ms u32 ‖ unit u16 ‖ flags u16（28B）。
    pub const FIELD_COVERAGE: u8 = 9;
    /// 10：控制合同；记录 = contract_id u32 ‖ asset_id u64 ‖ point_id u32 ‖
    /// unit u16 ‖ class u16 ‖ min f64 ‖ max f64 ‖ ramp_kw_per_s f64 ‖
    /// settle_ms u32 ‖ min_on_ms u64 ‖ min_off_ms u64 ‖ max_starts u16 ‖
    /// readback_point u32 ‖ tolerance_kw f64 ‖ reversible u8 ‖
    /// compensation u8 ‖ priority u8 ‖ safe_state u8 ‖ lost_link u8 ‖
    /// flags u8 ‖ pad×3（87B）。
    pub const FIELD_CONTROL_CONTRACTS: u8 = 10;
    /// 11：安全约束目录；记录 = constraint_id u32 ‖ asset_id u64 ‖
    /// kind u16 ‖ point_id u32 ‖ limit f64 ‖ flags u16 ‖ pad u32（32B）。
    pub const FIELD_SAFETY_CONSTRAINTS: u8 = 11;
    /// 12：旧 ID 别名（迁移：market DevicePool u64 / VPP resource_id →
    /// asset_id）；记录 = kind u8 ‖ legacy_id u64 ‖ asset_id u64 ‖ pad u8
    /// （18B；kind=1 market-device、2 vpp-resource）。
    pub const FIELD_ALIASES: u8 = 12;
    /// 13：tombstone 登记（删除走墓碑；ID 永不复用）；记录 =
    /// asset_id u64 ‖ kind u16 ‖ flags u16（12B）。
    pub const FIELD_TOMBSTONES: u8 = 13;

    /// 头部定长（字段 1）。
    pub const HEADER_LEN: usize = 64;
    /// 单条 schema 记录长（字段 2）。
    pub const SCHEMA_REC_LEN: usize = 8;
    /// 单条单位换算记录长（字段 3）。
    pub const UNIT_CONV_REC_LEN: usize = 12;
    /// 单条资产记录长（字段 4）。
    pub const ASSET_REC_LEN: usize = 32;
    /// 单条拓扑节点记录长（字段 5）。
    pub const TOPO_NODE_REC_LEN: usize = 12;
    /// 单条拓扑边记录长（字段 6）。
    pub const TOPO_EDGE_REC_LEN: usize = 56;
    /// 单条点语义记录长（字段 7）。
    pub const POINT_SEM_REC_LEN: usize = 26;
    /// 单条反馈槽记录长（字段 8）。
    pub const FEEDBACK_SLOT_REC_LEN: usize = 6;
    /// 单条安全覆盖记录长（字段 9）。
    pub const COVERAGE_REC_LEN: usize = 28;
    /// 单条控制合同记录长（字段 10）。
    pub const CONTROL_CONTRACT_REC_LEN: usize = 87;
    /// 单条安全约束记录长（字段 11）。
    pub const SAFETY_CONSTRAINT_REC_LEN: usize = 32;
    /// 单条别名记录长（字段 12）。
    pub const ALIAS_REC_LEN: usize = 18;
    /// 单条 tombstone 记录长（字段 13）。
    pub const TOMBSTONE_REC_LEN: usize = 12;

    /// 表项防御性上界（封闭 payload 无界拒绝）。
    pub const MAX_SCHEMAS: u32 = 64;
    pub const MAX_UNIT_CONVERSIONS: u32 = 64;
    pub const MAX_ASSETS: u32 = 1024;
    pub const MAX_TOPO_NODES: u32 = 1024;
    pub const MAX_TOPO_EDGES: u32 = 4096;
    pub const MAX_POINT_SEMANTICS: u32 = 4096;
    pub const MAX_FEEDBACK_SLOTS: u32 = 16;
    pub const MAX_COVERAGE: u32 = 512;
    pub const MAX_CONTROL_CONTRACTS: u32 = 512;
    pub const MAX_SAFETY_CONSTRAINTS: u32 = 1024;
    pub const MAX_ALIASES: u32 = 1024;
    pub const MAX_TOMBSTONES: u32 = 1024;

    /// 资产 flags：位 0 = tombstone（已删除，ID 不复用）。
    pub const ASSET_FLAG_TOMBSTONE: u16 = 1 << 0;

    /// 点语义 role 码。
    pub const ROLE_MEASUREMENT: u8 = 1;
    pub const ROLE_CONTROL: u8 = 2;
    pub const ROLE_SWITCH_STATE: u8 = 3;

    /// 反馈槽码（与 TrustedFeedbackBuilder 槽序对齐）。
    pub const FEEDBACK_SLOT_GRID_UP: u8 = 1;
    pub const FEEDBACK_SLOT_SOC: u8 = 2;
    pub const FEEDBACK_SLOT_DC_BUS_V: u8 = 3;
    pub const FEEDBACK_SLOT_OVERCURRENT: u8 = 4;
    pub const FEEDBACK_SLOT_OVERTEMP: u8 = 5;
    /// 必选反馈槽数（编译期强制 5 槽齐全）。
    pub const FEEDBACK_SLOT_COUNT: u8 = 5;

    /// 拓扑边 kind 码。
    pub const EDGE_KIND_SWITCH: u16 = 1;
    /// 拓扑分支边（无开关状态；始终可导通）。
    pub const EDGE_KIND_BRANCH: u16 = 2;
    /// 拓扑切换边必须带开关状态观测点（不可观测的开关 = 未知态）。
    pub const EDGE_SWITCH_POINT_REQUIRED: bool = true;
    /// 拓扑节点 kind 码。
    pub const NODE_KIND_GRID: u16 = 10;

    /// 控制合同 class 码。
    pub const CONTRACT_CLASS_SWITCH: u16 = 1;
    pub const CONTRACT_CLASS_SETPOINT: u16 = 2;

    /// 安全约束 kind 码。
    pub const SAFETY_KIND_OVERCURRENT: u16 = 1;
    pub const SAFETY_KIND_OVERTEMP: u16 = 2;

    /// 别名 kind 码。
    pub const ALIAS_KIND_MARKET_DEVICE: u8 = 1;
    pub const ALIAS_KIND_VPP_RESOURCE: u8 = 2;
}

/// `catalog` 资产/点语义 kind 码（[`catalog`] 模块常量族的语义旁注——
/// 资产 kind 闭枚举：1=Battery 2=PV 3=Wind 4=Generator 5=FlexibleLoad
/// 6=FixedLoad 7=EVSE 8=HVAC 9=ProcessUnit 10=GridConnection 11=Aggregate）。
pub mod catalog_kinds {
    pub const ASSET_BATTERY: u16 = 1;
    pub const ASSET_PV: u16 = 2;
    pub const ASSET_WIND: u16 = 3;
    pub const ASSET_GENERATOR: u16 = 4;
    pub const ASSET_FLEXIBLE_LOAD: u16 = 5;
    pub const ASSET_FIXED_LOAD: u16 = 6;
    pub const ASSET_EVSE: u16 = 7;
    pub const ASSET_HVAC: u16 = 8;
    pub const ASSET_PROCESS_UNIT: u16 = 9;
    pub const ASSET_GRID_CONNECTION: u16 = 10;
    pub const ASSET_AGGREGATE: u16 = 11;

    /// 拓扑节点 kind 闭枚举（[`catalog::FIELD_TOPO_NODES`]）。
    pub const NODE_BUSBAR: u16 = 1;
    pub const NODE_FEEDER: u16 = 2;
    pub const NODE_TRANSFORMER: u16 = 3;
    pub const NODE_SWITCH: u16 = 4;
    pub const NODE_DER: u16 = 5;
    pub const NODE_LOAD: u16 = 6;
    pub const NODE_STORAGE: u16 = 7;
    pub const NODE_METER: u16 = 8;
    pub const NODE_PROCESS_UNIT: u16 = 9;
    pub const NODE_GRID: u16 = 10;
}
