//! SPEC-C（WP-C.1 / D-TSDB-2 方案 A）：svc-telemetry 磁盘历史适配。
//!
//! 架构（决策备忘 docs/delivery/wp2-memory-audit-d-tsdb2.md §二 + 审计表
//! wp-c-memory-audit-spec-c-20260827.md）：EIPC 授予边取得 svc-storage 块
//! 命令面整盘视图 → [`RegionScope`] 平移钳制到 TSDB 区段（ota-ab
//! `DATA_TSDB_BASE_BLOCK/_BLOCKS`）→ Lfs mount-or-format →
//! [`TimeSeriesDB::open`] **by-value 归本服务单一所有权**（禁跨界引用）。
//!
//! 热缓存/慢库两层关系（防漂移定义处）：
//! - [`crate::telemetry::HistoryBuffer`]：进程内热查询层，快、易失；
//! - 本模块 [`TsdbHistorySink`]：慢库持久层，批次边界落盘；
//! - [`HistoryChain`] 把两者组成单 sink：热层永远先写（采集线不因磁盘
//!   故障停摆），磁盘结果**如实上抛**——服务侧据此打印 COMMIT/DROPPED，
//!   不许伪成功（R1/R9）。
//!
//! target 装配失败一律显式降级回内存环并串口登记（不伪造持久化），与
//! svc-control-dispatch DurableIntentStore 兜底纪律同构。

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use eneros_agent_sdk::block_io::{BlockIo, EipcBlockDevice, EipcSend};
use eneros_agent_sdk::history_wire::{HistPointWire, HistoryPageSource, PAGE_RECORDS};
use eneros_fs::{FsError, Lfs};
use eneros_storage::driver::BlockDevice;
use eneros_storage::partition::RegionScope;
use eneros_tsdb::schema::{DataQuality, DeviceId, MetricId, TimeSeriesPoint, TsdbConfig};
use eneros_tsdb::TimeSeriesDB;

use crate::telemetry::{HistoryBuffer, HistoryRecord, TelemetryHistorySink};

/// 目标侧 chunk 切换点数（审计表 §一#4：内存活动 chunk ≈4 KiB 有界）。
/// 上调 MUST 先重出内存审计表。
pub const TARGET_TSDB_CHUNK_POINTS: u32 = 256;

/// checkpoint 合并窗（批数；68.3 WP-68.3.3 TCG 实测订正，2026-09-06）。
///
/// vol68-3 run3 实弹：TCG/QEMU（virtio-blk + O_DIRECT RMW）单块命令
/// ~5-15ms，逐批 checkpoint（~1.3k 命令/次，实测单次 7.7-32s）在 OBS/
/// TEL_INGEST 处理内同步执行会饿死决策链（agent=OK 0、17 轮/600s、
/// CONTROL-CHAIN 无 Committed）。合并为每 8 批一次后：ACK=历史层已
/// 受理（write_batch 进 TSDB 缓冲/活动 chunk），磁盘持久点=最近一次
/// checkpoint，**掉电丢尾 ≤8 批**（≈6min@实测 OBS 批距 ~46s；ADR-0047
/// MemEpochStore 非持久同款诚实登记——对照 68.3 前=内存环掉电全丢）。
/// 8 的取法：标准 600s 窗实测批数 ~13-24（run4=13），N=128 时全窗零
/// checkpoint（run4 实弹 TSDB-CHECKPOINT-DUR 缺词形、重启窗无旧数据
/// 可读）；N=8 保证每窗 ≥1 次落盘且 duty ~2-6%。异步化/命令降本
/// （扇区对齐批量写、index 增量日志）归后续卷（登记册
/// REG-TSDB-CHECKPOINT-ASYNC）。
pub const CHECKPOINT_EVERY_BATCHES: u64 = 8;

/// LFS 几何：4096B 块 × 区段折算段数（16 MiB 盘 8 MiB 区段 → 2048 段）。
const FS_BLOCK_SIZE: usize = 4096;

/// TSDB 数据目录（区分于未来其他卷的目录规划）。
pub const DISK_DATA_DIR: &str = "/tsdb";

// ============================================================================
// 块面适配：agent-sdk BlockIo（PersistIo 字节面）→ storage BlockDevice
//
// 全仓单件 = agent-sdk `block_io::EipcBlockDevice`（SPEC-B/C 收编：本模块
// 曾经的 `BlockIoFace` 同构件已删，R7 候选执行；erase 零写契约测试在
// agent-sdk 侧唯一）。TSDB 区段以 4096B 计块（FS_BLOCK_SIZE 折算）。
// ============================================================================

