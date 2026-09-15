//! EnerOS EventStore（蓝图 61 [07] §4，v0.314.32）——分区事件日志、恢复与重放。
//!
//! 唯一不变量（[07] §4.1）：**已 ACK 接入的事件在规定 RPO 内可恢复、有序重放
//! 和去重；日志满、损坏、慢消费者和保留删除都可见且按 QoS 降级。**
//!
//! 本 crate 是自包含新 crate（[07] §4.2），复用 [`eneros_storage::BlockDevice`]
//! 与 durable-log 的 CRC/恢复思想，但不把单记录单块实现包装后宣称完成——
//! 这里是真正的 **4KiB 块对齐 segment 流式格式 + 双槽消费位点 + 水位/保留**。
//!
//! # 1. 依赖选择（为什么是 `eneros-storage`）
//!
//! 候选有二：`boot/image-format` 的 `BlockDevice`（`sector_size/total_lbas/
//! read_blocks`）与 `drivers/storage` 的 [`eneros_storage::BlockDevice`]。
//! **选择 `eneros-storage`**，理由：
//!
//! - image-format 的 trait 是固件镜像解析侧的**只读**契约（没有
//!   `write/erase/flush`），物理上无法承载事件日志写路径；
//! - `eneros-storage::BlockDevice` 是存储栈统一接口（read/write/erase/
//!   flush/health），virtio-blk/eMMC/NVMe/mock 均已实现，与 durable-log
//!   同源（[07] §4.2 "复用 BlockDevice" 即指此 trait）；
//! - CRC32 取 `eneros_storage::crc32`（HYG-001：其实现已归一到
//!   `eneros_ipc_wire::crc32` 唯一出处，此处零新增实现）。
//!
//! # 状态登记（蓝图 66 WP-66.6.3；同名异物消歧见 CONTEXT.md 键表）
//!
//! 本 crate 与 `cloud-sync::event_store`（跨域同步事件缓存，生产在用）
//! 同名异物——裸写 `EventStore` 须以 crate 路径限定。上游断链 BLOCKED：
//! `ingest_bridge` / `store_fact_sink` / `snapshot-service` 三接缝共享
//! 前置 = 目标侧存储句柄移交（63.3 登记维持）。**机制深、生产零消费是
//! 裁决内事实**（非疏漏；62.3 SS-deferred 冻结登记）。
//!
//! 生产接线注意（durable-log 39 收尾复验同款纪律）：target 侧应把**角色分区
//! 作用域句柄**（`ScopedBlockDevice`）适配到本 crate 的 `D: BlockDevice` 上，
//! 整盘裸 handle 不得触及 LBA0/GPT——类型门在调用方完成。
//!
//! # 2. 物理格式（[07] §4.3 逐项，wire 常量唯一出处 = [`format`]）
//!
//! - **分区键** [`PartitionId`] = u32：`hash(tenant/site/QoS/source_id)` 由
//!   调用方计算，store 只见 id（[07] §4.3 "分区键：tenant/site/QoS/hash
//!   (source_id)" 的落地边界）。
//! - **segment** 默认 4MiB，可由 [`StoreConfig::segment_bytes`] 调整（≥8KiB、
//!   4096 对齐）；一切落盘以 4KiB 块对齐（`D::block_size()` 必须为 4096）。
//! - 设备布局：块 0 = superblock；块 1/2 = consumer offset 双槽；其后按
//!   `segment_bytes / 4096` 块均分为 segment 槽位（槽位号 = 全局槽 id；
//!   segment id = **每分区**单调计数，写进 header）。
//! - **segment header**（64B，位于段首块）：`magic "EVSG" / version u16 /
//!   flags u16 / partition u32 / segment id u32 / first offset u64 /
//!   created u64 / config digest u32 / header CRC32`。
//! - **record**（紧跟 header 流式紧凑排布，可跨块）：
//!   `len u32 / flags u16 / schema u16 / offset u64 / event_id 16B /
//!   payload CRC32 / payload`，头 36B；**record 最大 1152B**（payload ≤
//!   [`format::PAYLOAD_MAX`] = 1116B；ObservationEvent 128B、DomainEvent
//!   ≤1024B 均可容纳），超长 → [`StoreError::RecordTooLarge`]。
//!   `event_id = [0;16]` 保留（全零 record 头与擦除态歧义）→
//!   [`StoreError::BadEventId`]；恢复扫描把记录后的全零/全 FF 区域判为
//!   自由空间（干净结束），把"可解析但 CRC/offset 不过"的残片判为损坏。
//! - **segment trailer**（段尾块，seal 时写入 = commit marker）：
//!   `magic "EVTR" / record count u32 / last offset u64 / SHA-256(全段 record
//!   编码字节流) 32B / trailer CRC32`。
//! - **append 批量**：writer 攒批最多 [`StoreConfig::batch_records`]（默认
//!   64）条后自动落盘；**Q0 走 [`StoreConfig::q0_batch_records`]（默认 1 =
//!   写透，ACK 即 durable）**。时间维度（4ms 以先到为准）在本 crate 的同步
//!   no_std API 中无定时器可用，由调用方周期性调用 [`EventStore::flush`]
//!   表达（见 §5 偏离清单）。
//!
//! # 3. ACK/持久语义（RPO=0 for acked）
//!
//! - Q0（`flags & 0b11 == 0`，默认 `q0_batch_records = 1`）：`append` 返回
//!   `Ok(offset)` 前记录字节已写入设备并 `flush` 栅栏——**ACK 即 durable**。
//! - Q1–Q3：可攒批；`append` 返回 = 已受理（offset 已分配、读可见），持久化
//!   在批量落盘时完成。掉电时未落盘的 staged 记录丢失且不产生重复副作用
//!   （durable policy 是否回 ACK 由调用方按 [07] §4.6 决定）。
//! - 事件去重键 = 调用方传入的 `event_id`（16B，如 telemetry-model 的
//!   `first_16(SHA-256(...))`），store 原样持久、原样返回（[07] §4.4
//!   "重放保留原 event_id/event_time"——event_time 在 payload 内）。
//!
//! # 4. 恢复与消费（[07] §4.4）
//!
//! - [`EventStore::recover`]：扫描全部 segment 槽位——header 有效段逐条校验
//!   `len` 上限 / payload CRC32 / offset 连续性，**截断逻辑尾到最后一条有效
//!   CRC record，绝不返回损坏记录**；header 损坏段计入 `bad_segments`（fail-
//!   closed，不回收复用）。[`RecoveryReport`] 报告 `truncated_records`（截断
//!   点后可解析残片数，**best effort**——半写记录的 len 字段本身可能损坏）/
//!   `valid_records` / `bad_segments` / `truncated_bytes` / `offset_gaps` /
//!   `resealed_segments`。
//! - **offset 空洞语义**：段内 `record.offset` 跳号（预期 N 实得 >N）= 介质级
//!   损坏 → 与 CRC 损坏同判：停扫、截断、`offset_gaps += 1`、其后记录全部
//!   计入 `truncated_records`（fail-closed 不跳过继续读）。读路径
//!   （[`EventStore::read`]）遇不连续 → [`StoreError::OffsetGap`]。
//! - **consumer offset 单独双槽/世代持久**：块 1/2 交替写入，世代 u64 单调；
//!   写后读回校验 + flush。掉电不回退：撕裂写使新槽 CRC 无效 → 取两槽中
//!   世代更高的有效槽 → **commit 后读回必须 ≥ 已 commit 值**（结构保证，
//!   见 tests/consumer.rs）。单次持久承载全部 (group, partition) 条目，
//!   上限 254 条（超出 → [`StoreError::CommitTooLarge`]）。
//! - **quarantine**：就是一个普通分区——调用方把坏事件写入约定 PartitionId
//!   且不给 Snapshot consumer 订阅即可（[07] §4.4），store 无特殊逻辑。
//! - 读放行保证：[`EventStore::read`] 对每条记录重验 payload CRC32，保留原
//!   `event_id`/payload，[`ReadReport`] 带 `next_offset`；消费位点以下因
//!   保留删除不可达 → [`StoreError::OffsetGone`]。
//!
//! # 5. 容量与保留（[07] §4.5）
//!
//! - [`EventStore::watermarks`]：如实报告全局/每分区用量与 permille（‰，
//!   700=70%）。**70/85/95/100% 阈值动作策略在调用方**；store 只提供可选
//!   [`StoreConfig::reject_q2q3_at_95_percent`]：全局占用 ≥950‰ 时拒绝
//!   `flags & 0b11 ∈ {2,3}` 的 append（[07] §4.5 "95% 拒绝 Q3/Q2"），
//!   返回 [`StoreError::HighWatermark`]。
//! - [`EventStore::delete_full_segments_before`]：**只删完整 segment**
//!   （trailer 有效且整段 last offset < keep_after_offset，开放尾段绝不删）；
//!   返回 [`DeletedSegment`] 清单（segment digest = trailer SHA-256、offset
//!   范围、记录数）供调用方写审计。**consumer 进度与 legal hold 检查在
//!   调用方**（可用 [`EventStore::committed_offset`] 辅助），store 不越权。
//!
//! # 6. 实现与差分测试
//!
//! - [`mem::MemEventStore`]：内存 reference model（BTreeMap 分区 → segment
//!   Vec），与块设备实现走**同一 trait、同一 wire 编码、同一批量/水位/保留
//!   协议**（仅无介质 I/O）。
//! - [`block::BlockEventStore<D>`]：块设备实现。
//! - `tests/differential.rs`：同一随机操作序列跑两个实现，逐 API 比对
//!   结果（append/read/commit/recover/watermarks/retention/staged）。
//! - `tests/recovery.rs`：半写 header/record/trailer、CRC 错、offset 空洞、
//!   断电 100 次循环（随机 append N → 模拟掉电截断 → recover → 已 ACK 记录
//!   全部可读、零丢失零重复）。
//! - 测试后端 [`testdev`]：[`testdev::MemBlockDevice`]（内存 4KiB 块数组）
//!   与 [`testdev::FaultyBlockDevice`]（半写截断/丢写/写后位翻转注入），
//!   pub 供集成测试使用（host 测试/仿真专用，非生产后端）。
//!
//! # 7. 对卷册 §4.2 签名的偏离清单（最小显式化）
//!
//! 1. `append` 卷册只写 `event: &[u8]`，未定义 event_id/flags 来源。本 crate
//!    显式化为 `append(partition, event_id: [u8;16], flags: u16, payload:
//!    &[u8])`——event_id 是去重键必须由调用方产生（[02] §3.1 event_id 规则
//!    需要 SHA-256(身份+时间+值)，store 无时钟无 schema 语义）；flags 低 2 位
//!    编码 QoS（0=Q0..3=Q3，[07] §4.3/§4.5 的 QoS 降级都依赖它）。
//! 2. record 的 `schema u16` 字段保留为 0：schema/版本随 payload 自描述
//!    （ObservationEvent/DomainEvent wire 自带 schema），append 签名不为它
//!    增参。
//! 3. trait 在卷册 4 方法之外增加 `committed_offset` / `watermarks` /
//!    `delete_full_segments_before` / `flush` / `staged_records`——§4.4 消费
//!    位点读回、§4.5 容量保留与批量落盘栅栏要求的能力，卷册签名未列出。
//! 4. 批量"4ms 以先到为准"的时间维度：同步 no_std API 无定时器，由调用方
//!    周期 `flush()` 表达；计数维度（64 records，Q0 可配更小）自动触发。
//! 5. `truncated_records` 为 best effort（半写残片的 len 字段可能不可解析）；
//!    精确字节量见 `truncated_bytes`。
//!
//! # 8. no_std
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；依赖仅
//! `eneros-storage`、`sha2`（均 no_std 兼容）。aarch64-unknown-none 可编译。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod block;
pub mod format;
pub mod mem;
pub mod memdev;
pub mod scoped;

