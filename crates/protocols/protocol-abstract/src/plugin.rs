//! 协议插件合同（蓝图 39 WP-39.10）。
//!
//! [`ProtocolPlugin`] 是协议**服务级**合同（35-00 §10.2 固定 seam）：
//! descriptor（能力声明）+ 配置校验（未知字段拒绝）+ 生命周期
//! （start/poll/stop，内部组合现有 [`crate::adapter::ProtocolAdapter`]
//! 与 [`crate::transport::TransportFactory`]）。ADR-0026 否决的是
//! protocol-sdk 的跨二进制 ABI 插件宏；本合同是纯 Rust trait，位于唯一
//! 协议 seam crate（protocol-abstract），不构成第二套 seam。
//!
//! # 不变量
//!
//! - `validate_config` 拒绝未消费字段（未知键 → `InvalidConfig`）；
//! - 协议写只输出 [`crate::write::PointWriteIntent`]（真正设备写仍须经
//!   蓝图 40 的 ControlDispatch 授权）；
//! - 未声明的能力不得声称可用（descriptor 是销售声明的唯一来源）。

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_upa_model::PointId;

use crate::adapter::AdapterState;
use crate::error::ProtocolError;
use crate::mapping::ProtocolPointMapping;
use crate::transport::{TransportFactory, TransportKind};
use crate::write::PointWriteIntent;

/// 协议角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRole {
    Master,
    Slave,
    Client,
    Server,
    Pub,
    Sub,
    PubSub,
}

/// 安全 profile（descriptor 声明；实施经 `channel::SecureChannelFactory`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityProfile {
    None,
    Tls12,
    Tls13,
    Iec62351,
}

/// 互操作等级（诚实命名，10.3 纪律）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InteropLevel {
    /// 规约文本缺失/待核对——codec 字节层未实现（蓝图 48 卷 BLOCKED 登记；
    /// owner 提供规约原文并落地权威 KAT 后方可升 CodecReady）。
    SpecBlocked,
    /// 仅 codec/状态机完成。
    CodecReady,
    /// host 独立 peer 验证。
    HostPeerVerified,
    /// 目标（QEMU/EL0）独立 peer 验证。
    TargetPeerVerified,
    /// 真机/HIL 验证。
    HardwareVerified,
}

/// 资源预算（descriptor 声明；超预算 poll 必须提前返回）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    /// 最大连接数。
    pub max_connections: usize,
    /// 单次 poll 最大工作量（毫秒）。
    pub max_poll_ms: u64,
    /// 最大帧/报文长度（字节）。
    pub max_frame: usize,
}

/// 协议能力描述（唯一产品声明来源）。
#[derive(Debug, Clone, Copy)]
pub struct ProtocolDescriptor {
    /// 协议 id（如 "modbus-rtu"）。
    pub id: &'static str,
    /// 协议版本。
    pub version: &'static str,
    /// 角色。
    pub role: ProtocolRole,
    /// 支持的传输种类。
    pub transport_kinds: &'static [TransportKind],
    /// 安全 profile。
    pub security_profile: SecurityProfile,
    /// 支持的点类型（`PointType` 名）。
    pub point_types: &'static [&'static str],
    /// 资源预算。
    pub resource_budget: ResourceBudget,
    /// 互操作等级（当前诚实状态）。
    pub interop_level: InteropLevel,
}

/// 协议配置（键值对；plugin 校验消费，未知键拒绝）。
#[derive(Debug, Clone, Default)]
pub struct ProtocolConfig {
    /// 配置项（键 → 值）。
    pub entries: Vec<(String, String)>,
}

impl ProtocolConfig {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// 设置配置项（已存在同键 → 替换；映射语义，禁重复键）。
    pub fn insert(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value.to_owned();
        } else {
            self.entries.push((key.to_owned(), value.to_owned()));
        }
    }
}