// ============================================================================
// 磁盘内核共享（单线程服务循环内 Rc<RefCell> 是诚实所有局面，非并发借口）
// ============================================================================

type DiskDb = Rc<RefCell<TimeSeriesDB>>;

fn map_record(r: &HistoryRecord) -> TimeSeriesPoint {
    // 品质映射：HistoryRecord.quality 四档码（单一来源 sample_wire 的
    // quality_code）↔ tsdb 四档 DataQuality（1→Uncertain；2→Bad；
    // 3→Unknown 如实承载——SPEC-A：不再折叠为 Bad，往返不丢失）。
    TimeSeriesPoint {
        timestamp: r.timestamp_ms,
        device_id: DeviceId(r.device_id),
        metric: MetricId(r.point),
        value: r.value,
        quality: DataQuality::from(r.quality),
    }
}

/// 慢库持久层 sink（实现 [`TelemetryHistorySink`] 即插接缝）。
pub struct TsdbHistorySink {
    db: DiskDb,
    /// 磁盘写失败批次数（观测）。
    pub disk_errors: u64,
    /// 距上次 checkpoint 的批次数（合并窗计数；见
    /// [`CHECKPOINT_EVERY_BATCHES`]）。
    batches_since_checkpoint: u64,
}

impl TelemetryHistorySink for TsdbHistorySink {
    /// 批次边界"后方 ACK"：写入 TSDB 缓冲/活动 chunk 即受理（ACK 语义，
    /// 磁盘持久点 = 最近一次 checkpoint——合并窗见
    /// [`CHECKPOINT_EVERY_BATCHES`]）。失败如实上抛（DROPPED 行）不伪
    /// 成功；窗满批 checkpoint 的结果**不得掩盖本批写失败**（68.3 复查
    /// 回归锁 c6）。
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()> {
        let pts: Vec<TimeSeriesPoint> = batch.iter().map(map_record).collect();
        let mut db = self.db.borrow_mut();
        let r = db.write_batch(&pts);
        self.batches_since_checkpoint += 1;
        // checkpoint 合并窗（见 CHECKPOINT_EVERY_BATCHES 文档）：仅窗满
        // 批次做磁盘 checkpoint。`r.and(ck)`：两步都成功才成功；本批写
        // 失败优先于 checkpoint 结果上抛（68.3 前行为 = and_then 短路
        // 同语义——窗满不得把丢批谎报成 COMMIT）。ck 失败也清零计数：
        // 保留计数会让后续每批都重试 ~30s 级 checkpoint（TCG 实测），复
        // 现 68.3 D.4 饿死形态——清零后 8 批再试，ck 持续失败期掉电丢尾
        // 上界 16 批（词形/Err 显式暴露，非静默）。
        let r = if self.batches_since_checkpoint >= CHECKPOINT_EVERY_BATCHES {
            self.batches_since_checkpoint = 0;
            let t0 = eneros_drv_svc::now_ms();
            let ck = db.checkpoint();
            let dur = eneros_drv_svc::now_ms().saturating_sub(t0);
            // 蓝图 68 卷 68.3（WP-68.3.3）：checkpoint 时长判定行（host
            // 时钟恒 0；target 为 Arm64Timer 单调毫秒）。TCG 实测量级
            // ~秒级/次（执行卡 §D 落卡）。
            eneros_runtime::println!(
                "[svc-telemetry] TSDB-CHECKPOINT-DUR ms={} items={}",
                dur,
                batch.len()
            );
            r.and(ck)
        } else {
            r
        };
        if r.is_err() {
            self.disk_errors += 1;
            return Err(());
        }
        Ok(())
    }
}

/// 慢库取数面（TELH 服务端经此出页）。
pub struct TsdbHistoryReader {
    db: DiskDb,
}

/// 页查询结果：本页行 + 该点位全量行数 + 钳制窗口长度 + 数据存在性。
pub struct PageResult {
    pub rows: Vec<HistPointWire>,
    /// 该点位**精确全量**行数（count API，与钳制窗无关）。
    pub total: u32,
    /// 钳制窗口长度 = `min(total, last_n)` 的实际行集大小；TELR `more`
    /// 游标按此推导（见 history_wire::more_for_page），不按 total——
    /// 避免钳制窗外空页往返（SPEC-A 卷）。
    pub window_len: u32,
}