/// 蓝图 61 [07] §4.6.3：接入桥——IngestionModule → 分区事件日志
/// （StoreSink：ACK=写透、delivery 位点幂等交付）。
pub mod ingest_bridge;

use alloc::vec::Vec;
use core::fmt;

/// 日志偏移（分区内单调，从 0 起；u64 全程计数）。
pub type LogOffset = u64;
/// 分区键（[07] §4.3：tenant/site/QoS/hash(source_id) 由调用方折叠成 u32）。
pub type PartitionId = u32;
/// 消费组 id。
pub type ConsumerGroupId = u32;

/// 从 flags 取 QoS（低 2 位：0=Q0、1=Q1、2=Q2、3=Q3；[07] §4.3/§4.5）。
#[inline]
pub fn qos_of(flags: u16) -> u16 {
    flags & 0b11
}

/// 消费位点提交项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionOffset {
    /// 分区。
    pub partition: PartitionId,
    /// 已消费到的位点（下一条待读 offset）。
    pub offset: LogOffset,
}

impl PartitionOffset {
    /// 便捷构造。
    pub fn new(partition: PartitionId, offset: LogOffset) -> Self {
        Self { partition, offset }
    }
}

/// Store 错误（fail-closed：任何损坏路径都不返回数据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// 底层块设备 I/O 失败（含故障注入半写/丢写）。
    Io,
    /// superblock 全零/全 FF（从未 format——需显式 provisioning）。
    NotFormatted,
    /// 非零但 magic 不符（外来格式，不覆盖）。
    ForeignFormat,
    /// superblock 版本/CRC 不符（损坏）。
    BadSuperblock,
    /// 构造/open 参数非法（segment 尺寸、块大小、config digest 漂移等）。
    BadConfig,
    /// payload 超过 record 上限（record 最大 1152B）。
    RecordTooLarge,
    /// `event_id == [0;16]` 保留（与擦除态全零 record 头歧义，恢复扫描
    /// 无法区分自由空间），append 拒绝。
    BadEventId,
    /// 消费位点条目超出双槽持久容量（254 条）。
    CommitTooLarge,
    /// 设备/段槽位耗尽。
    Full,
    /// read 起点超过分区写入位置（从未写过）。
    OffsetOutOfRange,
    /// read 起点已因保留删除不可达。
    OffsetGone,
    /// 读路径遇 offset 跳号（须先 recover；恢复语义见 crate 文档 §4）。
    OffsetGap,
    /// 水位 ≥95% 且调用方启用 reject_q2q3 配置位，Q2/Q3 append 被拒。
    HighWatermark,
    /// 记录编码损坏（读路径遇坏 record——先须 recover 截断）。
    Corrupt,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            StoreError::Io => "block i/o error",
            StoreError::NotFormatted => "store not formatted",
            StoreError::ForeignFormat => "foreign format on device",
            StoreError::BadSuperblock => "superblock corrupt",
            StoreError::BadConfig => "invalid configuration",
            StoreError::RecordTooLarge => "record exceeds 1152B limit",
            StoreError::BadEventId => "reserved all-zero event_id",
            StoreError::CommitTooLarge => "consumer offset entries exceed dual-slot capacity",
            StoreError::Full => "store full",
            StoreError::OffsetOutOfRange => "read offset beyond write position",
            StoreError::OffsetGone => "read offset removed by retention",
            StoreError::OffsetGap => "offset gap detected",
            StoreError::HighWatermark => "watermark >=95%, Q2/Q3 rejected",
            StoreError::Corrupt => "record encoding corrupt",
        };
        f.write_str(s)
    }
}

