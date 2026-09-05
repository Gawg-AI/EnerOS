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
    // SPEC-C（WP-C.2）：history-query 执行器经 EIPC 向 svc-telemetry 取
    // 历史页（TELR 帧；分块通道承载）。
    ("svc-mgmt", "svc-telemetry"),
    // 46（WP-46.3 配置驱动装配）：协议服务经 svc-config 取协议配置
    // （GET_ENTRY "protocol-<proto>"）——消灭源码 peer 硬编码。
    ("svc-iec104", "svc-config"),
    // 68.2（WP-68.2.3）：iec104 事件上行边接通（62.3 §H 在册项转正）。
    // 注释归真（68 卷事实基线 F13，2026-09-05）：63.2 试修即回退时判为
    // "iec104 适配器拿本机 now_ms 冒充 epoch"——复核失准：iec104 adapter
    // 无任何 epoch 代码，modbus/iec104 epoch 都是 open_source 正常分配
    // （investigation 实测 10666/23384）。真根因 = 每服务 open() 的
    // floor 各自取 boot 单调时刻（必不相等，ingestion_stage.rs `epoch_
    // floor = now_ms()`）+ 消费侧评估器单一全局 epoch（最后钉扎者赢）
    // → iec104 后开即 modbus（唯一控制合格源）样本全拒
    // SourceEpochMismatch，AI 链 agent=OK 59→0（evidence/63/vol63-2/
    // investigation-grant-edge.qemu-serial.log:5454,13765,13819）。
    // 68.2 消费侧 per-source 归位（trusted.rs SourceEpochTable + 钉扎门
    // restore 同步删除，t1/t2/t4 行为锁）修根因后"激活即回归"前提消灭；
    // 本边**追加于 iec104 config 边之后**（槽位冻结：
    // s1_protocol_clients_config_slots_frozen 守序）。
    ("svc-iec104", "svc-telemetry"),
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
    // SPEC-E（WP-E.3 有界学习闭环）：学习参数快照经 svc-storage 真落盘
    // （LEARN_STATE 区段，ota-ab LEARN_STATE_* 常量；存储不可得 ⇒ 学习
    // fail-closed 不可用，行为出厂值）。
    ("svc-ai-worker", "svc-storage"),
    // 47（WP-47.2 点表绑定）：控制写目标经 svc-config 点表解析——两端同源。
    ("svc-control-dispatch", "svc-config"),
    ("svc-device-agent", "svc-config"),
    // 47（WP-47.4 意图持久化）：DurableIntentStore 经 svc-storage 真落盘。
    ("svc-control-dispatch", "svc-storage"),
    // SPEC-C（D-TSDB-2 方案 A 句柄让渡）：svc-telemetry 经块命令面持有
    // 数据盘 TSDB 区段（ota-ab DATA_TSDB_* 常量；审计表 wp-c-memory-audit
    // §三）。svc-telemetry 首条客户端授予 → 槽 EIPC_EP_BASE_SLOT+0、bit 0。
    ("svc-telemetry", "svc-storage"),
    // 61 v0.314.27（观测事件影子路径）：svc-telemetry 经 svc-config 取
    // points 配置（来源注册表/映射世代——影子评估器与授权来源单源）。
    // 追加于本客户端既有边之后（WP-54.4 槽位冻结：既有边序不受影响）。
    ("svc-telemetry", "svc-config"),
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