impl TsdbHistoryReader {
    /// TSDB 索引条目数（重启后 index.bin 重建结果——TSDB-DISK-OPEN
    /// 行的持久性观测口，蓝图 68 卷 68.3 WP-68.3.4：旧数据保持的
    /// 串口证据即此值 > 0）。
    pub fn index_len(&self) -> usize {
        self.db.borrow().index_len()
    }

    /// 该点位在索引中是否有任何数据来源（区分"未知点位"与"空页"）。
    pub fn point_exists(&self, point: u32) -> bool {
        !self
            .db
            .borrow()
            .pairs_for_metric(MetricId(point))
            .is_empty()
    }

    /// 取该点位最近 `last_n` 条的第 `page` 页（升序；页大小 =
    /// agent_sdk::history_wire::PAGE_RECORDS）。多设备同点位聚合后全局
    /// 升序稳定排序（同时刻点按设备遍历序确定性输出）；超出总量给空行区
    /// （total 仍如实），未知点位返回 None 由服务端转 UNKNOWN_POINT 错误帧。
    ///
    /// 内存界（审计表 §一#8）：每设备至多 last_n 行（k 个设备 ×N，
    /// k=含该点位的设备数，量级个位）。
    pub fn query_page(&self, point: u32, last_n: usize, page: u32) -> Option<PageResult> {
        let mut all: Vec<HistPointWire> = Vec::new();
        {
            let mut db = self.db.borrow_mut();
            for (dev, metric) in db.pairs_for_metric(MetricId(point)) {
                let pts = db.query_last_n(dev, metric, last_n).ok()?;
                for p in pts {
                    all.push(HistPointWire {
                        timestamp_ms: p.timestamp,
                        device_id: p.device_id.0,
                        point: p.metric.0,
                        value: p.value,
                        quality: p.quality.as_u8(),
                    });
                }
            }
        }
        if all.is_empty() {
            return None;
        }
        // 多设备聚合下按时间戳排序（稳定序保证跨调用确定性）。
        all.sort_by_key(|r| r.timestamp_ms);
        if all.len() > last_n {
            let drop = all.len() - last_n;
            all.drain(..drop);
        }
        // total 与页窗口/last_n 无关——精确全量计数（count API O(索引)，
        // 不物化行），分页游标据此计算 more。
        let total = self.db.borrow().count_points_for_metric(MetricId(point)) as u32;
        let window_len = all.len() as u32;
        let start = page as usize * PAGE_RECORDS;
        let end = core::cmp::min(start + PAGE_RECORDS, all.len());
        let rows = if start < all.len() {
            all[start..end].to_vec()
        } else {
            Vec::new()
        };
        Some(PageResult {
            rows,
            total,
            window_len,
        })
    }
}

impl HistoryPageSource for TsdbHistoryReader {
    /// 以 TELR 单源策略的取数契约出页（窗口长度如实报告，供
    /// history_wire::more_for_page 推导游标）。
    fn query_page(
        &self,
        point: u32,
        last_n: usize,
        page: u32,
    ) -> Option<(Vec<HistPointWire>, u32, u32)> {
        let pr = TsdbHistoryReader::query_page(self, point, last_n, page)?;
        Some((pr.rows, pr.total, pr.window_len))
    }
}

/// 热环 + 慢库链式 sink：写热层永远先行；磁盘结果如实上抛由服务侧落判定行。
pub struct HistoryChain {
    disk: TsdbHistorySink,
    hot: HistoryBuffer,
}

impl TelemetryHistorySink for HistoryChain {
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()> {
        self.hot.write_batch(batch)?;
        self.disk.write_batch(batch)
    }
}

impl HistoryChain {
    /// 热层 DropOldest 计数（观测透传，不静默）。
    pub fn hot_dropped(&self) -> u64 {
        self.hot.dropped
    }

    /// 磁盘侧累计写失败批次数（观测透传）。
    pub fn disk_errors(&self) -> u64 {
        self.disk.disk_errors
    }
}

/// 当前 LFS 卷可用的最大块数——`BlockDeviceStorage` 编译期上限
/// （BLOCK_COUNT=64 → 单卷 256 KiB）。区段声明（DATA_TSDB_*，8 MiB）
/// 是未来扩容预留；本卷诚实只挂卷首窗口，登记审计表 §三。
const FS_VOLUME_MAX_BLOCKS_4K: u64 = 64;