/// 读出的单条记录（原样保留 event_id/payload，[07] §4.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef<'a> {
    /// 分区内单调 offset。
    pub offset: LogOffset,
    /// 调用方产生的 16B 去重键（原样往返）。
    pub event_id: [u8; 16],
    /// 原样往返的 flags（低 2 位 = QoS，见 [`qos_of`]）。
    pub flags: u16,
    /// record schema 字段（当前恒 0，见 crate 文档 §7.2）。
    pub schema: u16,
    /// 事件 payload。
    pub payload: &'a [u8],
}

impl RecordRef<'_> {
    /// QoS（flags 低 2 位）。
    #[inline]
    pub fn qos(&self) -> u16 {
        qos_of(self.flags)
    }
}

/// 记录访问器（[07] §4.2 `out: &mut dyn RecordVisitor`）。
///
/// 返回 `false` 提前停止读（已送出的记录有效）。
pub trait RecordVisitor {
    /// 逐条回调（分区内 offset 升序、无跳号）。
    fn record(&mut self, r: RecordRef<'_>) -> bool;
}

/// [`EventStore::read`] 的结果报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadReport {
    /// 实际送入 visitor 的记录数。
    pub records_read: u64,
    /// 送出的 payload 字节总数。
    pub payload_bytes: u64,
    /// 下一条记录的 offset（调用方下次 read 的 from；visitor 提前停止或无
    /// 记录时 = from）。
    pub next_offset: LogOffset,
}

