//! 审计事件与哈希链数据结构定义.
//!
//! 事件字段采用 `heapless::String` 定容存储（actor ≤ 64B / target ≤ 128B /
//! detail ≤ 256B），保证 no_std 无堆场景可用，且编码长度有界（actor/target
//! 长度可用 u8 前缀表达）。

/// 审计事件类型（12 类，判别值 1~12 连续）.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventType {
    /// 认证登录.
    AuthLogin = 1,
    /// 认证登出.
    AuthLogout = 2,
    /// 配置变更.
    ConfigChange = 3,
    /// 控制命令下发.
    ControlCommand = 4,
    /// 模型加载.
    ModelLoad = 5,
    /// 密钥使用.
    KeyUse = 6,
    /// Agent 启动.
    AgentStart = 7,
    /// Agent 停止.
    AgentStop = 8,
    /// 联邦加入.
    FederateJoin = 9,
    /// 联邦退出.
    FederateLeave = 10,
    /// 安全告警.
    SecurityAlert = 11,
    /// AI 决策（W5.1：意图/LP 解/安全校验/执行/偏差入链可追溯）.
    AiDecision = 12,
}

/// 审计事件（`seq == 0` 表示由日志器自动赋序）.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// 事件序号（链内严格递增；0 = 自动赋值）.
    pub seq: u64,
    /// 事件时间戳（毫秒，由调用方提供，no_std 无系统时钟）.
    pub timestamp_ms: u64,
    /// 事件类型.
    pub event_type: AuditEventType,
    /// 操作者标识（≤ 64 字节）.
    pub actor: heapless::String<64>,
    /// 操作对象标识（≤ 128 字节）.
    pub target: heapless::String<128>,
    /// 事件详情（≤ 256 字节）.
    pub detail: heapless::String<256>,
    /// 严重级别（0=最低，数值越大越严重）.
    pub severity: u8,
    /// 来源 IPv4 地址.
    pub source_ip: [u8; 4],
}

impl AuditEvent {
    /// 构造便捷事件：`seq`/`timestamp_ms`/`severity` 默认 0，`source_ip` 默认
    /// `[0; 4]`；超长字符串静默截断到定容上限（调用方应保证长度合规）.
    pub fn new(event_type: AuditEventType, actor: &str, target: &str, detail: &str) -> Self {
        let mut actor_s = heapless::String::<64>::new();
        // 定容字符串写入：长度合规时必然成功，超长忽略（测试构造辅助）
        let _ = actor_s.push_str(actor);
        let mut target_s = heapless::String::<128>::new();
        let _ = target_s.push_str(target);
        let mut detail_s = heapless::String::<256>::new();
        let _ = detail_s.push_str(detail);
        AuditEvent {
            seq: 0,
            timestamp_ms: 0,
            event_type,
            actor: actor_s,
            target: target_s,
            detail: detail_s,
            severity: 0,
            source_ip: [0; 4],
        }
    }
}

/// 链上条目：事件哈希 + 前链哈希 + 组合哈希的三元存证.
#[derive(Debug, Clone)]
pub struct ChainEntry {
    /// 条目序号（与事件序号一致，链内严格递增）.
    pub seq: u64,
    /// 前一条目的组合哈希（首条为创世哈希）.
    pub prev_hash: [u8; 32],
    /// 本条事件的 SM3 哈希.
    pub event_hash: [u8; 32],
    /// 组合哈希 `SM3(prev_hash ‖ event_hash)`，作为下一条的 `prev_hash`.
    pub combined_hash: [u8; 32],
    /// 事件时间戳（毫秒，冗余自事件便于按时间检索）.
    pub timestamp_ms: u64,
}

/// 哈希链头状态（内存中的链尾视图，可经存储恢复）.
#[derive(Debug, Clone)]
pub struct HashChain {
    /// 链尾组合哈希（下一条目的 `prev_hash`；空链时为创世哈希）.
    pub last_hash: [u8; 32],
    /// 链尾序号（空链为 0）.
    pub last_seq: u64,
    /// 已追加条目总数.
    pub entry_count: u64,
    /// 创世哈希（链起点锚，固定不变）.
    pub genesis_hash: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVT1：事件类型判别值覆盖 1~12 全值域且无重复.
    #[test]
    fn evt1_event_type_discriminants_cover_1_to_12() {
        let cases = [
            (AuditEventType::AuthLogin, 1u8),
            (AuditEventType::AuthLogout, 2),
            (AuditEventType::ConfigChange, 3),
            (AuditEventType::ControlCommand, 4),
            (AuditEventType::ModelLoad, 5),
            (AuditEventType::KeyUse, 6),
            (AuditEventType::AgentStart, 7),
            (AuditEventType::AgentStop, 8),
            (AuditEventType::FederateJoin, 9),
            (AuditEventType::FederateLeave, 10),
            (AuditEventType::SecurityAlert, 11),
            (AuditEventType::AiDecision, 12),
        ];
        for (ty, expect) in cases {
            assert_eq!(ty as u8, expect);
        }
    }

    /// EVT2：heapless::String\<64\> 写入 65 字节超容量返回 Err（边界行为）.
    #[test]
    fn evt2_heapless_string_over_capacity_returns_err() {
        let mut s = heapless::String::<64>::new();
        // 恰好写满 64 字节成功
        let full = "a".repeat(64);
        assert!(s.push_str(&full).is_ok());
        // 再写 1 字节（即第 65 字节）超容量返回 Err
        assert!(s.push_str("b").is_err());

        // 一次性写入 65 字节同样返回 Err
        let mut s2 = heapless::String::<64>::new();
        let over = "c".repeat(65);
        assert!(s2.push_str(&over).is_err());
    }
}