/// 打开（或首次格式化后打开）TSDB 磁盘区段；返回 (链式 sink, 读面)。
///
/// 单位纪律：`base_block_512`/`region_blocks_512` 均以数据盘 512B 块计
/// （ota-ab 常量口径）；内部折算 LFS 的 4096B 几何（必须整除对齐）。
/// mount 失败且仅坏超级块（首次上电/异盘重划）时显式格式化一次——
/// 格式化只作用于已让渡区段头部 LFS 窗口（RegionScope 钳制），不涉整盘。
// 接缝契约与 TelemetryHistorySink 同理由：错误面为 ()（服务侧只判成败、
// bin 侧统一落"显式 Unavailable"文案），错误类型化属后续卷。
#[allow(clippy::result_unit_err)]
pub fn open_disk_history<C: EipcSend + Clone + 'static>(
    io: BlockIo<C>,
    base_block_512: u64,
    region_blocks_512: u64,
) -> Result<(HistoryChain, TsdbHistoryReader), ()> {
    let region_bytes = region_blocks_512 * 512;
    assert!(
        base_block_512 % 8 == 0 && region_bytes % FS_BLOCK_SIZE as u64 == 0,
        "TSDB 区段基址/长度必须与 LFS 4096B 几何对齐"
    );
    let face = EipcBlockDevice::new(io, FS_BLOCK_SIZE, region_blocks_512 / 8);
    let fs_blocks = core::cmp::min(face.block_count(), FS_VOLUME_MAX_BLOCKS_4K);
    // Face 以 4096B 计块；作用域基址按其口径折算（base512/8）。
    let base_block_4k = base_block_512 / 8;

    // 先 mount（保数据）；仅当坏超级块才格式化重试（fresh face 保用权）。
    let fs = {
        match RegionScope::new(face.clone(), base_block_4k, fs_blocks) {
            Ok(scope) => match Lfs::mount(alloc::boxed::Box::new(scope)) {
                Ok(fs) => fs,
                Err(FsError::BadSuperBlock) => {
                    let dev2 = face.clone();
                    let scope2 =
                        RegionScope::new(dev2, base_block_4k, fs_blocks).map_err(|_| ())?;
                    Lfs::format(alloc::boxed::Box::new(scope2)).map_err(|_| ())?
                }
                Err(_) => return Err(()),
            },
            Err(_) => return Err(()),
        }
    };

    let cfg = TsdbConfig {
        data_dir: alloc::string::String::from(DISK_DATA_DIR),
        max_points_per_chunk: TARGET_TSDB_CHUNK_POINTS,
        ..TsdbConfig::default()
    };
    let db = TimeSeriesDB::open(fs, cfg).map_err(|_| ())?;
    let shared: DiskDb = Rc::new(RefCell::new(db));
    Ok((
        HistoryChain {
            disk: TsdbHistorySink {
                db: shared.clone(),
                disk_errors: 0,
                batches_since_checkpoint: 0,
            },
            hot: HistoryBuffer::new(crate::telemetry::DEFAULT_HISTORY_CAP),
        },
        TsdbHistoryReader { db: shared },
    ))
}

// ============================================================================
// SPEC-C host 判据（DoD C1）：FakeEipc 全链——ingest 记录经真实 wire 帧 →
// BlockIo 块命令面 → RegionScope 区段钳制 → LFS 卷 → TSDB，回读逐字段
// 一致；重建 BlockIo 模拟掉电重启后重挂载（mount 路径非 format）数据完整。
// ============================================================================

#[cfg(test)]
mod spec_c_chain_tests {
    use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
    use eneros_drv_svc::runtime::{wire, DriverResponse, OP_EXECUTE, ST_OK};
    use eneros_durable_state::StoreError;

    use super::*;

    /// svc-storage 命令面内存仿真：整盘 Vec<u8>；共享句柄用于"掉电"
    /// 场景（驱动实例可弃、介质长存——tsdb virtio 掉电测试同法）。
    /// Clone（SPEC-B/C）：EipcBlockDevice 单件 clone 需 C: Clone。
    /// `cmds`：块命令往返计数器（共享；WP-68.3.3 性能闸门观测口）。
    /// `fail_next_write`：单发介质写失败 latch（共享；68.3 复查回归锁——
    /// 布防后下一个写命令失败一次即自解除，后续写正常）。
    #[derive(Clone)]
    struct FakeStorage {
        disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>,
        region_bytes: u64,
        cmds: alloc::rc::Rc<core::cell::Cell<u64>>,
        fail_next_write: alloc::rc::Rc<core::cell::Cell<bool>>,
    }