/// [`EventStore::recover`] 的结果报告（[07] §4.4 + 故障可见性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// header 有效的 segment 数（全部分区）。
    pub segments_scanned: u32,
    /// 恢复确认的记录总数（CRC/offset 全部通过）。
    pub valid_records: u64,
    /// 截断点后的记录形残片数（best effort，含其后不可达记录；见 §7.5）。
    pub truncated_records: u64,
    /// 各段截断点之后到数据区末尾的总字节数。
    pub truncated_bytes: u64,
    /// header 损坏的 segment 数（槽位 fail-closed 保留，不复用）。
    pub bad_segments: u32,
    /// 段内 offset 跳号次数（每处一次）。
    pub offset_gaps: u32,
    /// recover 为陈旧开放段补写 trailer 的次数（崩溃于 seal 窗口）。
    pub resealed_segments: u32,
}

/// 保留删除的一条审计记录（调用方负责落审计事件，[07] §4.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletedSegment {
    /// 分区。
    pub partition: PartitionId,
    /// 每分区单调 segment id（header 字段）。
    pub segment_id: u32,
    /// 段首记录 offset。
    pub first_offset: LogOffset,
    /// 段末记录 offset（= first_offset + record_count - 1）。
    pub last_offset: LogOffset,
    /// 记录数。
    pub record_count: u64,
    /// segment digest = trailer SHA-256（全段 record 编码字节流摘要）。
    pub digest: [u8; 32],
}