/// 单次 poll 的工作预算。
#[derive(Debug, Clone, Copy)]
pub struct WorkBudget {
    /// 本次 poll 最大工作量（毫秒）。
    pub max_work_ms: u64,
    /// 最大轮询次数。
    pub max_polls: u32,
}

/// poll 进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Idle,
    Busy,
    Completed,
    Degraded,
}

/// 协议健康（真实状态，不构造成功）。
#[derive(Debug, Clone)]
pub struct ProtocolHealth {
    /// 适配器状态。
    pub state: AdapterState,
    /// 最近一次 poll 时间（毫秒；0 = 从未）。
    pub last_poll_ms: u64,
    /// 累计错误数。
    pub error_count: u64,
    /// 最近错误（可空）。
    pub last_error: Option<ProtocolError>,
    /// 最近一次 poll 进度。
    pub last_progress: Progress,
}

/// 点注册表（plugin poll 时解析点路由）。
pub trait PointRegistry {
    /// 按 point_id 解析协议路由（地址 + 工程量变换）。
    fn resolve(&self, point_id: PointId) -> Option<&ProtocolPointMapping>;
    /// 点表规模（观测/容量）。
    fn len(&self) -> usize;
    /// 是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 收集全部点 id（保持插入序；45.4 插件 `start` 取用点表驱动路由）。
    ///
    /// 对象安全（`&mut dyn` 场景可用）：向调用方 Vec 追加，不返回迭代器。
    fn collect_ids(&self, out: &mut Vec<PointId>);
}

/// 具体点注册表：`Vec<ProtocolPointMapping>` 内聚（45.4 服务装配层注入；
/// 支持 `resolve` + 枚举）。
///
/// 保持插入序；以 `BTreeMap<PointId, usize>` 加速 `resolve`。
#[derive(Debug, Clone, Default)]
pub struct VecPointRegistry {
    mappings: alloc::vec::Vec<ProtocolPointMapping>,
    index: alloc::collections::BTreeMap<PointId, usize>,
}

impl VecPointRegistry {
    /// 构造（重复 point_id → 后者覆盖前者，保持首个出现序）。
    pub fn new(mappings: alloc::vec::Vec<ProtocolPointMapping>) -> Self {
        let mut index = alloc::collections::BTreeMap::new();
        for (i, m) in mappings.iter().enumerate() {
            index.insert(m.point_id, i);
        }
        Self { mappings, index }
    }
    /// 全部映射（只读）。
    pub fn mappings(&self) -> &[ProtocolPointMapping] {
        &self.mappings
    }
}

impl PointRegistry for VecPointRegistry {
    fn resolve(&self, point_id: PointId) -> Option<&ProtocolPointMapping> {
        self.index.get(&point_id).map(|&i| &self.mappings[i])
    }
    fn len(&self) -> usize {
        self.mappings.len()
    }
    fn collect_ids(&self, out: &mut Vec<PointId>) {
        out.extend(self.mappings.iter().map(|m| m.point_id));
    }
}

/// 单点读取结果（45.4 点表驱动读取；raw → 上层工程量变换/质量位映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointRead {
    /// 原始值（`ProtocolPointMapping::to_engineering(raw)` 变换）。
    pub raw: i64,
    /// 读取成功（响应正常）；超时/坏帧/异常码 → false（上层质量位映射）。
    pub ok: bool,
    /// 序号（顺序接收/重放判定）。
    pub seq: u64,
}

/// 45.4 点表消费辅助：统计注册表中地址匹配 `pred` 的点数。
///
/// 服务层已按协议过滤注入注册表；插件在本辅助内 `collect_ids` + `resolve`
/// 消费点表（**不自造地址**），返回路由点数供观测/索引。
pub fn count_matching_mappings(
    points: &mut dyn PointRegistry,
    mut pred: impl FnMut(&ProtocolPointMapping) -> bool,
) -> usize {
    let mut ids: Vec<PointId> = Vec::new();
    points.collect_ids(&mut ids);
    ids.iter()
        .filter_map(|id| points.resolve(*id))
        .filter(|m| pred(m))
        .count()
}