    impl FakeStorage {
        fn new(region_bytes: u64) -> Self {
            Self {
                disk: alloc::rc::Rc::new(core::cell::RefCell::new(
                    alloc::vec![0u8; region_bytes as usize],
                )),
                region_bytes,
                cmds: alloc::rc::Rc::new(core::cell::Cell::new(0)),
                fail_next_write: alloc::rc::Rc::new(core::cell::Cell::new(false)),
            }
        }
        fn take_handle(&self) -> Self {
            Self {
                disk: alloc::rc::Rc::clone(&self.disk),
                region_bytes: self.region_bytes,
                cmds: alloc::rc::Rc::clone(&self.cmds),
                fail_next_write: alloc::rc::Rc::clone(&self.fail_next_write),
            }
        }
        /// 布防单发写失败（下一次 do_write 失败一次后自动解除）。
        fn arm_one_write_failure(&self) {
            self.fail_next_write.set(true);
        }
        fn cmd_count(&self) -> u64 {
            self.cmds.get()
        }
        fn do_read(&self, off: u64, buf: &mut [u8]) -> Result<(), ()> {
            let end = (off as usize).checked_add(buf.len()).ok_or(())?;
            if end > self.disk.borrow().len() {
                return Err(());
            }
            let d = self.disk.borrow();
            buf.copy_from_slice(&d[off as usize..end]);
            Ok(())
        }
        fn do_write(&self, off: u64, data: &[u8]) -> Result<(), ()> {
            if self.fail_next_write.get() {
                self.fail_next_write.set(false);
                return Err(());
            }
            let end = (off as usize).checked_add(data.len()).ok_or(())?;
            if end > self.disk.borrow().len() {
                return Err(());
            }
            let mut d = self.disk.borrow_mut();
            d[off as usize..end].copy_from_slice(data);
            Ok(())
        }
    }

    impl eneros_agent_sdk::block_io::EipcSend for FakeStorage {
        fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()> {
            self.cmds.set(self.cmds.get() + 1);
            // 复用 drv-svc 真实 wire 解码——协议偏差在测试边界即暴露。
            let r = wire::decode_request(req).ok_or(())?;
            if r.opcode != OP_EXECUTE || r.payload_len == 0 {
                return Err(());
            }
            const OFF_BYTES: usize = core::mem::size_of::<u64>();
            let sub = r.payload[0];
            let mut resp = DriverResponse::new(r.correlation, ST_OK, 0xA1);
            match sub {
                BLK_CMD_READ => {
                    if (r.payload_len as usize) < 1 + OFF_BYTES + 4 {
                        return Err(());
                    }
                    let off = u64::from_le_bytes(r.payload[1..1 + OFF_BYTES].try_into().unwrap());
                    let n = u32::from_le_bytes(
                        r.payload[1 + OFF_BYTES..1 + OFF_BYTES + 4]
                            .try_into()
                            .unwrap(),
                    ) as usize;
                    self.do_read(off, &mut resp.payload[..n])?;
                    resp.payload_len = n as u16;
                }
                BLK_CMD_WRITE => {
                    if (r.payload_len as usize) < 1 + OFF_BYTES {
                        return Err(());
                    }
                    let off = u64::from_le_bytes(r.payload[1..1 + OFF_BYTES].try_into().unwrap());
                    self.do_write(off, &r.payload[1 + OFF_BYTES..r.payload_len as usize])?;
                }
                BLK_CMD_FLUSH => {}
                _ => return Err(()),
            }
            let _ = (StoreError::Unavailable,);
            let enc = wire::encode_response(&resp);
            Ok(enc.to_vec())
        }
    }

    fn rec(ts: u64, dev: u32, point: u32, value: f64, quality: u8) -> HistoryRecord {
        HistoryRecord {
            timestamp_ms: ts,
            device_id: dev,
            point,
            value,
            quality,
        }
    }

    const REGION_BLOCKS_512: u64 = 2048; // 1 MiB 测试区段

