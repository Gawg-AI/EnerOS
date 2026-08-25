//! EIPC 端点授予拓扑——单一事实源（蓝图 54 WP-54.4）.
//!
//! 历史病灶：授予表长在 root-task `spawn.rs`，服务 bin 手数槽位
//! （`EIPC_EP_BASE_SLOT + n`）靠注释对齐——表中间插一条边，后续槽位静默
//! 移位，mint 侧与消费侧错位。本模块把表与解析算法收编一处：
//! root-task spawn 期 mint 与服务侧取槽**共享同一张表、同一个纯函数**。
//!
//! 规则（不变量）：客户端按**表内出现序**获得其第 i 条授予 → 服务 CSpace
//! 槽 [`EIPC_EP_BASE_SLOT`] + i，X3 掩码 bit i。槽位数值与位分配一个都不变
//! （D2）；本卷收口的是编码知识的归属，不是编码本身。

use alloc::vec::Vec;

use crate::EIPC_EP_BASE_SLOT;

/// EIPC 端点授予表：(客户端服务, 目标服务)。自 root-task spawn.rs 逐字
/// 迁移（WP-54.4），行内注释保留历史登记。
pub const EIPC_GRANTS: &[(&str, &str)] = &[
    ("svc-control-dispatch", "svc-device-agent"),
    ("svc-energy-agent", "svc-telemetry"),
    ("svc-energy-agent", "svc-ai-worker"),
    ("svc-energy-agent", "svc-control-dispatch"),
    // 45.5（遥测通路接线，先行者）：svc-modbus-tcp 采集链向 svc-telemetry
    // 上报真实点样本；经 svc-config 取点表（ConfigGet "points" →
    // decode_points）。
    ("svc-modbus-tcp", "svc-telemetry"),
    ("svc-modbus-tcp", "svc-config"),
    // 41（WP-41.3 落盘）：业务服务 durable 状态经 EIPC 调 svc-storage 真落盘
    // （块命令面 OP_EXECUTE + BLK_CMD_*）。
    ("svc-ledger", "svc-storage"),
    ("svc-market", "svc-storage"),
    ("svc-ota", "svc-storage"),
    ("svc-observability", "svc-storage"),
    // 50（WP-50.1 exporter 真通道）：svc-observability 遥测批量帧经 EIPC
    // 分片 EXECUTE 送达 svc-mqtt。槽序：其表内第 2 条授予 = 槽 base+1（bit 1）。
    ("svc-observability", "svc-mqtt"),
    ("svc-cloud-sync", "svc-storage"),
    ("svc-agent-bus", "svc-storage"),
    // 41（WP-41.11）：管理 RPC 的 ConfigGet 经 EIPC 查 svc-config。
    ("svc-mgmt", "svc-config"),
    // 46（WP-46.3 配置驱动装配）：协议服务经 svc-config 取协议配置
    // （GET_ENTRY "protocol-<proto>"）——消灭源码 peer 硬编码。
    ("svc-iec104", "svc-config"),
    ("svc-mqtt", "svc-config"),
    ("svc-mms", "svc-config"),
    ("svc-goose", "svc-config"),
    ("svc-sv", "svc-config"),
    ("svc-modbus-rtu", "svc-config"),
    // 47（WP-47.1 协议写链）：dispatch 出站经协议主站写设备
    // （OP_MASTER_WRITE → svc-modbus-tcp）+ 入站意图授权前经 svc-telemetry
    // 取受信任遥测快照（TEL_SNAPSHOT）。
    ("svc-control-dispatch", "svc-modbus-tcp"),
    ("svc-control-dispatch", "svc-telemetry"),
    // 47（WP-47.1 协议入站写）：svc-mqtt 下行写意图上报 dispatch
    // （DISP_SUBMIT_INTENT，12 态链授权）。
    ("svc-mqtt", "svc-control-dispatch"),
    // 49（WP-49.2 远程 dual-brain）：mqtt↔ai-worker 分片推送；ai-worker 经
    // svc-config 取 ai-peer 状态机参数 + 经 svc-mqtt 发布 AI 请求分片。
    ("svc-mqtt", "svc-ai-worker"),
    // 50（WP-50.2 mgmt 通道）：管理请求 MGT1 帧推送 svc-mgmt。
    ("svc-mqtt", "svc-mgmt"),
    ("svc-ai-worker", "svc-config"),
    ("svc-ai-worker", "svc-mqtt"),
    // 47（WP-47.2 点表绑定）：控制写目标经 svc-config 点表解析——两端同源。
    ("svc-control-dispatch", "svc-config"),
    ("svc-device-agent", "svc-config"),
    // 47（WP-47.4 意图持久化）：DurableIntentStore 经 svc-storage 真落盘。
    ("svc-control-dispatch", "svc-storage"),
];