/// 协议插件（服务级合同）。
pub trait ProtocolPlugin {
    /// 能力描述。
    fn descriptor(&self) -> &ProtocolDescriptor;
    /// 校验配置（未知字段 → `InvalidConfig`；失败不启动）。
    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError>;
    /// 启动：经工厂获取传输并初始化适配器；随后进入 poll 循环。
    ///
    /// 39 收尾复验（P0）：`cfg` 必须是已经 `validate_config` 通过的
    /// 配置本体——校验过的字段必须真正进入运行路径（蓝图 §8：
    /// "每字段改变运行行为；unconsumed → Gate 失败"）；插件内禁止
    /// 再以 `ProtocolConfig::default()` 顶替。
    ///
    /// 45.4：`points` 为点表驱动路由事实源（插件不自造地址）——点表不再
    /// 被忽略；插件在本处 `points.collect_ids()` 消费点表（按自身协议
    /// 地址过滤/索引），后续 `read_point` 只认注入的映射。
    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        transports: &mut dyn TransportFactory,
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError>;
    /// 周期轮询（收发/超时/心跳；遵守预算）。
    fn poll(&mut self, now_ms: u64, budget: WorkBudget) -> Result<Progress, ProtocolError>;
    /// 45.4 点表驱动单点读取。默认 = 不支持（诚实；协议 I/O 未接通 →
    /// 明确 `Unsupported`，不伪造读数）。各插件按适配器能力覆写。
    fn read_point(
        &mut self,
        now_ms: u64,
        mapping: &ProtocolPointMapping,
    ) -> Result<PointRead, ProtocolError> {
        let _ = (now_ms, mapping);
        Err(ProtocolError::Unsupported)
    }
    /// 47（WP-47.1 协议出站写）：点表驱动单点写入（经适配器真实协议
    /// 事务）。默认 = 不支持（诚实；未接入写路径的协议明确
    /// `Unsupported`）。**本方法只执行**——授权已由 ControlDispatch
    /// 九步链完成，插件禁止自作授权（不双头授权）。
    fn write_point(
        &mut self,
        now_ms: u64,
        mapping: &ProtocolPointMapping,
        value: eneros_upa_model::PointValue,
    ) -> Result<(), ProtocolError> {
        let _ = (now_ms, mapping, value);
        Err(ProtocolError::Unsupported)
    }
    /// 47（WP-47.1 协议入站写）：导出本插件最近 poll 中解析到的
    /// **写意图**（全部取走，缓冲清空）。默认 = 无（不产生意图的插件
    /// 无需覆写）。**插件禁止直接写设备**——写请求只能以 [`PointWriteIntent`]
    /// 形式交给 ControlDispatch 授权链。
    fn drain_write_intents(&mut self, _out: &mut Vec<PointWriteIntent>) {}
    /// 健康。
    fn health(&self) -> ProtocolHealth;
    /// 可选调试摘要（默认 `None`；插件覆盖返回连接/状态行）。观测用，
    /// 非语义——不改变协议行为。
    fn debug_line(&self) -> Option<alloc::string::String> {
        None
    }
    /// 停止（断开连接、释放资源）。
    fn stop(&mut self) -> Result<(), ProtocolError>;
}

/// 默认 `PointRegistry`：空表（任何解析都失败——fail-closed）。
pub struct EmptyPointRegistry;

impl PointRegistry for EmptyPointRegistry {
    fn resolve(&self, _point_id: PointId) -> Option<&ProtocolPointMapping> {
        None
    }
    fn len(&self) -> usize {
        0
    }
    fn collect_ids(&self, _out: &mut Vec<PointId>) {}
}