    /// 正例全链：写入 N 批 → 回读逐字段相等（含品质码）→ 截断页正确。
    #[test]
    fn c1_full_chain_ingest_query_fieldwise() {
        let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
        let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        let (mut chain, reader) =
            open_disk_history(io, 0, REGION_BLOCKS_512).expect("open(format-on-first)");
        // 三批跨点位/设备记录（品质档全覆盖）。
        let b1: Vec<HistoryRecord> = (0..10u64)
            .map(|i| rec(i * 100, 7, 3, i as f64 * 2.5, 0))
            .collect();
        let b2: Vec<HistoryRecord> = (10..20u64)
            .map(|i| rec(i * 100, 7, 3, i as f64 * 2.5, 1))
            .collect();
        let b3 = alloc::vec![rec(50, 9, 41, -12.75, 2), rec(60, 9, 41, f64::NAN, 3),];
        assert!(chain.write_batch(&b1).is_ok());
        assert!(chain.write_batch(&b2).is_ok());
        assert!(chain.write_batch(&b3).is_ok());

        // point=3：20 行全局升序、值/品质逐字段一致。
        let pr = reader.query_page(3, 100, 0).expect("point 3 exists");
        assert_eq!(pr.total, 20);
        for (i, row) in pr.rows.iter().enumerate() {
            let want_v = i as f64 * 2.5;
            assert_eq!(row.timestamp_ms, i as u64 * 100);
            assert!(
                (row.value - want_v).abs() < f64::EPSILON,
                "value 字段不一致 idx={}",
                i
            );
            assert_eq!(row.point, 3);
            assert_eq!(row.device_id, 7);
            assert_eq!(row.quality, if i < 10 { 0 } else { 1 }, "品质如实存档 {i}");
        }
        // point=41：NaN 保真 + Bad/Unknown 档位不抹平。
        let p41 = reader.query_page(41, 10, 0).expect("point 41");
        assert_eq!(p41.total, 2);
        assert_eq!(p41.rows[0].device_id, 9);
        assert!(p41.rows[0].value == -12.75 && p41.rows[0].quality == 2);
        assert!(p41.rows[1].value.is_nan(), "NaN 必须经压缩列存保真");
        // Unknown(3) 如实经 DB 往返（SPEC-A：DataQuality 增第 4 档，不再
        // 折叠为 Bad——对账基线上 SCADA 读到与生产一致的品质）。
        assert_eq!(p41.rows[1].quality, 3);

        // last_n 截断 + 分页游标一致性。
        let tail = reader.query_page(3, 5, 0).expect("tail page0");
        assert_eq!(tail.total, 20);
        assert_eq!(tail.rows.len(), 5);
        assert_eq!(tail.rows[0].timestamp_ms, 1500, "截尾必须取最新端");
        let page1 = reader.query_page(3, 5, 1).expect("page1 empty-tail");
        assert!(page1.rows.is_empty() && page1.total == 20);
    }

    /// 持久性三步法（host 口径）：写入 → 弃实例（掉电）→ 同介质重挂载
    /// （走 mount 非 format）读回一致；格式化不可静默重置历史卷。
    #[test]
    fn c3_power_loss_remount_preserves_data() {
        let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
        {
            let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
            let (mut chain, reader) = open_disk_history(io, 0, REGION_BLOCKS_512).expect("open");
            // 写满一个合并窗（窗内批次仅入 TSDB 缓冲，窗满批承担磁盘
            // checkpoint——掉电丢尾语义见 CHECKPOINT_EVERY_BATCHES）。
            for w in 0..CHECKPOINT_EVERY_BATCHES {
                let base = 1_000 + w * 1_000;
                let batch: Vec<HistoryRecord> = (0..2u64)
                    .map(|i| rec(base + i * 10, 7, 42, (w * 2 + i) as f64, 0))
                    .collect();
                assert!(chain.write_batch(&batch).is_ok(), "batch #{w}");
            }
            assert_eq!(
                reader
                    .query_page(42, (CHECKPOINT_EVERY_BATCHES * 2) as usize, 0)
                    .expect("pre")
                    .total,
                (CHECKPOINT_EVERY_BATCHES * 2) as u32
            );
            // —— 掉电：drop db/LFS 实例（块离开作用域），介质留存 ——
        }
        // —— 上电：新 BlockIo → open 必须 mount 成功并读回全部数据 ——
        let io2 = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        let (_chain2, reader2) = open_disk_history(io2, 0, REGION_BLOCKS_512).expect("remount");
        let total_want = (CHECKPOINT_EVERY_BATCHES * 2) as usize;
        let got = reader2.query_page(42, total_want, 0).expect("post");
        assert_eq!(
            got.total as usize, total_want,
            "掉电后窗内全部点必须完整（mount 不重建卷）"
        );
        for (i, row) in got.rows.iter().enumerate() {
            assert_eq!(
                row.timestamp_ms,
                1_000 + (i as u64 / 2) * 1_000 + (i as u64 % 2) * 10
            );
            assert_eq!(row.value, i as f64, "值一致性 idx={i}");
            assert_eq!(row.quality, 0);
        }
    }