/// 每分区水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionWatermark {
    /// 分区。
    pub partition: PartitionId,
    /// 该分区已写日志字节（含设备已确认 + 未落盘 staged 的 record 编码字节）。
    pub log_bytes: u64,
    /// 该分区已分配 segment 的容量字节。
    pub capacity_bytes: u64,
    /// log_bytes/capacity_bytes 千分比（capacity 0 → 0）。
    pub used_permille: u32,
}

/// 全局水位（[07] §4.5；阈值动作为调用方策略，store 如实报告）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatermarkReport {
    /// 设备总容量字节。
    pub capacity_bytes: u64,
    /// 物理占用字节（固定区 + 已分配 segment 槽位 × segment_bytes）。
    pub used_bytes: u64,
    /// used_bytes/capacity_bytes 千分比（700=70%、850=85%、950=95%，
    /// 上限 1000）。
    pub used_permille: u32,
    /// 每分区水位（按 [`PartitionId`] 升序）。
    pub partitions: Vec<PartitionWatermark>,
}

/// 构造配置（[07] §4.3：segment 尺寸/批量/QoS 水位位/时钟源）。
///
/// config digest（crc32）写入 superblock 与每个 segment header，open 时
/// 不一致 → [`StoreError::BadConfig`]（防配置漂移静默破坏格式）。
#[derive(Debug, Clone, Copy)]
pub struct StoreConfig {
    /// segment 大小（字节）：默认 4MiB；须 ≥8192 且 4096 对齐。
    pub segment_bytes: u32,
    /// 攒批上限（records，Q1–Q3）：默认 64，到量自动落盘。
    pub batch_records: u32,
    /// Q0 批量上限（"Q0 可配更小批量"，[07] §4.3）：默认 1 = 写透。
    pub q0_batch_records: u32,
    /// ≥950‰ 时拒绝 Q2/Q3 append（[07] §4.5 "95% 拒绝 Q3/Q2"）；默认关。
    pub reject_q2q3_at_95_percent: bool,
    /// segment header `created` 字段时间源（unix 秒）；no_std 默认恒 0。
    pub clock_unix_s: fn() -> u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            segment_bytes: 4 * 1024 * 1024,
            batch_records: 64,
            q0_batch_records: 1,
            reject_q2q3_at_95_percent: false,
            clock_unix_s: || 0,
        }
    }
}