/// 从配置中解析整数（无该键/非数字 → 错误）。
pub fn config_u64(cfg: &ProtocolConfig, key: &str) -> Result<Option<u64>, ProtocolError> {
    match cfg.get(key) {
        None => Ok(None),
        Some(v) => v
            .parse::<u64>()
            .map(Some)
            .map_err(|_| ProtocolError::InvalidConfig),
    }
}

/// 校验配置只包含已知键。
pub fn reject_unknown_keys(cfg: &ProtocolConfig, known: &[&str]) -> Result<(), ProtocolError> {
    if let Some((key, _)) = cfg
        .entries
        .iter()
        .find(|(k, _)| !known.contains(&k.as_str()))
    {
        // 未知键 → 未消费字段必须阻断（WP-39.10 不变量）。
        let _ = key;
        return Err(ProtocolError::InvalidConfig);
    }
    Ok(())
}

/// 写意图导出（协议层写请求的标准化输出）。
///
/// 蓝图 39 WP-39.10 + 47 WP-47.1：协议 poll 中解析出的写请求**只输出**
/// 意图；真正设备写仍必须经蓝图 40 的 ControlDispatch（validate → authorize →
/// dispatch → ACK → readback）。本类型禁止直接写设备。
pub fn export_write_intent(
    protocol_id: &str,
    device_id: eneros_upa_model::DeviceId,
    point_id: PointId,
    value: eneros_upa_model::PointValue,
    sequence: u64,
    issued_ms: u64,
) -> PointWriteIntent {
    PointWriteIntent {
        protocol_id: alloc::string::String::from(protocol_id),
        device_id,
        point_id,
        value,
        sequence,
        issued_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::ProtocolAddress;
    use crate::transport::TransportKind;

    #[test]
    fn reject_unknown_config_keys() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("known", "1");
        assert!(reject_unknown_keys(&cfg, &["known"]).is_ok());
        cfg.insert("unknown", "x");
        assert_eq!(
            reject_unknown_keys(&cfg, &["known"]),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn config_u64_parses_and_rejects() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("t3", "120");
        assert_eq!(config_u64(&cfg, "t3"), Ok(Some(120)));
        assert_eq!(config_u64(&cfg, "missing"), Ok(None));
        cfg.insert("bad", "abc");
        assert_eq!(config_u64(&cfg, "bad"), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn descriptor_and_write_intent_types() {
        let d = ProtocolDescriptor {
            id: "modbus-rtu",
            version: "1.0",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Serial],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 256,
            },
            interop_level: InteropLevel::CodecReady,
        };
        assert_eq!(d.id, "modbus-rtu");
        let intent = export_write_intent(
            d.id,
            7,
            PointId::from(42u32),
            eneros_upa_model::PointValue::Float(1.5),
            1,
            1000,
        );
        assert_eq!(intent.device_id, 7);
        assert_eq!(intent.point_id, PointId::from(42u32));
        assert_eq!(intent.protocol_id, "modbus-rtu");
        assert_eq!(intent.sequence, 1);
    }

    // ---- 45.4 点表驱动路由 seam ----

    fn map(point_id: u32, addr: ProtocolAddress) -> ProtocolPointMapping {
        ProtocolPointMapping {
            point_id,
            device_id: 1,
            protocol_addr: addr,
            data_type: eneros_upa_model::PointType::Analog,
            scale: 0.1,
            offset: 0.0,
            identity: crate::mapping::PointIdentityBinding::UNKNOWN,
        }
    }

    #[test]
    fn vec_registry_resolve_and_enumerate() {
        let reg = VecPointRegistry::new(vec![
            map(
                1001,
                ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 4097,
                    func_code: 3,
                },
            ),
            map(
                2001,
                ProtocolAddress::Mqtt {
                    topic: "plant/pv/power".into(),
                },
            ),
        ]);
        assert_eq!(reg.len(), 2);
        // resolve 命中/未命中。
        assert!(reg.resolve(1001).is_some());
        assert!(
            reg.resolve(1001).unwrap().protocol_addr
                == ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 4097,
                    func_code: 3
                }
        );
        assert!(reg.resolve(9999).is_none());
        // 枚举保持插入序（45.4 插件点表驱动）。
        let mut ids = Vec::new();
        reg.collect_ids(&mut ids);
        assert_eq!(ids, vec![1001, 2001]);
    }

    #[test]
    fn empty_registry_fails_closed() {
        let reg = EmptyPointRegistry;
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
        assert!(reg.resolve(1).is_none());
        let mut ids = Vec::new();
        reg.collect_ids(&mut ids);
        assert!(ids.is_empty());
    }

    #[test]
    fn count_matching_mappings_filters_by_address() {
        let mut reg = VecPointRegistry::new(vec![
            map(
                1001,
                ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 4097,
                    func_code: 3,
                },
            ),
            map(
                2001,
                ProtocolAddress::Mqtt {
                    topic: "plant/pv/power".into(),
                },
            ),
            map(
                2002,
                ProtocolAddress::Mms {
                    object_ref: "LD0/LRPW.Analogue.MX.w".into(),
                },
            ),
        ]);
        let n = count_matching_mappings(&mut reg, |m| {
            matches!(m.protocol_addr, ProtocolAddress::Modbus { .. })
        });
        assert_eq!(n, 1);
        let n_mqtt = count_matching_mappings(&mut reg, |m| {
            matches!(m.protocol_addr, ProtocolAddress::Mqtt { .. })
        });
        assert_eq!(n_mqtt, 1);
        let n_ms = count_matching_mappings(&mut reg, |m| {
            matches!(m.protocol_addr, ProtocolAddress::Mms { .. })
        });
        assert_eq!(n_ms, 1);
        // 订阅型（goose/sv）：全部路由（服务层已按协议过滤注入）。
        assert_eq!(count_matching_mappings(&mut reg, |_| true), 3);
    }

    // read_point 默认实现：诚实 Unsupported（未覆写协议插件不伪造读数）。
    #[test]
    fn read_point_default_is_unsupported() {
        struct NoRead;
        impl ProtocolPlugin for NoRead {
            fn descriptor(&self) -> &ProtocolDescriptor {
                static D: ProtocolDescriptor = ProtocolDescriptor {
                    id: "x",
                    version: "0",
                    role: ProtocolRole::Master,
                    transport_kinds: &[],
                    security_profile: SecurityProfile::None,
                    point_types: &[],
                    resource_budget: ResourceBudget {
                        max_connections: 0,
                        max_poll_ms: 0,
                        max_frame: 0,
                    },
                    interop_level: InteropLevel::CodecReady,
                };
                &D
            }
            fn validate_config(&self, _cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
                Ok(())
            }
            fn start(
                &mut self,
                _c: &ProtocolConfig,
                _t: &mut dyn crate::transport::TransportFactory,
                _p: &mut dyn PointRegistry,
            ) -> Result<(), ProtocolError> {
                Ok(())
            }
            fn poll(&mut self, _n: u64, _b: WorkBudget) -> Result<Progress, ProtocolError> {
                Ok(Progress::Idle)
            }
            fn health(&self) -> ProtocolHealth {
                ProtocolHealth {
                    state: AdapterState::Stopped,
                    last_poll_ms: 0,
                    error_count: 0,
                    last_error: None,
                    last_progress: Progress::Idle,
                }
            }
            fn stop(&mut self) -> Result<(), ProtocolError> {
                Ok(())
            }
        }
        let mut p = NoRead;
        assert_eq!(
            p.read_point(
                0,
                &map(
                    1,
                    ProtocolAddress::Modbus {
                        slave_addr: 1,
                        reg_addr: 1,
                        func_code: 3
                    }
                )
            ),
            Err(ProtocolError::Unsupported)
        );
    }
}