    /// E 卷传输缺陷诊断（win1-t2 首写挂死）：16 KiB EL0 服务栈（main.rs
    /// STACK_SIZE=0x4000）+ 服务循环 ~5 KiB 槽位帧下，open→write→checkpoint
    /// 全链是否栈溢出。宿主 optimized-test 帧与 target release 同阶——
    /// 此测不过则 target 必挂；此测过只说明余量存在，target 结论仍以实弹为准。
    ///
    /// 栈尺寸经 `STACK_DIAG_BYTES` 覆盖（溢出=进程级 abort 不可捕获，
    /// 二分须逐进程跑）；缺省 64 KiB = 服务栈供给候选值，作常驻回归守门。
    #[test]
    fn diag_stack_write_path_survives() {
        let stack: usize = std::env::var("STACK_DIAG_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64 * 1024);
        let h = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || {
                // 垫 control_service_main 的 24 槽 clients 数组 + 帧局部。
                let pad: [u8; 5120] = [0u8; 5120];
                core::hint::black_box(&pad);
                let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
                let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
                let (mut chain, _reader) =
                    open_disk_history(io, 0, REGION_BLOCKS_512).expect("open");
                let batch: Vec<HistoryRecord> =
                    (0..6u64).map(|i| rec(i * 100, 1, 1, i as f64, 0)).collect();
                chain
                    .write_batch(&batch)
                    .expect("first write_batch (target 挂死路径)");
            })
            .expect("spawn");
        h.join()
            .expect("write path must survive at provisioned stack");
    }

    /// fail-closed：未授予/坏区域打开失败返回 Err（不伪造可用）；
    /// 同 sink 二次相同批次 = 追加语义（幂等性在上游 TEL_INGEST 全量替换
    /// 保证——模块头注记单源此处不再复制）。
    #[test]
    fn c1_open_failclosed_and_append_semantics() {
        // 区域越出介质容量 → RegionScope 建构拒绝 → open Err。
        let storage = FakeStorage::new(64 * 1024);
        let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        assert!(
            open_disk_history(io, 0, REGION_BLOCKS_512).is_err(),
            "越界区段必须拒绝"
        );
        // 追加语义直证。
        let storage2 = FakeStorage::new(REGION_BLOCKS_512 * 512);
        let io2 = BlockIo::new(storage2.take_handle(), "svc-storage", 0, 5_000);
        let (mut chain, reader) = open_disk_history(io2, 0, REGION_BLOCKS_512).expect("open");
        let one = alloc::vec![rec(7, 7, 7, 1.0, 0)];
        assert!(chain.write_batch(&one).is_ok());
        assert!(chain.write_batch(&one).is_ok());
        assert_eq!(reader.query_page(7, 10, 0).expect("p").total, 2);
    }

    /// SPEC-A：TELR 单源策略对**真服务端**生效——total=600/last_n 超上限
    /// （钳至 512）时 more 按窗口推导，客户端恰好收满 11 页，无空页往返
    /// （历史病灶按 total 推会多付第 12 页空行；蓝图 A 卷 A.2 行为锁定）。
    #[test]
    fn spec_a_window_more_on_real_reader_no_empty_page() {
        use eneros_agent_sdk::history_wire::{
            answer_history_query, decode_response, encode_query, HistQuery,
        };
        let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
        let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        let (mut chain, reader) = open_disk_history(io, 0, REGION_BLOCKS_512).expect("open");
        let batch: Vec<HistoryRecord> = (0..600u64)
            .map(|i| rec(i * 10, 7, 42, i as f64, 0))
            .collect();
        assert!(chain.write_batch(&batch).is_ok());
        // 模拟客户端逐页拉取（more 驱动）：页数 = ceil(512/48) = 11，无空页。
        let mut page = 0u32;
        let mut pages_fetched = 0u32;
        loop {
            let req = encode_query(&HistQuery {
                point: 42,
                last_n: u32::MAX,
                page,
            });
            let resp_bytes = answer_history_query(Some(&reader), &req);
            let r = decode_response(&resp_bytes).expect("decode");
            pages_fetched += 1;
            assert!(!r.rows.is_empty(), "无空页往返（page={page}）");
            if !r.more {
                break;
            }
            page += 1;
        }
        assert_eq!(
            pages_fetched, 11,
            "512 窗口 = 10 满页 + 1 尾页（32 行），无第 12 页"
        );
    }

