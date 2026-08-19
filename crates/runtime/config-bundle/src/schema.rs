//! 各 ACTIVE 配置的 canonical 字段 id（单一事实；host 编译面与 target
//! 解码面共用——禁止两侧各自定义漂移）。
//!
//! 规则：field id 从 1 起连续分配；0 保留。新增字段 = 追加 id（schema
//! minor 升版）；删除/重定义字段 = 升 major 并拒绝旧 id。

/// `control-write`（consumer: svc-control-dispatch；schema v1）
pub mod control_write {
    pub const CONFIG_ID: &str = "control-write";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 3;
    /// 1：租约 ID，16B 二进制（两侧同源，产线烧录）。
    pub const FIELD_LEASE_ID: u8 = 1;
    /// 2：legacy_input 策略，u8（1=DenyAndLog；其余拒绝）。
    pub const FIELD_LEGACY_INPUT: u8 = 2;
    /// 3：handshake_retries，u8（≤5）。
    pub const FIELD_HANDSHAKE_RETRIES: u8 = 3;
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
/// canonical payload 单一字段 `FIELD_POINTS`，值为自描述子格式：
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
/// ```
///
/// 地址编码（与 `ProtocolAddress` 直映；goose/sv 为订阅绑定，addr_len=0）：
/// - modbus-rtu/tcp：`[slave u8][reg u16 LE][func u8]`（4B）
/// - iec104：`[common u16 LE][ioa u16 LE][type u8]`（5B）
/// - mms：object_ref UTF-8 字节
/// - mqtt：topic UTF-8 字节
pub mod points {
    pub const CONFIG_ID: &str = "points";
    pub const SCHEMA: u16 = 1;
    pub const MAX_FIELD: u8 = 1;
    /// 1：整个点表（自描述子格式，见模块注释）。
    pub const FIELD_POINTS: u8 = 1;
    /// 点表最大点数（防御性上界）。
    pub const MAX_POINTS: u32 = 1024;
    /// 采集周期上限（毫秒；默认 1 小时）。
    pub const MAX_PERIOD_MS: u32 = 3_600_000;
    /// 采集周期默认值（毫秒）。
    pub const DEFAULT_PERIOD_MS: u32 = 1_000;
    /// 地址最大长度（MQTT topic / MMS object_ref 上限）。
    pub const MAX_ADDR_LEN: usize = 256;
    // 协议路由判别码（对应 points.toml `protocol` 枚举；插件路由，非点模型）。
    pub const PROTO_MODBUS_RTU: u8 = 1;
    pub const PROTO_MODBUS_TCP: u8 = 2;
    pub const PROTO_IEC104: u8 = 3;
    pub const PROTO_MMS: u8 = 4;
    pub const PROTO_GOOSE: u8 = 5;
    pub const PROTO_SV: u8 = 6;
    pub const PROTO_MQTT: u8 = 7;
}
