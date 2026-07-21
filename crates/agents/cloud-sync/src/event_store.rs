//! 事件溯源存储（Event Sourcing）与 CRC32-IEEE 完整性校验（D8/D9）。
//!
//! 断网期间事件持久累积，容量有界：满时先 `compact()`（保留全部未同步 + 最近
//! 100 条已同步），仍满则驱逐最旧已同步事件腾位；全部未同步不可压缩时返回
//! `Err(SyncError::StoreFull)`——宁可显式背压报错，不静默丢数据（蓝图 §7.1）。

use alloc::vec::Vec;

use crate::SyncError;

/// compact 后保留的最近已同步事件条数（D9 内存水位）。
const COMPACT_KEEP_SYNCED: usize = 100;

/// CRC32-IEEE（反射多项式 0xEDB88320）查找表，编译期生成（D8）。
const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// CRC32-IEEE 查找表（const 256 项）。
static CRC32_TABLE: [u32; 256] = crc32_table();

/// CRC32-IEEE 校验和（D8；已知向量 `"123456789" -> 0xCBF43926`）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC32_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// 事件类型（蓝图 §3 六类事件；序列化码 0..=5，D11）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventType {
    /// 遥测（遥测/遥信量测数据）。
    Telemetry = 0,
    /// 状态（设备/Agent 状态变更）。
    Status = 1,
    /// 告警（v0.53.2 告警记录）。
    Alarm = 2,
    /// 控制日志（遥控/遥调操作留痕）。
    ControlLog = 3,
    /// 交易记录（电力市场成交）。
    TradeRecord = 4,
    /// 配置变更（点表/策略变更）。
    ConfigChange = 5,
}

/// 事件（6 pub 字段 + CRC32 完整性校验，D8）。
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// 全局递增偏移量（批次幂等键，batch_id=from_offset）。
    pub offset: u64,
    /// 事件时间戳（ms，D7 调用方注入，集成层由 v0.12.0 RTC 供给）。
    pub timestamp: u64,
    /// 事件类型。
    pub event_type: EventType,
    /// 事件载荷（原始字节）。
    pub payload: Vec<u8>,
    /// `crc32(payload)` 校验和。
    pub checksum: u32,
    /// 是否已同步到云端。
    pub synced: bool,
}

impl Event {
    /// 完整性校验：`crc32(payload) == checksum`（D8，蓝图 §7.3）。
    pub fn verify(&self) -> bool {
        crc32(&self.payload) == self.checksum
    }
}

/// 事件溯源存储（容量有界，满则压缩，D9）。
pub struct EventStore {
    /// 事件序列（offset 升序，append-only）。
    events: Vec<Event>,
    /// 压缩水位：最早保留事件的 offset（紧凑化后推进）。
    base_offset: u64,
    /// 下一个待分配 offset。
    current_offset: u64,
    /// 容量上限（事件条数）。
    max_events: usize,
}

impl EventStore {
    /// 构造容量为 `max_events` 的存储；`max_events == 0` 返回 `InvalidConfig`（D9）。
    pub fn new(max_events: usize) -> Result<Self, SyncError> {
        if max_events == 0 {
            return Err(SyncError::InvalidConfig);
        }
        Ok(Self {
            events: Vec::new(),
            base_offset: 0,
            current_offset: 0,
            max_events,
        })
    }

    /// 追加事件（D7 时间注入）：返回分配的递增 offset。
    ///
    /// 容量满时先 `compact()`，仍满则驱逐最旧已同步事件腾位（已同步数据云端
    /// 已持久，本地驱逐不丢数据）；全部未同步不可压缩时返回 `Err(StoreFull)`（D9）。
    pub fn append(
        &mut self,
        event_type: EventType,
        payload: &[u8],
        now: u64,
    ) -> Result<u64, SyncError> {
        if self.events.len() >= self.max_events {
            self.compact();
            // 压缩后仍满：驱逐最旧已同步事件为新事件腾位
            while self.events.len() >= self.max_events {
                match self.events.iter().position(|e| e.synced) {
                    Some(idx) => {
                        self.events.remove(idx);
                    }
                    // 全部未同步不可压缩：显式背压，不丢已有数据（D9）
                    None => return Err(SyncError::StoreFull),
                }
            }
            self.refresh_base_offset();
        }
        let offset = self.current_offset;
        self.events.push(Event {
            offset,
            timestamp: now,
            event_type,
            payload: payload.to_vec(),
            checksum: crc32(payload),
            synced: false,
        });
        self.current_offset += 1;
        Ok(offset)
    }

    /// 取前 `max` 条未同步事件引用（offset 升序）。
    pub fn get_unsynced(&self, max: usize) -> Vec<&Event> {
        self.events.iter().filter(|e| !e.synced).take(max).collect()
    }

    /// 将 `offset <= up_to_offset` 的事件标记已同步（幂等，蓝图 §6.4）。
    pub fn mark_synced(&mut self, up_to_offset: u64) {
        for e in self.events.iter_mut() {
            if e.offset <= up_to_offset {
                e.synced = true;
            }
        }
    }

    /// 压缩：移除最旧已同步事件，保留全部未同步 + 最近 100 条已同步，
    /// 同步推进 `base_offset`（D9）。
    pub fn compact(&mut self) {
        let synced_count = self.events.iter().filter(|e| e.synced).count();
        if synced_count <= COMPACT_KEEP_SYNCED {
            return;
        }
        let mut to_remove = synced_count - COMPACT_KEEP_SYNCED;
        let events = core::mem::take(&mut self.events);
        self.events = events
            .into_iter()
            .filter(|e| {
                if to_remove > 0 && e.synced {
                    to_remove -= 1;
                    false
                } else {
                    true
                }
            })
            .collect();
        self.refresh_base_offset();
    }