/// 服务侧"按目标名判定授予在位并取槽"的便捷单源（SPEC-SERIES-2026-08-28/B
/// WP-B.1）：解析 + X3 掩码位判定一步完成；未授予/无此边 → `None`
/// （fail-closed）。收编历史病灶——各 bin 手写
/// `resolve + (mask & (1 << bit) != 0)` 闭包（2026-08-28 体检实测 6 bin
/// 7 处 inline，含 svc-mgmt 硬编码掩码位 `& 2` 的魔数内联）。
pub fn eipc_granted_slot(client: &str, eipc_mask: u64, target: &'static str) -> Option<u64> {
    let (bit, slot) = eipc_resolve(client, target)?;
    if eipc_mask & (1 << bit) != 0 {
        Some(slot)
    } else {
        None
    }
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

    /// SPEC-B：eipc_granted_slot 掩码判定单源——授予在位取槽、缺位 None、
    /// 无此边 None；与手工 `resolve + mask` 逐位等价（收编对象是六 bin
    /// 手写闭包，此处锁定其语义）。
    #[test]
    fn wp_b1_granted_slot_mask_judgment() {
        // 授予在位：掩码含该位 → 槽。
        let (bit, slot) = eipc_resolve("svc-control-dispatch", "svc-storage").unwrap();
        assert_eq!(
            eipc_granted_slot("svc-control-dispatch", 1 << bit, "svc-storage"),
            Some(slot)
        );
        // 掩码缺位 → None（未授予）。
        assert_eq!(
            eipc_granted_slot("svc-control-dispatch", 0, "svc-storage"),
            None
        );
        assert_eq!(
            eipc_granted_slot("svc-control-dispatch", 1 << bit, "svc-storage").expect("在位"),
            slot
        );
        // 无此边 → None。
        assert_eq!(
            eipc_granted_slot("svc-control-dispatch", u64::MAX, "svc-nosuch"),
            None
        );
        // 收编等价性：对冻结抽样边，闭包语义与单源逐位一致。
        for &(client, target) in EIPC_GRANTS {
            for mask in [0u64, 1 << 0, 1 << 5, u64::MAX] {
                let manual = eipc_resolve(client, target)
                    .and_then(|(b, s)| (mask & (1 << b) != 0).then_some(s));
                assert_eq!(
                    eipc_granted_slot(client, mask, target),
                    manual,
                    "{client}→{target} mask={mask:#x} 闭包等价"
                );
            }
        }
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
        // SPEC-C 新增两边的槽位冻结（其余客户端编号不受影响——每客户端
        // 独立计数）：mgmt 第 2 边、telemetry 首边。
        assert_eq!(eipc_resolve("svc-mgmt", "svc-config"), Some((0, 3)));
        assert_eq!(eipc_resolve("svc-mgmt", "svc-telemetry"), Some((1, 4)));
        assert_eq!(eipc_resolve("svc-telemetry", "svc-storage"), Some((0, 3)));
        // 61 v0.314.27：telemetry 第 2 边 = svc-config（影子路径配置取数；
        // 追加序——既有边槽位不变）。
        assert_eq!(eipc_resolve("svc-telemetry", "svc-config"), Some((1, 4)));
    }

    /// S1 WP-S1.1 C6（F-9/F-11）：七个协议客户端今日 `svc-config` 槽位冻结
    /// ——S1 起 bin 改按名取槽，本测试锁「表序迁移前后等价」：goose/sv/
    /// mms/iec104/modbus-rtu 首边 → 槽 base+0（原手数 +0）；modbus-tcp
    /// 第 2 边 → 槽 base+1（原手数 +1）；mqtt 首边 → base+0。`EIPC_GRANTS`
    /// 任何插边在测试侧先红（C6 反证：手插 ("svc-mqtt","svc-fake") 于
    /// mqtt config 边前即红，见 evidence）。
    #[test]
    fn s1_protocol_clients_config_slots_frozen() {
        assert_eq!(
            eipc_resolve("svc-goose", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        assert_eq!(
            eipc_resolve("svc-sv", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        assert_eq!(
            eipc_resolve("svc-mms", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        assert_eq!(
            eipc_resolve("svc-iec104", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        assert_eq!(
            eipc_resolve("svc-modbus-rtu", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        // modbus-tcp：telemetry 先行（45.5），config 是第 2 条边。
        assert_eq!(
            eipc_resolve("svc-modbus-tcp", "svc-telemetry"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
        assert_eq!(
            eipc_resolve("svc-modbus-tcp", "svc-config"),
            Some((1, EIPC_EP_BASE_SLOT + 1))
        );
        assert_eq!(
            eipc_resolve("svc-mqtt", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT))
        );
    }

    /// 68.2 WP-68.2.3 授予边接通锁（翻转 63.2 的 wp6323 锁）：svc-iec104→
    /// svc-telemetry 边在位且钉扎。历史语义（保留防反复）：62.3 §H
    /// "登记不修"的前提 = 消费侧单全局 epoch 根因未修，激活即回归（AI 链
    /// agent=OK 59→0，investigation 实证）；68 卷 F13 根因重定位 + 68.2
    /// 消费侧 per-source 归位（SourceEpochTable，t1/t2/t4 行为锁）后前提
    /// 消灭，本边转正。追加序（config 边后）由 s1 冻结锁守。
    #[test]
    fn wp682_iec104_telemetry_edge_granted_and_pinned() {
        assert_eq!(
            eipc_resolve("svc-iec104", "svc-config"),
            Some((0, EIPC_EP_BASE_SLOT)),
            "既有 config 边槽位冻结（新边不得挤位）"
        );
        let (bit, slot) = eipc_resolve("svc-iec104", "svc-telemetry").expect("68.2 授予边在位");
        assert_eq!(bit, 1, "iec104 第 2 条边 = telemetry（追加于 config 后）");
        assert_eq!(slot, EIPC_EP_BASE_SLOT + 1);
        let (mask, grants) = eipc_grants("svc-iec104");
        assert_eq!(mask, 0b11, "iec104 恰两条授予边");
        assert_eq!(
            grants,
            alloc::vec![
                (EIPC_EP_BASE_SLOT, "svc-config"),
                (EIPC_EP_BASE_SLOT + 1, "svc-telemetry"),
            ],
        );
        // 掩码判定便捷面同源一致。
        assert_eq!(
            eipc_granted_slot("svc-iec104", mask, "svc-telemetry"),
            Some(slot)
        );
    }
}
