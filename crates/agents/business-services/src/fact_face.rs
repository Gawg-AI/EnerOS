//! 蓝图 68 卷 68.4（WP-68.4.1）：svc-market 外部事实持久化面（SFS 解冻）。
//!
//! 链路与 control-services::event_face 同构（裁决见 68.4 执行卡）：EIPC
//! 块命令面 → [`CachedBlocks`] → [`RegionScope`]（DATA_FACT_* 区段钳制，
//! 类型门）→ `BlockEventStore`（event-store 单源）→
//! [`StoreFactEventSink`]（Q0 写透——ACK 即 durable，RPO=0；本面是
//! 全系统 RPO=0 的严格锚点：事实零流量 ⇒ 无停顿预算压力）。
//!
//! 诚实性纪律：打开失败显式降级（bin 落 FACT-FACE-UNAVAILABLE 词形）；
//! 上游事实 transport（market peer / sealed FactEnvelope 通道）今日未
//! 部署 ⇒ sink 已生产在位、零消费——跨 bin 接线（market_ingest /
//! external_facts）登记后续卷（68.4 执行卡 WP-68.4.1 改动面 3）。

use eneros_agent_sdk::block_io::{BlockIo, EipcBlockDevice, EipcSend};
use eneros_event_store::block::BlockEventStore;
use eneros_event_store::scoped::open_region_scoped;
use eneros_event_store::RecoveryReport;
use eneros_ota_ab::ab_partition::DataRegion;
use eneros_storage::partition::{CachedBlocks, RegionScope};

use crate::store_fact_sink::StoreFactEventSink;

/// 事实面设备栈（与事件面同栈形；缓存键 = 盘绝对块号）。
type FactDev<C> = RegionScope<CachedBlocks<4, EipcBlockDevice<C>>>;
/// 事实面 store 类型（bin 持有；Sink 泛型实参唯一出处）。
pub type FactStore<C> = BlockEventStore<FactDev<C>>;
/// 事实面 sink 类型（bin 持有字段的唯一出处）。
pub type FactSink<C> = StoreFactEventSink<FactStore<C>>;

/// 打开（或首次格式化后打开）事实面；返回 (sink, 恢复摘要)。
///
/// StoreConfig 与 OBS 事件面同参单源（[`eneros_control_services::
/// event_face::event_store_config`]——segment 16KiB / Q0 写透批量 1）。
/// open-or-format 语义在 `open_region_scoped`（open 保数据；仅
/// NotFormatted 显式格式化；ForeignFormat fail-closed）。
/// 错误面为 `()`：与 [`eneros_control_services::event_face`] 的打开面
/// 同理由（bin 侧只判成败、统一落显式 Unavailable 文案）；错误类型化
/// 属后续卷（clippy::result_unit_err 豁免同 open_disk_history 先例）。
#[allow(clippy::result_unit_err)]
pub fn open_fact_face<C: EipcSend + Clone + 'static>(
    io: BlockIo<C>,
    region: DataRegion,
) -> Result<(StoreFactEventSink<FactStore<C>>, RecoveryReport), ()> {
    let face = EipcBlockDevice::new(
        io,
        eneros_event_store::format::BLOCK_SIZE,
        eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS / 8,
    );
    let (store, report) = open_region_scoped(
        || CachedBlocks::<4, _>::new(face.clone()),
        region.base_block_512() / 8,
        region.blocks_512() / 8,
        eneros_control_services::event_face::event_store_config(),
    )
    .map_err(|_| ())?;
    Ok((StoreFactEventSink::new(store), report))
}

#[cfg(test)]
mod tests {
    use eneros_control_services::test_support::FakeStorage;

    use super::*;
    use crate::external_facts::FactEventSink;

    fn make_io(storage: &FakeStorage) -> BlockIo<FakeStorage> {
        BlockIo::new(storage.clone(), "svc-storage", 0, 5_000)
    }

    /// 事实面区段块数（生产常量单源）。
    const FACT_BLOCKS_512: u64 = eneros_ota_ab::ab_partition::DATA_FACT_BLOCKS;