    /// 重算 base_offset：最早保留事件 offset；空存储则对齐 current_offset。
    fn refresh_base_offset(&mut self) {
        self.base_offset = match self.events.first() {
            Some(e) => e.offset,
            None => self.current_offset,
        };
    }

    /// 当前存储事件条数。
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 存储是否为空。
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 下一个待分配 offset。
    pub fn current_offset(&self) -> u64 {
        self.current_offset
    }

    /// 压缩水位（最早保留事件 offset）。
    pub fn base_offset(&self) -> u64 {
        self.base_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ES1 append 递增 offset + timestamp 注入（D7）。
    #[test]
    fn es1_append_offsets_and_timestamp() {
        let mut s = EventStore::new(10).unwrap();
        assert_eq!(s.append(EventType::Telemetry, b"p=100", 1000).unwrap(), 0);
        assert_eq!(s.append(EventType::Status, b"on", 1000).unwrap(), 1);
        assert_eq!(s.append(EventType::Alarm, b"ov", 1000).unwrap(), 2);
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert_eq!(s.current_offset(), 3);
        assert_eq!(s.base_offset(), 0);
        let unsynced = s.get_unsynced(10);
        assert_eq!(unsynced.len(), 3);
        assert!(unsynced.iter().all(|e| e.timestamp == 1000));
        assert_eq!(unsynced[1].event_type, EventType::Status);
    }

    /// ES2 checksum + verify（含篡改检出，蓝图 §7.3）。
    #[test]
    fn es2_checksum_and_verify() {
        let mut s = EventStore::new(10).unwrap();
        s.append(EventType::Telemetry, b"p=100", 1).unwrap();
        let unsynced = s.get_unsynced(1);
        let e = unsynced[0];
        assert_eq!(e.checksum, crc32(b"p=100"));
        assert!(e.verify());
        // 篡改 payload 后校验失败
        let mut tampered = e.clone();
        tampered.payload[0] = b'x';
        assert!(!tampered.verify());
    }

    /// ES3 get_unsynced 过滤 + take(max)。
    #[test]
    fn es3_get_unsynced_filter_and_take() {
        let mut s = EventStore::new(10).unwrap();
        for i in 0..5u64 {
            s.append(EventType::Status, b"s", i).unwrap();
        }
        s.mark_synced(1);
        let all = s.get_unsynced(10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].offset, 2);
        assert_eq!(all[2].offset, 4);
        let taken = s.get_unsynced(2);
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[1].offset, 3);
    }

    /// ES4 mark_synced 幂等。
    #[test]
    fn es4_mark_synced_idempotent() {
        let mut s = EventStore::new(10).unwrap();
        for _ in 0..3 {
            s.append(EventType::Status, b"s", 1).unwrap();
        }
        s.mark_synced(2);
        s.mark_synced(2);
        s.mark_synced(0);
        assert!(s.get_unsynced(10).is_empty());
        assert_eq!(s.len(), 3);
    }

    /// ES5 compact 保留未同步 + 最近 100 已同步 + base_offset 推进。
    #[test]
    fn es5_compact_keeps_unsynced_and_recent_100() {
        let mut s = EventStore::new(300).unwrap();
        for i in 0..150u64 {
            s.append(EventType::Telemetry, b"t", i).unwrap();
        }
        s.mark_synced(129); // offset 0..=129 共 130 条已同步
        s.compact();
        assert_eq!(s.len(), 120); // 100 已同步 + 20 未同步
        assert_eq!(s.base_offset(), 30); // 移除最旧 30 条已同步（offset 0..29）
        let unsynced = s.get_unsynced(200);
        assert_eq!(unsynced.len(), 20);
        assert_eq!(unsynced[0].offset, 130);
    }

    /// ES6 满自动 compact/驱逐后写入成功（蓝图场景：max=4，2 已同步）。
    #[test]
    fn es6_full_evicts_synced_then_writes() {
        let mut s = EventStore::new(4).unwrap();
        for i in 0..4u64 {
            s.append(EventType::Telemetry, b"t", i).unwrap();
        }
        s.mark_synced(1); // offset 0,1 已同步
        assert_eq!(s.append(EventType::Telemetry, b"t", 10).unwrap(), 4);
        assert_eq!(s.append(EventType::Telemetry, b"t", 11).unwrap(), 5);
        // 最终保留 offset 2..5，未同步零丢失
        assert_eq!(s.len(), 4);
        let unsynced = s.get_unsynced(10);
        assert_eq!(unsynced.len(), 4);
        assert_eq!(unsynced[0].offset, 2);
        assert_eq!(unsynced[3].offset, 5);
    }

    /// ES7 全未同步满 → StoreFull 且不丢已有（D9，蓝图 §7.1）。
    #[test]
    fn es7_full_all_unsynced_store_full() {
        let mut s = EventStore::new(4).unwrap();
        for i in 0..4u64 {
            s.append(EventType::Alarm, b"a", i).unwrap();
        }
        assert_eq!(
            s.append(EventType::Alarm, b"a", 99),
            Err(SyncError::StoreFull)
        );
        assert_eq!(s.len(), 4);
        assert_eq!(s.get_unsynced(10).len(), 4);
        assert_eq!(s.current_offset(), 4);
    }

    /// ES8 new(0) → InvalidConfig（D9）。
    #[test]
    fn es8_new_zero_invalid_config() {
        assert!(matches!(EventStore::new(0), Err(SyncError::InvalidConfig)));
    }

    /// ES9 crc32 已知向量（CRC32-IEEE "123456789" → 0xCBF43926，D8）。
    #[test]
    fn es9_crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