/// 在给定表上按名解析：返回 `(X3 掩码位, CSpace 槽位)`；无此授予边 →
/// `None`（fail-closed）。表参数化供插边演练测试复用（生产入口为
/// [`eipc_resolve`]，恒读权威表 [`EIPC_GRANTS`]）。
fn resolve_in(table: &[(&str, &str)], client: &str, target: &str) -> Option<(u32, u64)> {
    let mut idx = 0u64;
    for (c, t) in table {
        if *c == client {
            if *t == target {
                return Some((idx as u32, EIPC_EP_BASE_SLOT + idx));
            }
            idx += 1;
        }
    }
    None
}

/// 服务侧取槽唯一入口：本服务对目标服务的 `(X3 掩码位, CSpace 槽位)`。
/// 调用方以 `mask & (1 << bit)` 判定授予在位，再以 `slot` 开通道；
/// 未授予/无此边一律 `None` → fail-closed（诚实降级，不伪造可达）。
pub fn eipc_resolve(client: &str, target: &str) -> Option<(u32, u64)> {
    resolve_in(EIPC_GRANTS, client, target)
}

/// 客户端全部授予（root-task mint 侧推导）：`(X3 掩码, [(槽位, 目标名)])`
/// ——与 [`eipc_resolve`] 共享同一张表与同一编号规则（原 root-task
/// `eipc_grants_for` 语义，WP-54.4 起委托至此）。
pub fn eipc_grants(client: &str) -> (u64, Vec<(u64, &'static str)>) {
    grants_in(EIPC_GRANTS, client)
}

/// [`eipc_grants`] 的表参数化内核（插边演练用：mint/consumer 同读一张副本）。
fn grants_in<'a>(table: &[(&'a str, &'a str)], client: &str) -> (u64, Vec<(u64, &'a str)>) {
    let mut mask = 0u64;
    let mut grants = Vec::new();
    for (c, t) in table {
        if *c == client {
            let slot = EIPC_EP_BASE_SLOT + grants.len() as u64;
            mask |= 1 << grants.len();
            grants.push((slot, *t));
        }
    }
    (mask, grants)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WP-54.4 存在意义直证（DoD#2 插边演练）：在表副本中间插入一条假边后，
    /// **mint 侧（eipc_grants）与消费侧（resolve_in）对每条真实边的槽位
    /// 指认仍然一致**——旧世界（消费侧手数常量）里这种插入会让两侧错位，
    /// 新世界里两侧同源同移，配对永不漂移。
    #[test]
    fn wp544_edge_insertion_keeps_mint_and_consumer_in_agreement() {
        // 基线：权威表上 mint ≡ consumer。
        for &(client, target) in EIPC_GRANTS {
            let (bit, slot) = eipc_resolve(client, target).expect("表内必有此边");
            let (mask, grants) = eipc_grants(client);
            assert_ne!(mask & (1 << bit), 0, "{client}→{target} 位应在掩码内");
            assert!(
                grants.iter().any(|(s, t)| *s == slot && *t == target),
                "{client}→{target} 槽 {slot} 应在 mint 列表内"
            );
        }

        // 演练：给多授予客户端 svc-control-dispatch 的授予序列中间插假边。
        let mut copy: alloc::vec::Vec<(&str, &str)> = EIPC_GRANTS.to_vec();
        let insert_at = copy
            .iter()
            .position(|(c, _)| *c == "svc-control-dispatch")
            .expect("dispatch 至少一条边")
            + 1;
        copy.insert(insert_at, ("svc-control-dispatch", "svc-fake-target"));

        // 插边后：该客户端后续真实边槽位整体 +1——但 mint/consumer 两侧
        // 同步移位，逐边配对依旧一致；假边自身获得独立槽位不侵占他边。
        let (fake_bit, fake_slot) =
            resolve_in(&copy, "svc-control-dispatch", "svc-fake-target").expect("假边可解析");
        assert_eq!(fake_bit, 1, "假边插在其第 2 位（bit 1）");
        assert_eq!(fake_slot, EIPC_EP_BASE_SLOT + 1);
        // 插边后的 mint 侧（同读副本）：
        let (_, minted) = grants_in(&copy, "svc-control-dispatch");
        for (client, target) in copy.iter().filter(|(c, _)| *c == "svc-control-dispatch") {
            let (bit, slot) = resolve_in(&copy, client, target).expect("副本表内必有此边");
            assert_ne!(
                minted.iter().filter(|(s, _)| *s == slot).count(),
                0,
                "{target}: 消费侧槽 {slot}（bit {bit}）应与 mint 侧一致"
            );
            assert!(
                minted.iter().any(|(_, t)| *t == *target),
                "{target}: 目标名配对保持"
            );
        }

        // 他客户端完全不受该假边影响（按客户端各自表序编号）。
        for (client, target) in EIPC_GRANTS
            .iter()
            .filter(|(c, _)| *c != "svc-control-dispatch")
        {
            assert_eq!(
                resolve_in(&copy, client, target),
                resolve_in(EIPC_GRANTS, client, target),
                "{client}→{target} 不受他客户端插边影响"
            );
        }
    }

    /// fail-closed：不存在的授予边返回 None（不伪造可达）。
    #[test]
    fn wp544_unknown_pair_is_none() {
        assert_eq!(eipc_resolve("svc-nosuch", "svc-storage"), None);
        assert_eq!(
            eipc_resolve("svc-market", "svc-mqtt"),
            None,
            "未授予的边不可解析"
        );
    }

    /// D2 冻结抽样：关键边槽位数值一个都不变（53 卷 QEMU 基线的装配事实）。
    #[test]
    fn wp544_slot_values_frozen() {
        assert_eq!(
            eipc_resolve("svc-control-dispatch", "svc-device-agent"),
            Some((0, 3))
        );
        assert_eq!(
            eipc_resolve("svc-control-dispatch", "svc-modbus-tcp"),
            Some((1, 4))
        );
        assert_eq!(
            eipc_resolve("svc-control-dispatch", "svc-telemetry"),
            Some((2, 5))
        );
        assert_eq!(
            eipc_resolve("svc-control-dispatch", "svc-config"),
            Some((3, 6))
        );
        assert_eq!(
            eipc_resolve("svc-control-dispatch", "svc-storage"),
            Some((4, 7))
        );
        assert_eq!(
            eipc_resolve("svc-energy-agent", "svc-telemetry"),
            Some((0, 3))
        );
        assert_eq!(eipc_resolve("svc-ai-worker", "svc-config"), Some((0, 3)));
        assert_eq!(eipc_resolve("svc-ai-worker", "svc-mqtt"), Some((1, 4)));
        assert_eq!(eipc_resolve("svc-observability", "svc-mqtt"), Some((1, 4)));
        assert_eq!(
            eipc_resolve("svc-mqtt", "svc-control-dispatch"),
            Some((1, 4))
        );
    }
}