    /// t1：RPO=0 行为锁——N 条事实写入 → 模拟掉电（弃 sink/驱动、介质
    /// 留存）→ 重开 → 重放对账零丢失零乱序；修订水位重建拦旧修订。
    #[test]
    fn t1_fact_face_rpo0_power_cut() {
        // 全盘仿真：生产基址（DATA_FACT_BASE_BLOCK）原样参与平移。
        let storage =
            FakeStorage::new((eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS * 512) as usize);
        // 首开（format-on-first）+ 三条事实（同 external_id 修订链 1..3
        // + 另一 id）。
        {
            let (mut sink, report) =
                open_fact_face(make_io(&storage), DataRegion::FactFace).expect("首开=format");
            assert_eq!(report.valid_records, 0);
            sink.append(1, &[7u8; 16], 1, b"canonical-a1", 2_000)
                .expect("fact a r1");
            sink.append(1, &[7u8; 16], 2, b"canonical-a2", 3_000)
                .expect("fact a r2");
            sink.append(2, &[9u8; 16], 1, b"canonical-b1", 4_000)
                .expect("fact b r1");
        }
        // —— 掉电：全部实例丢弃，介质留存 ——
        let (mut sink2, report2) =
            open_fact_face(make_io(&storage), DataRegion::FactFace).expect("重开=mount");
        assert_eq!(report2.valid_records, 3, "RPO=0：已 ACK 事实全恢复");
        let facts = sink2.collect_facts().expect("drain");
        assert_eq!(facts.len(), 3, "零丢失");
        // 分区内 offset 序 = 写入序（同源保序）。
        assert_eq!(facts[0].1, [7u8; 16]);
        assert_eq!(facts[0].2, 1);
        assert_eq!(facts[0].3, b"canonical-a1".to_vec());
        assert_eq!(facts[1].2, 2, "同源修订按序");
        assert_eq!(facts[2].1, [9u8; 16]);
        // 幂等：再 drain 零重复（位点双槽持久）。
        assert!(sink2.collect_facts().expect("drain2").is_empty());
        // 修订水位重建：旧修订拒、新修订过。
        sink2.recover().expect("wm rebuild");
        assert_eq!(
            sink2.append(1, &[7u8; 16], 2, b"stale", 5_000),
            Err("stale revision"),
            "重启后旧修订必须被水位拦下"
        );
        sink2
            .append(1, &[7u8; 16], 3, b"canonical-a3", 6_000)
            .expect("新修订通过");
    }

    /// t3：越区段零逃逸——全盘仿真（16 MiB，其余区段/保护带都在盘上），
    /// 全部块命令字节偏移必须落在事实面让渡区段内（生产基址平移下界被
    /// 越即红）；变异（去 RegionScope 钳制）必红。
    #[test]
    fn t3_fact_face_region_scoped_no_escape() {
        let storage =
            FakeStorage::new((eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS * 512) as usize);
        let (mut sink, _) = open_fact_face(make_io(&storage), DataRegion::FactFace).expect("open");
        for r in 1..10u32 {
            sink.append(1, &[5u8; 16], r, b"payload", 1_000 + r as u64)
                .expect("append");
        }
        let region_lo = (DataRegion::FactFace.base_block_512() * 512) as usize;
        let region_hi = region_lo + (FACT_BLOCKS_512 * 512) as usize;
        let cmds = storage.cmds();
        assert!(!cmds.is_empty());
        for (_cmd, off, len) in cmds.iter() {
            let (off, end) = (*off as usize, *off as usize + *len);
            assert!(
                off >= region_lo && end <= region_hi,
                "块命令越区段：off={off} len={len}（区段 [{region_lo},{region_hi})）"
            );
        }
    }

    /// t2（bin 装配存在性）：svc-market bin 必须装配事实面（E-DEF-01
    /// 教训的测试化——"改回 None"回归在此红）。
    #[test]
    fn t2_market_bin_wiring_present() {
        let bin = include_str!("bin/svc-market.rs");
        assert!(
            bin.contains("open_fact_face"),
            "svc-market 未装配事实面（68.4 SFS 回退回归）"
        );
        assert!(
            bin.contains("DATA_FACT_BASE_BLOCK"),
            "svc-market 未以 ota-ab 区段常量装配（区段表单源破坏）"
        );
        assert!(
            bin.contains("FACT-FACE-OPEN"),
            "svc-market 缺 FACT-FACE-OPEN 观测词形"
        );
    }
}