    /// WP-68.3.3 性能闸门（host）：ACK 路径批与 checkpoint 批的块命令
    /// 双预算。基线对照：68.3 前（52B 分片 × 每操作 mount）同路径
    /// ~1.2 万次往返/操作（E-DEF-01 实测 2k-13k 上沿，8× 扩容反推）；
    /// 68.3 后 host 实测（2026-09-06）：checkpoint 批 1264、轻批两位数。
    /// TCG 目标侧再叠 ~5-15ms/命令物理成本 → 逐批 checkpoint 饿死决策
    /// 链（run3 实弹），故 checkpoint 合并窗 = 8 批
    /// （CHECKPOINT_EVERY_BATCHES，掉电丢尾语义见该常量文档）。
    #[test]
    fn c5_checkpoint_command_budget() {
        let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
        let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        let (mut chain, reader) = open_disk_history(io, 0, REGION_BLOCKS_512).expect("open");
        let after_open = storage.cmd_count();
        // 轻批（ACK 路径，无 checkpoint）：命令数必须两位数量级。
        let light: Vec<HistoryRecord> =
            (0..7u64).map(|i| rec(i * 100, 7, 3, i as f64, 0)).collect();
        assert!(chain.write_batch(&light).is_ok());
        let light_cmds = storage.cmd_count() - after_open;
        std::println!("[c5-budget] open_cmds={after_open} light_batch_cmds={light_cmds}");
        assert!(
            light_cmds <= 150,
            "轻批块命令 {light_cmds} > 150（ACK 路径必须轻）"
        );
        // 写满合并窗：第 CHECKPOINT_EVERY_BATCHES 批承担 checkpoint。
        let before_window = storage.cmd_count();
        for w in 1..CHECKPOINT_EVERY_BATCHES {
            let base = 1_000 + w * 1_000;
            let b: Vec<HistoryRecord> = (0..2u64)
                .map(|i| rec(base + i * 10, 7, 3, (w * 2 + i) as f64, 0))
                .collect();
            assert!(chain.write_batch(&b).is_ok(), "batch #{w}");
        }
        let window_cmds = storage.cmd_count() - before_window;
        let per_batch = window_cmds / (CHECKPOINT_EVERY_BATCHES - 1);
        std::println!("[c5-budget] window_cmds={window_cmds} per_batch_avg={per_batch}");
        // N=8：均值含 checkpoint 尖峰（~1.4k/8 ≈ 180）；轻批（ACK 路径）
        // 已单独 ≤150 锁定。均值界防分片回缩/重挂载复活。
        assert!(
            per_batch <= 250,
            "合并窗批均块命令 {per_batch} > 250（性能面回归：分片回缩/重挂载复活）"
        );
        assert!(reader.index_len() > 0, "索引必须有条目（观测口自检）");
    }

    /// 68.3 复查回归锁（2026-09-06）：合并窗满批的**本批写失败**必须如实
    /// 上抛——窗满分支执行 checkpoint，但 checkpoint 结果不得掩盖
    /// `write_batch` 的 Err（68.3 前行为 = `and_then` 短路、写错误优先；
    /// 掩盖即"后方 ACK"契约破损：丢批谎报 TSDB-DISK-COMMIT、无
    /// TSDB-DISK-DROPPED 观测行）。
    #[test]
    fn c6_window_boundary_write_failure_not_masked() {
        let storage = FakeStorage::new(REGION_BLOCKS_512 * 512);
        let io = BlockIo::new(storage.take_handle(), "svc-storage", 0, 5_000);
        let (mut chain, _reader) = open_disk_history(io, 0, REGION_BLOCKS_512).expect("open");
        // 批 1..=7（窗内轻批）：入 TSDB 内存缓冲，不触盘。
        for w in 0..(CHECKPOINT_EVERY_BATCHES - 1) {
            let base = 10_000 + w * 1_000;
            let b: Vec<HistoryRecord> = (0..2u64)
                .map(|i| rec(base + i * 10, 7, 3, (w * 2 + i) as f64, 0))
                .collect();
            assert!(chain.write_batch(&b).is_ok(), "轻批 #{w} 必须 OK");
        }
        // 批 8（窗满批）：点数 > TARGET_TSDB_CHUNK_POINTS 强制 chunk
        // flush 触盘；布防单发介质写失败 = 本批 write_batch 必 Err，
        // 而**后续** checkpoint（latch 已自解除）应 Ok。
        storage.arm_one_write_failure();
        let big: Vec<HistoryRecord> = (0..(TARGET_TSDB_CHUNK_POINTS as u64 + 1))
            .map(|i| rec(99_000 + i * 10, 7, 3, i as f64, 0))
            .collect();
        let r = chain.write_batch(&big);
        assert!(
            r.is_err(),
            "窗满批本批写失败必须上抛（不得被 checkpoint 结果掩盖）"
        );
    }
}