impl StoreConfig {
    /// 校验配置（format/open 共用判据）。
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.segment_bytes < 8192
            || self.segment_bytes % format::BLOCK_SIZE as u32 != 0
            || self.batch_records == 0
            || self.q0_batch_records == 0
        {
            return Err(StoreError::BadConfig);
        }
        Ok(())
    }
}

/// 双槽消费位点镜像（generation, 条目表）——两实现共享类型。
pub type OffsetSlotImage = (u64, Vec<(u32, u32, u64)>);

/// 分区事件日志 trait（[07] §4.2 权威签名 + §7 偏离清单的显式扩展）。
pub trait EventStore {
    /// 追加一条事件，返回其分区内 offset。
    ///
    /// QoS = `flags & 0b11`；Q0（默认配置）写透后 ACK（ACK 即 durable），
    /// Q1–Q3 攒批（[`Self::flush`] 或批量到量落盘）。
    fn append(
        &mut self,
        partition: PartitionId,
        event_id: [u8; 16],
        flags: u16,
        payload: &[u8],
    ) -> Result<LogOffset, StoreError>;

    /// 从 `from` 起按序把记录送入 `out`，返回 [`ReadReport`]。
    ///
    /// 绝不返回损坏记录（逐条重验 CRC32）；`from` 超出写入位置 →
    /// [`StoreError::OffsetOutOfRange`]，已被保留删除 →
    /// [`StoreError::OffsetGone`]，遇跳号 → [`StoreError::OffsetGap`]。
    fn read(
        &self,
        partition: PartitionId,
        from: LogOffset,
        out: &mut dyn RecordVisitor,
    ) -> Result<ReadReport, StoreError>;

    /// 提交消费组位点（双槽/世代持久；掉电不回退，见 crate 文档 §4）。
    fn commit_consumer(
        &mut self,
        group: ConsumerGroupId,
        offsets: &[PartitionOffset],
    ) -> Result<(), StoreError>;

    /// 读回某消费组在某分区的已提交位点（无提交记录 → `None`）。
    fn committed_offset(&self, group: ConsumerGroupId, partition: PartitionId)
        -> Option<LogOffset>;

    /// 重启恢复（[07] §4.4）：扫描到最后有效 CRC record，截断逻辑尾，
    /// 绝不返回损坏记录；幂等（重复调用报告一致）。
    fn recover(&mut self) -> Result<RecoveryReport, StoreError>;

    /// 容量水位（[07] §4.5）：全局与每分区用量；阈值动作在调用方。
    fn watermarks(&self) -> WatermarkReport;

    /// 保留删除（[07] §4.5）：只删 `partition` 下**完整**且
    /// `last_offset < keep_after_offset` 的 segment（前缀），返回删除清单
    /// （digest/范围/记录数）供调用方写审计。consumer/legal hold 检查在
    /// 调用方。
    /// 删除 keep_after_offset 之前的完整段，**单次至多 max_segments 段**
    /// （REG-70.15-PRUNE-MEGAOP：无界删除突发会以 40 万+ 块 IPC 阻塞
    /// svc-telemetry 单线程循环 ⇒ 快照停摆 ⇒ 控制链停摆——调用方按窗
    /// 摊销，水位靠多窗排空）。
    fn delete_full_segments_before(
        &mut self,
        partition: PartitionId,
        keep_after_offset: LogOffset,
        max_segments: usize,
    ) -> Result<Vec<DeletedSegment>, StoreError>;

    /// 显式落盘栅栏：把所有 staged 记录写入设备并 flush 介质。
    fn flush(&mut self) -> Result<(), StoreError>;

    /// 已写入分区枚举（扫描自 segment 头——掉电重开后仍完整；交付面
    /// 枚举输入）。
    fn partitions(&self) -> alloc::collections::BTreeSet<PartitionId>;

    /// 分区当前未落盘的 staged 记录数（批量协议可观测性/差分比对用）。
    fn staged_records(&self, partition: PartitionId) -> u64;
}
